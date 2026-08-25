//! The provider service identity: one operator-imported Nostr secret key.
//!
//! It signs every public payload and derives the Iroh transport key, so the
//! advertised node id is stable across restarts. The key is held through
//! [`crate::secret_store`], never in the clear.

use anyhow::{Context, bail};
use fedi_decentralized_service_liquidity_manager::{Pubkey, ServiceResult};
use hkdf::Hkdf;
use nostr_sdk::Keys;
use sha2::Sha256;
use sqlx::Row;

use crate::database::Database;
use crate::secret_store::{self, EncryptedSecretRecord, SecretStore};
use crate::{failed_precondition, internal_error, invalid_argument};

const PROVIDER_IDENTITY_ID: i64 = 1;
pub(crate) const PROVIDER_NOSTR_SECRET: &str = "provider.nostr_secret_key";

/// HKDF domain separator for the public Iroh transport key. Deriving it from
/// the provider identity rather than generating it per boot is what makes the
/// advertised endpoint survive a restart; see [`ProductionProviderIdentity::derive_iroh_secret_key`].
const IROH_INFO: &[u8] = b"flip/v1/iroh";

/// Production provider signing key material loaded from encrypted local storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProductionProviderIdentity {
    /// Provider pubkey derived from the stored secret key.
    pub provider_pubkey: Pubkey,

    /// Hex Nostr secret key used by production Schnorr signing.
    pub nostr_secret_key_hex: String,
}

impl ProductionProviderIdentity {
    /// Public Iroh transport key for this provider identity.
    ///
    /// Derived, never generated: the resulting node id is the address FLIP
    /// advertises, and an endpoint identity regenerated on each boot would
    /// invalidate the published advertisement on every restart. Same
    /// construction as the Fleet Manager's `derive_iroh_secret_key`.
    pub(crate) fn derive_iroh_secret_key(&self) -> anyhow::Result<fedi_iroh_rpc::iroh::SecretKey> {
        let keys = Keys::parse(&self.nostr_secret_key_hex)
            .context("provider Nostr secret key is invalid")?;
        let hkdf = Hkdf::<Sha256>::new(None, &keys.secret_key().secret_bytes());
        let mut out = [0_u8; 32];
        hkdf.expand(IROH_INFO, &mut out)
            .expect("HKDF-SHA256 supports 32-byte output");
        Ok(fedi_iroh_rpc::iroh::SecretKey::from_bytes(&out))
    }
}

/// Looks up the installed provider pubkey, absent when none is installed.
///
/// For readers that treat "no identity yet" as a stage of setup rather than a
/// fault. Anything that needs an identity to act uses
/// [`load_provider_identity`] instead, which refuses.
pub(crate) async fn find_provider_identity(database: &Database) -> ServiceResult<Option<Pubkey>> {
    let provider_pubkey: Option<String> =
        sqlx::query_scalar("SELECT provider_pubkey FROM provider_identity WHERE id = ?")
            .bind(PROVIDER_IDENTITY_ID)
            .fetch_optional(database.pool())
            .await
            .map_err(internal_error)?;
    Ok(provider_pubkey.map(Pubkey))
}

/// Loads the installed provider pubkey.
pub(crate) async fn load_provider_identity(database: &Database) -> ServiceResult<Pubkey> {
    find_provider_identity(database).await?.ok_or_else(|| {
        failed_precondition(
            "provider identity is not installed; set FLIP_PROVIDER_NOSTR_SECRET_KEY",
        )
    })
}

/// Loads or imports the production provider signing identity.
pub(crate) async fn load_or_import_production_provider_identity(
    database: &Database,
    secret_store: &SecretStore,
    imported_secret_key: Option<&str>,
) -> anyhow::Result<Option<ProductionProviderIdentity>> {
    let stored_secret = load_provider_secret(database, secret_store).await?;
    let secret = match (stored_secret, imported_secret_key) {
        (Some(stored), Some(imported)) => {
            let stored_identity = production_identity_from_secret(&stored)
                .context("stored provider production key is invalid")?;
            let imported_identity = production_identity_from_secret(imported)
                .context("imported provider production key is invalid")?;
            if stored_identity.provider_pubkey != imported_identity.provider_pubkey {
                bail!("imported provider key does not match existing production provider identity");
            }
            stored
        }
        (Some(stored), None) => stored,
        (None, Some(imported)) => {
            let imported_identity = production_identity_from_secret(imported)
                .context("imported provider production key is invalid")?;
            upsert_provider_secret(database, secret_store, imported).await?;
            upsert_provider_identity(database, &imported_identity.provider_pubkey).await?;
            return Ok(Some(imported_identity));
        }
        (None, None) => return Ok(None),
    };

    let identity = production_identity_from_secret(&secret)?;
    upsert_provider_identity(database, &identity.provider_pubkey).await?;
    Ok(Some(identity))
}

/// Loads the installed production provider signing identity, if any.
///
/// Unlike [`load_or_import_production_provider_identity`] this never imports,
/// so it is safe to call from anything that only needs to read key material
/// (the derived Iroh transport key, for one).
pub(crate) async fn load_production_provider_identity(
    database: &Database,
    secret_store: &SecretStore,
) -> anyhow::Result<Option<ProductionProviderIdentity>> {
    match load_provider_secret(database, secret_store).await? {
        Some(secret) => Ok(Some(production_identity_from_secret(&secret)?)),
        None => Ok(None),
    }
}

/// Installs the production provider signing identity on a running daemon.
///
/// Install-only, matching the boot path's guard: a key that disagrees with the
/// one already installed is refused rather than rotating it, because
/// provider-key rotation is post-MVP and a live rotation would orphan every
/// published advertisement and the derived Iroh endpoint identity. Re-installing
/// the same key succeeds and reports `installed = false`.
pub(crate) async fn install_production_provider_identity(
    database: &Database,
    secret_store: &SecretStore,
    secret_key_hex: &str,
) -> ServiceResult<(ProductionProviderIdentity, bool)> {
    let candidate = production_identity_from_secret(secret_key_hex)
        .map_err(|error| invalid_argument(format!("{error:#}")))?;

    if let Some(stored) = load_provider_secret(database, secret_store)
        .await
        .map_err(internal_error)?
    {
        let stored_identity = production_identity_from_secret(&stored).map_err(internal_error)?;
        if stored_identity.provider_pubkey != candidate.provider_pubkey {
            return Err(failed_precondition(
                "a different provider identity is already installed; \
                 provider-key rotation is not supported",
            ));
        }
        return Ok((stored_identity, false));
    }

    upsert_provider_secret(database, secret_store, secret_key_hex)
        .await
        .map_err(internal_error)?;
    upsert_provider_identity(database, &candidate.provider_pubkey)
        .await
        .map_err(internal_error)?;
    // The public half only. This is the identity the advertisement, the public
    // Iroh node id, and every signature are derived from, so an operator needs
    // to see which one took effect and when.
    tracing::info!(
        provider_pubkey = %candidate.provider_pubkey.0,
        "installed the provider signing identity"
    );
    Ok((candidate, true))
}

pub(crate) fn production_identity_from_secret(
    secret_key_hex: &str,
) -> anyhow::Result<ProductionProviderIdentity> {
    let keys =
        Keys::parse(secret_key_hex.trim()).context("provider Nostr secret key is invalid")?;
    Ok(ProductionProviderIdentity {
        provider_pubkey: Pubkey(keys.public_key().to_hex()),
        nostr_secret_key_hex: keys.secret_key().to_secret_hex(),
    })
}

async fn upsert_provider_identity(
    database: &Database,
    provider_pubkey: &Pubkey,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO provider_identity \
         (id, provider_pubkey, created_at, updated_at) \
         VALUES (?, ?, unixepoch(), unixepoch()) \
         ON CONFLICT(id) DO UPDATE SET \
           provider_pubkey = excluded.provider_pubkey, \
           updated_at = unixepoch()",
    )
    .bind(PROVIDER_IDENTITY_ID)
    .bind(&provider_pubkey.0)
    .execute(database.pool())
    .await
    .context("upsert provider identity")?;
    Ok(())
}

async fn load_provider_secret(
    database: &Database,
    secret_store: &SecretStore,
) -> anyhow::Result<Option<String>> {
    let row = sqlx::query(
        "SELECT version, algorithm, key_id, nonce, ciphertext \
         FROM secret_records WHERE name = ?",
    )
    .bind(PROVIDER_NOSTR_SECRET)
    .fetch_optional(database.pool())
    .await
    .context("load provider signing secret")?;

    let Some(row) = row else {
        return Ok(None);
    };
    let record = EncryptedSecretRecord {
        version: row.get("version"),
        algorithm: row.get("algorithm"),
        key_id: row.get("key_id"),
        nonce: row.get("nonce"),
        ciphertext: row.get("ciphertext"),
    };
    Ok(Some(
        secret_store
            .decrypt(PROVIDER_NOSTR_SECRET, &record)
            .context("decrypt provider signing secret")?,
    ))
}

async fn upsert_provider_secret(
    database: &Database,
    secret_store: &SecretStore,
    secret_key_hex: &str,
) -> anyhow::Result<()> {
    let record = secret_store
        .encrypt(PROVIDER_NOSTR_SECRET, secret_key_hex.trim())
        .context("encrypt provider signing secret")?;
    secret_store::upsert_secret_record(PROVIDER_NOSTR_SECRET, &record)
        .execute(database.pool())
        .await
        .context("store provider signing secret")?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/identity.rs"]
mod tests;
