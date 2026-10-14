//! Validate encrypted guardian material before accepting or enabling it.
//!
//! The binary supplies its native module registry and network policy; core
//! checks the move's federation and the peer's effective transport identity.
//! All parsing/decryption runs off the async executor, in a fresh private
//! directory containing only the encrypted archive (never `private.json`).

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::ensure;
use fedi_decentralized_service_manifold_guardian::{FederationId, InviteCode};
use fedimint_server::config::ServerConfig;

use crate::facts::SeatNo;
use crate::identity::SeatKeys;
use crate::seat_process::{SeatProcessConfig, seat_dir};
use crate::transfer_package::TransferMaterial;

/// Native module validation and network policy, implemented at the binary's
/// composition root alongside its module selection. No permissive default.
pub trait GuardianConfigValidator: Send + Sync {
    fn validate(&self, config: &ServerConfig, network: bitcoin::Network) -> anyhow::Result<()>;
}

/// Neither parser errors nor their source chains may expose guardian material.
#[derive(Debug, thiserror::Error)]
#[error("invalid guardian transfer material")]
pub(crate) struct InvalidGuardian;

/// Validated identity for installation and destination endpoint proofs.
pub(crate) struct ValidatedGuardian {
    pub(crate) keys: SeatKeys,
    pub(crate) invite: InviteCode,
    /// Keep validation and installation tied to the same bytes.
    pub(crate) material: TransferMaterial,
}

/// This subtree contains only disposable transfer validation/install work,
/// never wallet state. Startup removes crash leftovers before creating workers.
pub(crate) fn transfer_work_dir(process: &SeatProcessConfig, seat_no: SeatNo) -> PathBuf {
    seat_dir(process, seat_no).join("guardian-transfer-work")
}

pub(crate) async fn clear_transfer_work(
    process: &SeatProcessConfig,
    seat_no: SeatNo,
) -> std::io::Result<()> {
    match tokio::fs::remove_dir_all(transfer_work_dir(process, seat_no)).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// An enabled import must still have exactly its accepted native identity on
/// disk. Never fall back to mnemonic keys, nor let a plaintext private config
/// override the accepted encrypted config.
pub(crate) async fn check_installed_material(
    process: &SeatProcessConfig,
    seat_no: SeatNo,
    accepted: &[u8],
) -> Result<(), InvalidGuardian> {
    use tokio::io::AsyncReadExt;
    let material = TransferMaterial::decode(accepted).map_err(|_| InvalidGuardian)?;
    let data = crate::seat_process::seat_data_dir(process, seat_no);
    match tokio::fs::symlink_metadata(data.join("private.json")).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(InvalidGuardian),
    }
    for (name, expected) in [
        ("private.encrypt", material.archive.private_encrypt.as_str()),
        ("private.salt", material.archive.private_salt.as_str()),
        ("local.json", material.archive.local_json.as_str()),
        ("consensus.json", material.archive.consensus_json.as_str()),
        ("password.private", material.api_auth.as_str()),
    ] {
        let mut bytes = Vec::new();
        tokio::fs::File::open(data.join(name))
            .await
            .map_err(|_| InvalidGuardian)?
            .take(expected.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| InvalidGuardian)?;
        if bytes != expected.as_bytes() {
            return Err(InvalidGuardian);
        }
    }
    Ok(())
}

/// Hydrate the effective keys before constructing the seat's API client. Used
/// both at daemon startup and when an idempotent CreateSeat repairs a missing
/// runtime entry. Imports never fall back to destination derivation.
pub(crate) async fn load_effective_keys(
    db: &crate::db::Db,
    identity: &crate::identity::RootMnemonic,
    process: &SeatProcessConfig,
    facts: &crate::facts::SeatFacts,
    validator: Arc<dyn GuardianConfigValidator>,
) -> anyhow::Result<SeatKeys> {
    if matches!(
        db.outgoing_transfer(&facts.seat_id).await?,
        Some(
            crate::transfer_state::OutgoingTransfer::Removing
                | crate::transfer_state::OutgoingTransfer::Removed
        )
    ) {
        // Removal has a permanent, higher-priority runtime fence and may have
        // already deleted native material. These placeholder keys cannot start
        // a child or authorize a guardian operation; don't depend on secrets
        // that this tombstone expressly permits to be gone.
        return Ok(identity.derive_seat_keys(&facts.seat_id));
    }
    let Some(incoming) = db.incoming_transfer(&facts.seat_id).await? else {
        return Ok(identity.derive_seat_keys(&facts.seat_id));
    };
    ensure!(
        incoming.binding.destination_seat_id == facts.seat_id
            && incoming.binding.fa_id == facts.fa_id
            && incoming.binding.destination_service_pubkey == identity.derive_service_pubkey(),
        "imported guardian destination mismatch"
    );
    let Some((bytes, _)) = incoming.material() else {
        return Ok(identity.derive_seat_keys(&facts.seat_id));
    };
    let validated = validate_material(
        db,
        process,
        facts.seat_no,
        TransferMaterial::decode(bytes)?,
        incoming.binding.federation_id.clone(),
        validator,
    )
    .await?;
    ensure!(
        incoming.invite_code() == Some(&validated.invite),
        "imported guardian invite mismatch"
    );
    if matches!(
        incoming.state,
        crate::transfer_state::IncomingState::Imported { .. }
    ) {
        check_installed_material(process, facts.seat_no, bytes).await?;
    }
    Ok(validated.keys)
}

#[cfg(test)]
pub(crate) struct RejectGuardianConfig;

#[cfg(test)]
impl GuardianConfigValidator for RejectGuardianConfig {
    fn validate(&self, _: &ServerConfig, _: bitcoin::Network) -> anyhow::Result<()> {
        anyhow::bail!("guardian config validation is not configured by this test")
    }
}

pub(crate) async fn validate_material(
    db: &crate::db::Db,
    process: &SeatProcessConfig,
    seat_no: SeatNo,
    material: TransferMaterial,
    federation: FederationId,
    validator: Arc<dyn GuardianConfigValidator>,
) -> Result<ValidatedGuardian, InvalidGuardian> {
    let work_dir = transfer_work_dir(process, seat_no);
    let network = process.bitcoin_network;
    let db = db.clone();
    // Native Argon2 is blocking. A panicking native decoder is a rejected
    // package, not a poisoned seat actor; known validator indexing assumptions
    // are also checked by the composition root before native validation.
    tokio::task::spawn_blocking(move || {
        // Even unexpected cancellation of the async caller must not release
        // the data-root lock while native code still owns a staging directory.
        let _db_guard = db;
        validate_blocking(&work_dir, material, federation, network, validator.as_ref())
            .map_err(|_| InvalidGuardian)
    })
    .await
    .map_err(|_| InvalidGuardian)?
}

fn validate_blocking(
    work_dir: &Path,
    material: TransferMaterial,
    federation: FederationId,
    network: bitcoin::Network,
    validator: &dyn GuardianConfigValidator,
) -> anyhow::Result<ValidatedGuardian> {
    // Recheck the bounded representation even for callers loading from disk.
    material.encode()?;
    ensure!(material.api_auth.trim() == material.api_auth);
    ensure!(!material.api_auth.contains('\0'));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(work_dir)?;
    let staging = tempfile::Builder::new()
        .prefix("validate-")
        .tempdir_in(work_dir)?;
    for (name, contents) in [
        ("private.encrypt", material.archive.private_encrypt.as_str()),
        ("private.salt", material.archive.private_salt.as_str()),
        ("local.json", material.archive.local_json.as_str()),
        ("consensus.json", material.archive.consensus_json.as_str()),
        ("password.private", material.api_auth.as_str()),
    ] {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(staging.path().join(name))?;
        file.write_all(contents.as_bytes())?;
    }
    let config = fedimint_server::config::io::read_server_config(staging.path())?;
    let endpoints = &config.consensus.iroh_endpoints;
    ensure!(!endpoints.is_empty());
    ensure!(
        endpoints
            .keys()
            .eq(config.consensus.broadcast_public_keys.keys())
    );
    ensure!(
        endpoints
            .keys()
            .enumerate()
            .all(|(i, peer)| peer.to_usize() == i)
    );
    // Refuse mixed transports: the imported identity is exactly the Iroh peer.
    ensure!(config.consensus.api_endpoints.is_empty());
    ensure!(config.consensus.tls_certs.is_empty());
    ensure!(config.local.p2p_endpoints.is_empty());
    let own = endpoints
        .get(&config.local.identity)
        .ok_or(InvalidGuardian)?;
    let api = config.private.iroh_api_sk.as_ref().ok_or(InvalidGuardian)?;
    let p2p = config.private.iroh_p2p_sk.as_ref().ok_or(InvalidGuardian)?;
    ensure!(api.public() == own.api_pk && p2p.public() == own.p2p_pk);
    ensure!(
        config
            .private
            .modules
            .keys()
            .eq(config.consensus.modules.keys())
    );
    ensure!(
        config
            .private
            .modules
            .iter()
            .all(|(id, private)| { private.kind() == &config.consensus.modules[id].kind })
    );
    ensure!(config.calculate_federation_id().to_string() == federation.0);
    validator.validate(&config, network)?;
    let invite = fedimint_core::invite_code::InviteCode::new(
        config.consensus.api_endpoints()[&config.local.identity]
            .url
            .clone(),
        config.local.identity,
        config.calculate_federation_id(),
        None,
    );
    Ok(ValidatedGuardian {
        keys: SeatKeys {
            iroh_api: iroh::SecretKey::from_bytes(&api.to_bytes()),
            iroh_p2p: iroh::SecretKey::from_bytes(&p2p.to_bytes()),
            api_auth: material.api_auth.clone(),
            preserve_guardian_identity: true,
        },
        invite: InviteCode(invite.to_string()),
        material,
    })
}

#[cfg(test)]
pub(crate) mod tests;
