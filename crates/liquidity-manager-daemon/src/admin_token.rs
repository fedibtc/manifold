//! Operator Admin API bearer token storage.
//!
//! The boot `--bootstrap-admin-token` is exactly that: a bootstrap. It is the
//! only credential a fresh deployment has, and it lives in the process
//! environment, so rotating it would otherwise mean editing deployment wiring
//! and restarting. A rotated token is persisted here instead, encrypted at rest
//! like every other daemon-owned secret, and takes over from the boot argument
//! the moment it exists.

use fedi_decentralized_service_liquidity_manager::ServiceResult;
use sqlx::Row;

use crate::database::Database;
use crate::secret_store::{self, EncryptedSecretRecord, SecretStore};
use crate::{internal_error, invalid_argument};

pub(crate) const ADMIN_TOKEN_SECRET: &str = "admin.api_token";

/// Shortest accepted replacement token. Not a strength claim — it only keeps a
/// rotation from locking the operator out with a trivially guessable value.
const MIN_TOKEN_LEN: usize = 16;

/// Replaces the Operator Admin API bearer token.
///
/// After this returns, the boot bootstrap token is no longer accepted.
pub(crate) async fn rotate(
    database: &Database,
    secret_store: &SecretStore,
    new_token: &str,
) -> ServiceResult<()> {
    if new_token.trim() != new_token {
        return Err(invalid_argument(
            "admin token must not have leading or trailing whitespace",
        ));
    }
    if new_token.len() < MIN_TOKEN_LEN {
        return Err(invalid_argument(format!(
            "admin token must be at least {MIN_TOKEN_LEN} characters"
        )));
    }

    let record = secret_store
        .encrypt(ADMIN_TOKEN_SECRET, new_token)
        .map_err(internal_error)?;
    secret_store::upsert_secret_record(ADMIN_TOKEN_SECRET, &record)
        .execute(database.pool())
        .await
        .map_err(internal_error)?;
    // No token material, and none of its shape: a rotation is worth a line
    // because it retires the boot token and locks out whoever held the old one.
    tracing::info!("rotated the Admin API token");

    Ok(())
}

/// Loads the rotated admin token, if one has been installed.
///
/// Returns `Ok(None)` when no rotation has happened, which is what makes the
/// boot bootstrap token still acceptable. A stored-but-undecryptable record is
/// an error rather than a fallback: silently reverting to the bootstrap token
/// would undo a rotation the operator believes took effect.
pub(crate) async fn load(
    database: &Database,
    secret_store: &SecretStore,
) -> anyhow::Result<Option<String>> {
    let row = sqlx::query(
        "SELECT version, algorithm, key_id, nonce, ciphertext \
         FROM secret_records WHERE name = ?",
    )
    .bind(ADMIN_TOKEN_SECRET)
    .fetch_optional(database.pool())
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    // `try_get`, not `get`: this runs inside the Admin API auth middleware,
    // where the panicking accessor would drop the connection instead of
    // answering, leaving the operator with no status to act on.
    let record = EncryptedSecretRecord {
        version: row.try_get("version")?,
        algorithm: row.try_get("algorithm")?,
        key_id: row.try_get("key_id")?,
        nonce: row.try_get("nonce")?,
        ciphertext: row.try_get("ciphertext")?,
    };
    Ok(Some(secret_store.decrypt(ADMIN_TOKEN_SECRET, &record)?))
}

#[cfg(test)]
#[path = "../tests/admin_token.rs"]
mod tests;
