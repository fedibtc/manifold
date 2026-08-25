use sqlx::{Any, AnyPool, Row, Transaction};

use fedi_decentralized_push_gateway_types::HookId;

/// Maximum hook lifetime admitted by the public management API.
pub const MAX_HOOK_LIFETIME_SECONDS: i64 = 31_536_000;
/// Bounded cleanup margin after a hook's terminal lifetime.
pub const IDEMPOTENCY_CLEANUP_MARGIN_SECONDS: i64 = 7 * 86_400;

/// Storage owner for compact accepted hook-idempotency markers.
#[derive(Clone, Debug)]
pub struct HookIdempotencyRepository {
    pool: AnyPool,
}

impl HookIdempotencyRepository {
    /// Creates a repository over the configured gateway database.
    #[must_use]
    pub fn new(pool: AnyPool) -> Self {
        Self { pool }
    }

    /// Finds an accepted key inside the caller's invocation transaction.
    pub async fn retained_target_count_in_transaction(
        transaction: &mut Transaction<'_, Any>,
        hook_id: &HookId,
        caller_idempotency_key: &str,
    ) -> Result<Option<usize>, sqlx::Error> {
        let target_count: Option<i64> = sqlx::query_scalar(
            "SELECT target_count FROM hook_idempotency_tombstones
             WHERE hook_id = $1 AND caller_idempotency_key = $2",
        )
        .bind(&hook_id.0)
        .bind(caller_idempotency_key)
        .fetch_optional(&mut **transaction)
        .await?;
        Ok(target_count.map(|count| count.try_into().unwrap_or(0)))
    }

    /// Reclaims a bounded expired batch and reports whether a new marker fits.
    pub async fn ensure_capacity_in_transaction(
        transaction: &mut Transaction<'_, Any>,
        now_timestamp: i64,
        max_rows_global: u64,
        reclamation_batch_size: u64,
    ) -> Result<bool, sqlx::Error> {
        Self::purge_expired_in_transaction(transaction, now_timestamp, reclamation_batch_size)
            .await?;
        if max_rows_global == 0 {
            return Ok(true);
        }
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hook_idempotency_tombstones")
            .fetch_one(&mut **transaction)
            .await?;
        Ok(u64::try_from(rows).unwrap_or(u64::MAX) < max_rows_global)
    }

    /// Records one accepted key in the same transaction as event/outbox rows.
    pub async fn record_in_transaction(
        transaction: &mut Transaction<'_, Any>,
        hook_id: &HookId,
        caller_idempotency_key: &str,
        target_count: usize,
        accepted_at: i64,
        hook_expires_at: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        let lifecycle_end = hook_expires_at
            .unwrap_or_else(|| accepted_at.saturating_add(MAX_HOOK_LIFETIME_SECONDS));
        let retain_until = lifecycle_end
            .max(accepted_at)
            .saturating_add(IDEMPOTENCY_CLEANUP_MARGIN_SECONDS);
        sqlx::query(
            "INSERT INTO hook_idempotency_tombstones (
                 hook_id, caller_idempotency_key, target_count, accepted_at, retain_until
             ) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&hook_id.0)
        .bind(caller_idempotency_key)
        .bind(i64::try_from(target_count).unwrap_or(i64::MAX))
        .bind(accepted_at)
        .bind(retain_until)
        .execute(&mut **transaction)
        .await?;
        Ok(())
    }

    /// Deletes expired compact markers outside a larger admission transaction.
    pub async fn purge_expired(&self, now_timestamp: i64) -> Result<u64, sqlx::Error> {
        Ok(
            sqlx::query("DELETE FROM hook_idempotency_tombstones WHERE retain_until < $1")
                .bind(now_timestamp)
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }

    /// Deletes at most one bounded expired batch inside another transaction.
    pub async fn purge_expired_in_transaction(
        transaction: &mut Transaction<'_, Any>,
        now_timestamp: i64,
        batch_size: u64,
    ) -> Result<u64, sqlx::Error> {
        if batch_size == 0 {
            return Ok(0);
        }
        let batch_size = i64::try_from(batch_size).unwrap_or(i64::MAX);
        Ok(sqlx::query(
            "DELETE FROM hook_idempotency_tombstones
             WHERE (hook_id, caller_idempotency_key) IN (
                 SELECT hook_id, caller_idempotency_key
                 FROM hook_idempotency_tombstones
                 WHERE retain_until < $1
                 ORDER BY retain_until
                 LIMIT $2
             )",
        )
        .bind(now_timestamp)
        .bind(batch_size)
        .execute(&mut **transaction)
        .await?
        .rows_affected())
    }

    /// Returns the number of compact retained markers.
    pub async fn row_count(&self) -> Result<i64, sqlx::Error> {
        sqlx::query("SELECT COUNT(*) AS rows FROM hook_idempotency_tombstones")
            .fetch_one(&self.pool)
            .await
            .map(|row| row.get("rows"))
    }
}
