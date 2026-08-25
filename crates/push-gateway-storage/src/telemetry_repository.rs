//! Encrypted FMan-wide telemetry targets.

use sqlx::{AnyPool, Row};

use crate::time::unix_timestamp;

/// Encrypted-at-rest material for one verified FMan target.
///
/// This type deliberately has no `Debug`: the ciphertext contains a bearer.
pub struct EncryptedTelemetryTarget {
    pub fman_pubkey: String,
    pub secret_nonce: Vec<u8>,
    pub secret_ciphertext: Vec<u8>,
}

/// Encrypted target returned only to the protected collector adapter.
pub struct StoredTelemetryTarget {
    pub secret_nonce: Vec<u8>,
    pub secret_ciphertext: Vec<u8>,
}

/// Low-cardinality telemetry persistence gauges.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TelemetryStorageMetrics {
    pub targets: i64,
}

/// Durable FMan telemetry repository.
#[derive(Clone, Debug)]
pub struct TelemetryRepository {
    pool: AnyPool,
}

impl TelemetryRepository {
    #[must_use]
    pub fn new(pool: AnyPool) -> Self {
        Self { pool }
    }

    /// Idempotently replace one FMan target.
    pub async fn upsert_verified_target(
        &self,
        target: &EncryptedTelemetryTarget,
    ) -> Result<(), sqlx::Error> {
        let now = unix_timestamp();
        sqlx::query(
            "INSERT INTO guardian_telemetry_fmans (
                fman_pubkey, secret_nonce, secret_ciphertext, created_at, updated_at
             ) VALUES ($1,$2,$3,$4,$4)
             ON CONFLICT (fman_pubkey) DO UPDATE SET
                secret_nonce = EXCLUDED.secret_nonce,
                secret_ciphertext = EXCLUDED.secret_ciphertext,
                updated_at = EXCLUDED.updated_at",
        )
        .bind(&target.fman_pubkey)
        .bind(&target.secret_nonce)
        .bind(&target.secret_ciphertext)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn target(
        &self,
        fman_pubkey: &str,
    ) -> Result<Option<StoredTelemetryTarget>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT secret_nonce, secret_ciphertext
             FROM guardian_telemetry_fmans WHERE fman_pubkey = $1",
        )
        .bind(fman_pubkey)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| StoredTelemetryTarget {
            secret_nonce: row.get("secret_nonce"),
            secret_ciphertext: row.get("secret_ciphertext"),
        }))
    }

    pub async fn metrics(&self) -> Result<TelemetryStorageMetrics, sqlx::Error> {
        Ok(TelemetryStorageMetrics {
            targets: sqlx::query_scalar("SELECT COUNT(*) FROM guardian_telemetry_fmans")
                .fetch_one(&self.pool)
                .await?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn repository() -> (tempfile::TempDir, TelemetryRepository) {
        let dir = tempfile::tempdir().unwrap();
        let url = format!(
            "sqlite://{}?mode=rwc",
            dir.path().join("push.sqlite").display()
        );
        let database = crate::Database::connect(&url).await.unwrap();
        (dir, TelemetryRepository::new(database.pool().clone()))
    }

    fn target() -> EncryptedTelemetryTarget {
        EncryptedTelemetryTarget {
            fman_pubkey: "22".repeat(32),
            secret_nonce: vec![1; 12],
            secret_ciphertext: vec![2; 48],
        }
    }

    #[tokio::test]
    async fn fman_target_is_idempotently_replaced() {
        let (_dir, repository) = repository().await;
        repository.upsert_verified_target(&target()).await.unwrap();
        repository.upsert_verified_target(&target()).await.unwrap();

        let stored = repository.target(&"22".repeat(32)).await.unwrap().unwrap();
        assert_eq!(stored.secret_ciphertext, vec![2; 48]);
        assert_eq!(repository.metrics().await.unwrap().targets, 1);

        let mut changed = target();
        changed.secret_ciphertext = vec![3; 48];
        repository.upsert_verified_target(&changed).await.unwrap();
        let stored = repository.target(&"22".repeat(32)).await.unwrap().unwrap();
        assert_eq!(stored.secret_ciphertext, vec![3; 48]);
        assert_eq!(repository.metrics().await.unwrap().targets, 1);
    }
}
