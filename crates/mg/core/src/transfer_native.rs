//! Validate moved guardian material before accepting or enabling it.
//!
//! The binary supplies its native module registry and network policy; core
//! checks the move's federation and the peer's effective transport identity.
//! Parsing and native validation run off the async executor, from the bytes
//! in memory; nothing is written until the seat owner installs the archive.

use std::sync::Arc;

use anyhow::ensure;
use fedi_decentralized_service_manifold_guardian::{
    FederationId, GuardianMaterial, InviteCode, SEAT_TRANSFER_MATERIAL_MAX_BYTES,
};
use fedimint_server::config::ServerConfig;

use crate::backup::{GuardianArchive, read_guardian_archive};
use crate::seat_process::SeatProcessConfig;
use crate::transfer_state::ImportedKeys;

/// The guardian's immutable config and the Iroh secrets it runs under, as
/// moved between MGs and as installed on the destination.
///
/// Neither `Debug` nor serialization exposes these secrets accidentally. Backup
/// and durable installation have separate, deliberately chosen representations.
#[derive(Clone)]
pub struct TransferMaterial {
    /// The immutable three-file guardian archive, never a consensus database.
    pub archive: GuardianArchive,
    /// The guardian's Iroh identity, the one its federation's config names.
    pub(crate) keys: ImportedKeys,
}

impl std::fmt::Debug for TransferMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TransferMaterial(redacted)")
    }
}

/// A sanitized refusal that never includes config or parser input.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum MaterialError {
    /// A file is missing, unreadable, or not UTF-8.
    #[error("invalid guardian transfer material")]
    Invalid,
    /// The material would exceed its RPC budget.
    #[error("guardian transfer material exceeds its size limit")]
    TooLarge,
}

impl TransferMaterial {
    /// Read only the bounded immutable config, never the guardian database.
    /// Oversize/missing files fail before source retirement is committed.
    pub(crate) async fn read(
        data_dir: &std::path::Path,
        keys: ImportedKeys,
    ) -> Result<Self, MaterialError> {
        let archive = read_guardian_archive(data_dir)
            .await
            .map_err(|_| MaterialError::Invalid)?
            .ok_or(MaterialError::Invalid)?;
        let material = Self { archive, keys };
        material.check_size()?;
        Ok(material)
    }

    /// Accept material from the wire only within its bounds.
    pub(crate) fn from_wire(material: GuardianMaterial) -> Result<Self, MaterialError> {
        let GuardianMaterial {
            private_json,
            local_json,
            consensus_json,
            iroh_api_sk,
            iroh_p2p_sk,
        } = material;
        let material = Self {
            archive: GuardianArchive {
                private_json,
                local_json,
                consensus_json,
            },
            keys: ImportedKeys::from_bytes(&iroh_api_sk, &iroh_p2p_sk),
        };
        material.check_size()?;
        Ok(material)
    }

    pub(crate) fn into_wire(self) -> GuardianMaterial {
        GuardianMaterial {
            private_json: self.archive.private_json,
            local_json: self.archive.local_json,
            consensus_json: self.archive.consensus_json,
            iroh_api_sk: self.keys.iroh_api.to_bytes(),
            iroh_p2p_sk: self.keys.iroh_p2p.to_bytes(),
        }
    }

    /// Check representability before retiring a source or answering a pull.
    pub fn check_size(&self) -> Result<(), MaterialError> {
        let mut total: usize = 64;
        for field in [
            &self.archive.private_json,
            &self.archive.local_json,
            &self.archive.consensus_json,
        ] {
            total = total
                .checked_add(field.len())
                .ok_or(MaterialError::TooLarge)?;
        }
        if SEAT_TRANSFER_MATERIAL_MAX_BYTES < total {
            return Err(MaterialError::TooLarge);
        }
        Ok(())
    }
}

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
    pub(crate) invite: InviteCode,
    /// Keep validation and installation tied to the same bytes.
    pub(crate) material: TransferMaterial,
}

/// This subtree contains only disposable transfer validation/install work,
/// never wallet state. Startup removes crash leftovers before creating workers.
/// The keys a seat runs under, before constructing its API client. Used both
/// at daemon startup and when an idempotent CreateSeat repairs a missing
/// runtime entry. An accepted import keeps the Iroh identity in its row;
/// everything else derives from the mnemonic.
pub(crate) async fn load_effective_keys(
    db: &crate::db::Db,
    identity: &crate::identity::RootMnemonic,
    facts: &crate::facts::SeatFacts,
) -> anyhow::Result<crate::identity::SeatKeys> {
    let derived = identity.derive_seat_keys(&facts.seat_id);
    if db.outgoing_transfer(&facts.seat_id).await?
        == Some(crate::transfer_state::OutgoingTransfer::Removed)
    {
        // Removal has a permanent, higher-priority runtime fence. These
        // placeholder keys cannot start a child or authorize a guardian
        // operation.
        return Ok(derived);
    }
    let Some(incoming) = db.incoming_transfer(&facts.seat_id).await? else {
        return Ok(derived);
    };
    ensure!(
        incoming.binding.destination_seat_id == facts.seat_id
            && incoming.binding.fa_id == facts.fa_id
            && incoming.binding.destination_service_pubkey == identity.derive_service_pubkey(),
        "imported guardian destination mismatch"
    );
    Ok(match incoming.accepted() {
        Some(keys) => keys.seat_keys(derived.api_auth),
        None => derived,
    })
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
    process: &SeatProcessConfig,
    material: TransferMaterial,
    federation: FederationId,
    validator: Arc<dyn GuardianConfigValidator>,
) -> Result<ValidatedGuardian, InvalidGuardian> {
    let network = process.bitcoin_network;
    // Native validation is blocking. A panicking native decoder is a rejected
    // guardian, not a poisoned seat actor; known validator indexing assumptions
    // are also checked by the composition root before native validation.
    tokio::task::spawn_blocking(move || {
        validate_blocking(material, federation, network, validator.as_ref())
            .map_err(|_| InvalidGuardian)
    })
    .await
    .map_err(|_| InvalidGuardian)?
}

fn validate_blocking(
    material: TransferMaterial,
    federation: FederationId,
    network: bitcoin::Network,
    validator: &dyn GuardianConfigValidator,
) -> anyhow::Result<ValidatedGuardian> {
    // Recheck the bounded representation even for callers loading from disk.
    material.check_size()?;
    // The same three `serde_json` reads fedimintd's plaintext loader does.
    let config = ServerConfig {
        consensus: serde_json::from_str(&material.archive.consensus_json)?,
        local: serde_json::from_str(&material.archive.local_json)?,
        private: serde_json::from_str(&material.archive.private_json)?,
    };
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
    // The secrets the source handed over are the ones its config names, both
    // as the peer's public identity and as the private keys the child loads.
    let api = config.private.iroh_api_sk.as_ref().ok_or(InvalidGuardian)?;
    let p2p = config.private.iroh_p2p_sk.as_ref().ok_or(InvalidGuardian)?;
    ensure!(api.public() == own.api_pk && p2p.public() == own.p2p_pk);
    ensure!(
        api.to_bytes() == material.keys.iroh_api.to_bytes()
            && p2p.to_bytes() == material.keys.iroh_p2p.to_bytes()
    );
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
        invite: InviteCode(invite.to_string()),
        material,
    })
}

#[cfg(test)]
pub(crate) mod tests;
