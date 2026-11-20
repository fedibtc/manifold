//! The writer transaction that consumes a private grant is the only way to
//! switch holders. Legacy evidence is a frozen, one-time migration input.

use sqlx::SqlitePool;

use super::{Db, DbError, now_ms};

/// Remember ambiguity across a crash between migration 0005 and 0019.
pub(super) async fn preserve_upgrade_boundary(pool: &SqlitePool) -> Result<(), DbError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let upgrading_multiple: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='holder_authorization_events')
         AND NOT EXISTS(SELECT 1 FROM sqlite_master WHERE name='holder_authorization')",
    )
    .fetch_one(&mut *tx)
    .await?;
    if upgrading_multiple {
        // Several credentials can belong to one holder. The old durable
        // rows were signed events: never choose between different authors.
        // Treat malformed evidence as ambiguous. The surviving singleton
        // still has to pass full offline authentication before adoption.
        let ambiguous: bool = sqlx::query_scalar(
            "SELECT COUNT(*)>1 AND (COUNT(DISTINCT author)<>1 OR COUNT(author)<>COUNT(*))
             FROM (SELECT CASE WHEN json_valid(event_json) THEN json_extract(event_json,'$.pubkey') END AS author
                   FROM holder_authorization_events)",
        )
            .fetch_one(&mut *tx)
            .await?;
        if ambiguous {
            sqlx::query("CREATE TABLE IF NOT EXISTS holder_binding_ambiguous_upgrade (id INTEGER PRIMARY KEY CHECK (id = 1))")
                .execute(&mut *tx).await?;
            sqlx::query("INSERT OR IGNORE INTO holder_binding_ambiguous_upgrade VALUES (1)")
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

impl Db {
    /// Disable outstanding invitations when this process is fenced.
    pub async fn clear_holder_grant(&self) -> Result<(), DbError> {
        sqlx::query("DELETE FROM holder_pairing_grant")
            .execute(self.pool())
            .await?;
        Ok(())
    }
    /// Replace only the pinned holder's existing event and only with a later
    /// signed statement. The pin check and write share one SQL statement.
    pub async fn renew_holder(
        &self,
        holder: nostr_sdk::PublicKey,
        issued_at: u64,
        event_json: &str,
    ) -> anyhow::Result<()> {
        let written = sqlx::query("INSERT INTO holder_authorization(id, authorization_issued_at, event_json)
             SELECT 1, ?, ? FROM holder_binding WHERE id=1 AND holder=?
             ON CONFLICT(id) DO UPDATE SET authorization_issued_at=excluded.authorization_issued_at, event_json=excluded.event_json
             WHERE holder_authorization.authorization_issued_at < excluded.authorization_issued_at")
            .bind(issued_at.to_be_bytes().to_vec()).bind(event_json)
            .bind(holder.to_bytes().to_vec()).execute(self.pool()).await?;
        if written.rows_affected() == 0 {
            anyhow::ensure!(
                self.pinned_holder().await? == Some(holder),
                "holder is not pinned"
            );
        }
        Ok(())
    }

    /// Check cheaply before doing cryptographic work. Commit always checks
    /// again.
    pub async fn holder_grant_live(&self, digest: [u8; 32]) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM holder_pairing_grant WHERE id=1 AND digest=? AND expires_at_ms>?)")
            .bind(digest.to_vec()).bind(now_ms()).fetch_one(self.pool()).await?)
    }

    /// Current holder only; a migration candidate does not establish a pin.
    pub async fn pinned_holder(&self) -> anyhow::Result<Option<nostr_sdk::PublicKey>> {
        let raw: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT holder FROM holder_binding WHERE id=1")
                .fetch_optional(self.pool())
                .await?;
        raw.map(|raw| nostr_sdk::PublicKey::from_slice(&raw))
            .transpose()
            .map_err(Into::into)
    }
    /// Read the local, pre-upgrade enrollment for cryptographic revalidation.
    pub async fn legacy_holder_evidence(&self) -> Result<Option<String>, DbError> {
        Ok(
            sqlx::query_scalar("SELECT event_json FROM holder_binding_legacy_evidence WHERE id=1")
                .fetch_optional(self.pool())
                .await?,
        )
    }

    /// Adopt precisely the locally migrated event after authentication. An
    /// intervening pairing or a changed active row makes adoption a no-op.
    pub async fn adopt_legacy_holder(
        &self,
        holder: nostr_sdk::PublicKey,
        event_json: &str,
    ) -> Result<(), DbError> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT OR IGNORE INTO holder_binding SELECT 1, ?, 'legacy_active'
             FROM holder_binding_legacy_evidence e JOIN holder_authorization h ON h.id=e.id
             WHERE e.event_json=? AND h.event_json=e.event_json",
        )
        .bind(holder.to_bytes().to_vec())
        .bind(event_json)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM holder_binding_legacy_evidence WHERE event_json=?")
            .bind(event_json)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Replace the previous private invitation. The caller hashes a random
    /// bearer with a process-local key so restored grants cannot be redeemed.
    pub async fn install_holder_grant(
        &self,
        digest: [u8; 32],
        expires_at_ms: i64,
    ) -> Result<(), DbError> {
        sqlx::query("INSERT OR REPLACE INTO holder_pairing_grant VALUES (1, ?, ?)")
            .bind(digest.to_vec())
            .bind(expires_at_ms)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Commit one authenticated submission. Replay, expiration and replacement
    /// of the invitation all fail before any ownership change.
    pub async fn commit_holder_pairing(
        &self,
        digest: [u8; 32],
        holder: nostr_sdk::PublicKey,
        issued_at: u64,
        event_json: &str,
    ) -> Result<bool, DbError> {
        let mut tx = self.begin_write().await?;
        let consumed = sqlx::query(
            "DELETE FROM holder_pairing_grant WHERE id=1 AND digest=? AND expires_at_ms > ?",
        )
        .bind(digest.to_vec())
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
        if consumed.rows_affected() == 0 {
            return Ok(false);
        }
        // A new grant authorizes a change of holder, not a rollback of an
        // already pinned holder's signed issue time.
        let stale: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM holder_binding b JOIN holder_authorization h ON h.id=b.id
             WHERE b.holder=? AND h.authorization_issued_at>=? AND h.event_json<>?)",
        )
        .bind(holder.to_bytes().to_vec())
        .bind(issued_at.to_be_bytes().to_vec())
        .bind(event_json)
        .fetch_one(&mut *tx)
        .await?;
        if stale {
            return Ok(false);
        }
        sqlx::query("INSERT OR REPLACE INTO holder_binding VALUES (1, ?, 'paired')")
            .bind(holder.to_bytes().to_vec())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT OR REPLACE INTO holder_authorization VALUES (1, ?, ?)")
            .bind(issued_at.to_be_bytes().to_vec())
            .bind(event_json)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM holder_binding_legacy_evidence")
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE onboarding_state SET stage='initial_offer', updated_at_ms=? WHERE id=1 AND stage='holder_authorization'")
            .bind(now_ms()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }
}
