use std::path::{Path, PathBuf};

use fedi_decentralized_service_fleet_manager::{
    SafeEventCursor, SafeEventJournal, SafeEventJournalIncarnation, TelemetryCapability,
};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use sqlx::{
    Row as _, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

use crate::{
    auth::VerifiedHttpAuth,
    cipher::SecretCipher,
    journal_target::{CollectionTarget, CommitOutcome, WorkTarget},
    journal_types::{JournalStreamId, ReceptionDay, ValidatedJournalBatch},
    metrics_policy::{MetricsIdentity, MetricsPolicy},
    metrics_snapshot::{MetricsSnapshot, MetricsTargetHealth},
    metrics_types::MetricsCommit,
};

const MAX_METRIC_STATE_BYTES: i64 = 32 * 1024 * 1024;
const MAX_METRIC_STATE_SAMPLES: i64 = 100_000;
const MAX_METRIC_SNAPSHOT_ROWS: i64 = MAX_TARGETS * 64;

/// Bounded result of loading persisted snapshots for one private exposition.
pub(crate) struct LoadedMetricSnapshots {
    /// Rows that passed the current full metrics policy.
    pub(crate) snapshots: Vec<MetricsSnapshot>,
    /// Bounded rows rejected before they could reach exposition.
    pub(crate) rejected: usize,
}

/// Cache identity for the active metrics target set and stored observations.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct MetricExpositionVersion {
    pub(crate) revision: i64,
    pub(crate) next_lease_expiry: Option<i64>,
}

/// One transactionally consistent view of private metrics exposition state.
pub(crate) struct MetricExpositionView {
    /// Cache identity read with the exposition rows.
    pub(crate) version: MetricExpositionVersion,
    /// Eligible latest guardian observations.
    pub(crate) snapshots: LoadedMetricSnapshots,
    /// Eligible active-target health observations.
    pub(crate) targets: Vec<MetricsTargetHealth>,
}

/// Durable source and archive position for one safe-journal stream.
#[derive(Clone)]
pub(crate) struct JournalStreamState {
    pub(crate) stream_id: JournalStreamId,
    pub(crate) journal: SafeEventJournal,
    pub(crate) incarnation: SafeEventJournalIncarnation,
    pub(crate) cursor: Option<SafeEventCursor>,
    pub(crate) observed_generation: u64,
}

/// One durable archive frame boundary used during crash recovery.
pub(crate) struct FrameBoundary {
    pub(crate) stream_id: JournalStreamId,
    pub(crate) day: ReceptionDay,
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) hash: [u8; 32],
}

/// Archive metadata committed atomically with a source cursor.
pub(crate) struct ArchiveFrame {
    pub(crate) day: ReceptionDay,
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) hash: [u8; 32],
}

/// Result of atomically admitting a verified registration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AdmissionOutcome {
    /// A new target or newer generation replaced the prior secret.
    Updated,
    /// The same generation and same signed request were already current.
    Idempotent,
}

/// Effective operational status of a persisted target.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TargetStatus {
    /// Target is admitted and its lease permits new work.
    Active,
    /// Target lease elapsed and no new work may start.
    Expired,
    /// An operator or definitive revocation quarantined the target.
    Quarantined,
}

/// Secret target fields accepted only after complete authentication.
pub(crate) struct TargetMaterial<'a> {
    pub(crate) fman_pubkey: &'a str,
    pub(crate) fman_name: &'a str,
    pub(crate) endpoint_id: &'a str,
    pub(crate) capability: &'a [u8; 32],
    pub(crate) generation: u64,
}

/// Durable collector state backed by one WAL-mode SQLite database.
#[derive(Clone)]
pub(crate) struct Store {
    pool: SqlitePool,
    cipher: SecretCipher,
    key_id: String,
    trust_profile: String,
    lease_seconds: i64,
    data_dir: PathBuf,
    #[cfg(test)]
    commit_hook: Option<std::sync::Arc<TestCommitHook>>,
    #[cfg(test)]
    metric_reservation_hook: Option<std::sync::Arc<TestCommitHook>>,
    #[cfg(test)]
    metric_commit_hook: Option<std::sync::Arc<TestCommitHook>>,
    #[cfg(test)]
    metric_exposition_hook: Option<std::sync::Arc<TestCommitHook>>,
    #[cfg(test)]
    metric_reservation_calls_before_failure: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
}

#[cfg(test)]
pub(crate) struct TestCommitHook {
    pub(crate) entered_once: std::sync::atomic::AtomicBool,
    pub(crate) entered: std::sync::Barrier,
    pub(crate) release: std::sync::Barrier,
}

#[cfg(test)]
fn wait_test_hook(hook: &Option<std::sync::Arc<TestCommitHook>>) {
    if let Some(hook) = hook
        && !hook
            .entered_once
            .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        hook.entered.wait();
        hook.release.wait();
    }
}

impl Store {
    #[cfg(test)]
    pub(crate) fn with_commit_hook(mut self, hook: std::sync::Arc<TestCommitHook>) -> Self {
        self.commit_hook = Some(hook);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_metric_reservation_hook(
        mut self,
        hook: std::sync::Arc<TestCommitHook>,
    ) -> Self {
        self.metric_reservation_hook = Some(hook);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_metric_commit_hook(mut self, hook: std::sync::Arc<TestCommitHook>) -> Self {
        self.metric_commit_hook = Some(hook);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_metric_exposition_hook(
        mut self,
        hook: std::sync::Arc<TestCommitHook>,
    ) -> Self {
        self.metric_exposition_hook = Some(hook);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_metric_reservation_failure_after(mut self, successful_calls: usize) -> Self {
        self.metric_reservation_calls_before_failure = Some(std::sync::Arc::new(
            std::sync::atomic::AtomicUsize::new(successful_calls),
        ));
        self
    }

    /// Open the database and apply embedded migrations.
    pub(crate) async fn open(
        path: &Path,
        trust_profile: &str,
        cipher: SecretCipher,
        key_id: String,
        lease_seconds: i64,
    ) -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Full)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let metadata = sqlx::query(
            "SELECT trust_profile, key_id, target_secret_format,
                    key_sentinel_nonce, key_sentinel_ciphertext
             FROM service_metadata WHERE singleton = 1",
        )
        .fetch_optional(&pool)
        .await?;
        let sentinel_aad = key_sentinel_aad(trust_profile, &key_id);
        if let Some(metadata) = metadata {
            let stored_profile: String = metadata.get("trust_profile");
            let stored_key_id: String = metadata.get("key_id");
            let stored_format: i64 = metadata.get("target_secret_format");
            if stored_profile != trust_profile {
                return Err(StoreError::EnvironmentMismatch);
            }
            let sentinel = cipher.decrypt(
                &metadata.get::<Vec<u8>, _>("key_sentinel_nonce"),
                &metadata.get::<Vec<u8>, _>("key_sentinel_ciphertext"),
                sentinel_aad.as_bytes(),
            );
            if stored_format != TARGET_SECRET_FORMAT {
                return Err(StoreError::UnsupportedSecretFormat);
            }
            if stored_key_id != key_id
                || !matches!(sentinel, Ok(ref plaintext) if plaintext == KEY_SENTINEL)
            {
                return Err(StoreError::KeyMismatch);
            }
        } else {
            let existing_state: i64 = sqlx::query_scalar(
                "SELECT
                    (SELECT COUNT(*) FROM targets) +
                    (SELECT COUNT(*) FROM auth_events) +
                    (SELECT COUNT(*) FROM journal_streams)",
            )
            .fetch_one(&pool)
            .await?;
            if existing_state != 0 {
                return Err(StoreError::KeyMismatch);
            }
            let (nonce, ciphertext) = cipher
                .encrypt(KEY_SENTINEL, sentinel_aad.as_bytes())
                .map_err(|_| StoreError::KeyMismatch)?;
            sqlx::query(
                "INSERT INTO service_metadata(
                    singleton, trust_profile, key_id, target_secret_format,
                    key_sentinel_nonce, key_sentinel_ciphertext
                 ) VALUES (1, ?, ?, ?, ?, ?)",
            )
            .bind(trust_profile)
            .bind(&key_id)
            .bind(TARGET_SECRET_FORMAT)
            .bind(nonce)
            .bind(ciphertext)
            .execute(&pool)
            .await?;
        }
        let data_dir = path.parent().ok_or(StoreError::UnsafeDataRoot)?.to_owned();
        crate::data_root_lock::secure_sqlite_files(&data_dir)?;
        Ok(Self {
            pool,
            cipher,
            key_id,
            trust_profile: trust_profile.to_owned(),
            lease_seconds,
            data_dir,
            #[cfg(test)]
            commit_hook: None,
            #[cfg(test)]
            metric_reservation_hook: None,
            #[cfg(test)]
            metric_commit_hook: None,
            #[cfg(test)]
            metric_exposition_hook: None,
            #[cfg(test)]
            metric_reservation_calls_before_failure: None,
        })
    }

    /// Durably consume one fresh signed event before expensive network verification.
    pub(crate) async fn reserve_auth(
        &self,
        auth: &VerifiedHttpAuth,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let result = async {
            sqlx::query("DELETE FROM auth_events WHERE expires_at < ?")
                .bind(now)
                .execute(&mut *transaction)
                .await?;
            if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM auth_events")
                .fetch_one(&mut *transaction)
                .await?
                >= 4096
            {
                return Err(StoreError::Saturated);
            }
            let inserted = sqlx::query(
                "INSERT INTO auth_events(event_id, expires_at) VALUES (?, ?)
             ON CONFLICT(event_id) DO NOTHING",
            )
            .bind(&auth.event_id)
            .bind(auth.created_at.checked_add(65).ok_or(StoreError::Refused)?)
            .execute(&mut *transaction)
            .await?;
            if inserted.rows_affected() != 1 {
                return Err(StoreError::Replay);
            }
            Ok(())
        }
        .await;
        finish_immediate(transaction, result).await
    }

    /// Atomically enforce replay, time, and generation ordering before replacing a secret.
    pub(crate) async fn admit(
        &self,
        auth: &VerifiedHttpAuth,
        target: TargetMaterial<'_>,
        now: i64,
    ) -> Result<AdmissionOutcome, StoreError> {
        let generation = i64::try_from(target.generation).map_err(|_| StoreError::Refused)?;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let result = async {
        let existing = sqlx::query(
            "SELECT target_id, key_id, generation, auth_created_at, registration_revision,
                     secret_nonce, secret_ciphertext, lease_until
             FROM targets WHERE fman_pubkey = ?",
        )
        .bind(target.fman_pubkey)
        .fetch_optional(&mut *transaction)
        .await?;
        let plaintext = serde_json::to_vec(&TargetSecret {
            format: TARGET_SECRET_FORMAT,
            endpoint_id: target.endpoint_id.to_owned(),
            capability: *target.capability,
        })
            .map_err(|_| StoreError::Secret)?;
        let mut revision_bump = false;
        let target_id = existing
            .as_ref()
            .map(|row| row.get::<String, _>("target_id"))
            .unwrap_or_else(random_id);
        let mut expired = false;
        let outcome = if let Some(row) = existing {
            expired = row.get::<i64, _>("lease_until") <= now;
            let current_generation: i64 = row.get("generation");
            let current_auth: i64 = row.get("auth_created_at");
            let stored_key_id: String = row.get("key_id");
            if stored_key_id != self.key_id {
                return Err(StoreError::Secret);
            }
            if generation < current_generation || auth.created_at < current_auth {
                return Err(StoreError::Refused);
            }
            if generation == current_generation {
                let aad = target_secret_aad(
                    &self.trust_profile,
                    &stored_key_id,
                    target.fman_pubkey,
                    &target_id,
                    current_generation,
                    current_auth,
                )?;
                let previous = self
                    .cipher
                    .decrypt(
                        row.get::<Vec<u8>, _>("secret_nonce").as_slice(),
                        row.get::<Vec<u8>, _>("secret_ciphertext").as_slice(),
                        &aad,
                    )
                    .map_err(|_| StoreError::Secret)?;
                let previous: TargetSecret =
                    serde_json::from_slice(&previous).map_err(|_| StoreError::Secret)?;
                if previous.format != TARGET_SECRET_FORMAT
                    || previous.capability != *target.capability
                {
                    return Err(StoreError::Refused);
                }
                revision_bump = previous.endpoint_id != target.endpoint_id;
                if revision_bump {
                    AdmissionOutcome::Updated
                } else {
                    AdmissionOutcome::Idempotent
                }
            } else {
                revision_bump = true;
                AdmissionOutcome::Updated
            }
        } else {
            if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM targets")
                .fetch_one(&mut *transaction)
                .await?
                >= MAX_TARGETS
            {
                return Err(StoreError::Saturated);
            }
            AdmissionOutcome::Updated
        };

        let aad = target_secret_aad(
            &self.trust_profile,
            &self.key_id,
            target.fman_pubkey,
            &target_id,
            generation,
            auth.created_at,
        )?;
        let (nonce, ciphertext) = self
            .cipher
            .encrypt(&plaintext, &aad)
            .map_err(|_| StoreError::Secret)?;
        let lease_until = now
            .checked_add(self.lease_seconds)
            .ok_or(StoreError::Refused)?;
        sqlx::query(
            "INSERT INTO targets (
                fman_pubkey,target_id,fman_name,key_id,secret_nonce,secret_ciphertext,
                generation,auth_created_at,lease_until,registration_revision,status,created_at,updated_at
             ) VALUES (?,?,?,?,?,?,?,?,?,1,'active',?,?)
             ON CONFLICT(fman_pubkey) DO UPDATE SET
                fman_name=excluded.fman_name,key_id=excluded.key_id,
                secret_nonce=excluded.secret_nonce,secret_ciphertext=excluded.secret_ciphertext,
                generation=excluded.generation,auth_created_at=excluded.auth_created_at,
                lease_until=excluded.lease_until,
                registration_revision=targets.registration_revision +
                  CASE WHEN ? THEN 1 ELSE 0 END,
                status=targets.status,updated_at=excluded.updated_at",
        )
        .bind(target.fman_pubkey).bind(target_id).bind(target.fman_name).bind(&self.key_id)
        .bind(nonce).bind(ciphertext).bind(generation).bind(auth.created_at).bind(lease_until)
        .bind(now).bind(now).bind(revision_bump).execute(&mut *transaction).await?;
        if expired {
            sqlx::query(
                "DELETE FROM metric_snapshots WHERE target_id =
                 (SELECT target_id FROM targets WHERE fman_pubkey = ?)",
            )
            .bind(target.fman_pubkey)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "UPDATE metric_poll_state SET last_complete_at=NULL WHERE target_id =
                 (SELECT target_id FROM targets WHERE fman_pubkey = ?)",
            )
            .bind(target.fman_pubkey)
            .execute(&mut *transaction)
            .await?;
        }
        if expired || outcome == AdmissionOutcome::Updated {
            sqlx::query(
                "UPDATE metric_exposition_revision SET revision=revision+1 WHERE singleton=1",
            )
            .execute(&mut *transaction)
            .await?;
        }
        Ok(outcome)
        }
        .await;
        finish_immediate(transaction, result).await
    }

    /// Return the effective target status without exposing target material.
    #[cfg(test)]
    pub async fn target_status(
        &self,
        fman_pubkey: &str,
        now: i64,
    ) -> Result<Option<TargetStatus>, StoreError> {
        let row = sqlx::query("SELECT status, lease_until FROM targets WHERE fman_pubkey = ?")
            .bind(fman_pubkey)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            Ok(
                match (
                    row.get::<String, _>("status").as_str(),
                    row.get::<i64, _>("lease_until") > now,
                ) {
                    ("quarantined", _) => TargetStatus::Quarantined,
                    ("active", false) => TargetStatus::Expired,
                    ("active", true) => TargetStatus::Active,
                    _ => return Err(StoreError::InvalidStatus),
                },
            )
        })
        .transpose()
    }

    /// Quarantine a target and advance its worker fence.
    #[cfg(test)]
    pub async fn quarantine(&self, fman_pubkey: &str) -> Result<bool, StoreError> {
        self.set_status(fman_pubkey, "quarantined").await
    }

    /// Explicitly reactivate a quarantined target and advance its worker fence.
    #[cfg(test)]
    pub async fn reactivate(&self, fman_pubkey: &str) -> Result<bool, StoreError> {
        self.set_status(fman_pubkey, "active").await
    }

    #[cfg(test)]
    async fn set_status(&self, fman_pubkey: &str, status: &str) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let target_id: Option<String> =
            sqlx::query_scalar("SELECT target_id FROM targets WHERE fman_pubkey = ?")
                .bind(fman_pubkey)
                .fetch_optional(&mut *transaction)
                .await?;
        let changed = sqlx::query(
            "UPDATE targets SET status = ?, registration_revision = registration_revision + 1
             WHERE fman_pubkey = ? AND status != ?",
        )
        .bind(status)
        .bind(fman_pubkey)
        .bind(status)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if status == "quarantined"
            && let Some(target_id) = target_id
        {
            sqlx::query("DELETE FROM metric_snapshots WHERE target_id = ?")
                .bind(target_id)
                .execute(&mut *transaction)
                .await?;
        }
        if changed == 1 {
            sqlx::query(
                "UPDATE metric_exposition_revision SET revision=revision+1 WHERE singleton=1",
            )
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(changed == 1)
    }

    /// Bind durable metrics state to the exact source and inventory policy.
    pub(crate) async fn configure_metrics_policy(
        &self,
        fingerprint: &str,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current: Option<String> =
            sqlx::query_scalar("SELECT fingerprint FROM metric_policy WHERE singleton = 1")
                .fetch_optional(&mut *transaction)
                .await?;
        if current.as_deref() != Some(fingerprint) {
            sqlx::query("DELETE FROM metric_snapshots")
                .execute(&mut *transaction)
                .await?;
            sqlx::query("DELETE FROM metric_poll_state")
                .execute(&mut *transaction)
                .await?;
            sqlx::query(
                "INSERT INTO metric_policy(singleton,fingerprint) VALUES(1,?)
                 ON CONFLICT(singleton) DO UPDATE SET fingerprint=excluded.fingerprint",
            )
            .bind(fingerprint)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "UPDATE metric_exposition_revision SET revision=revision+1 WHERE singleton=1",
            )
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Return only targets whose durable next-attempt deadline has elapsed.
    pub(crate) async fn due_metric_targets(
        &self,
        now: i64,
    ) -> Result<Vec<CollectionTarget>, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let removed = sqlx::query(
            "DELETE FROM metric_snapshots WHERE target_id IN
             (SELECT target_id FROM targets WHERE status!='active' OR lease_until <= ?)",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if removed > 0 {
            sqlx::query(
                "UPDATE metric_exposition_revision SET revision=revision+1 WHERE singleton=1",
            )
            .execute(&mut *transaction)
            .await?;
        }
        let rows = sqlx::query(
            "SELECT t.target_id,t.registration_revision FROM targets t
             LEFT JOIN metric_poll_state p ON p.target_id=t.target_id
             WHERE t.status='active' AND t.lease_until > ?
               AND (p.next_due_at IS NULL OR p.next_due_at <= ?)
             ORDER BY t.target_id LIMIT ?",
        )
        .bind(now)
        .bind(now)
        .bind(MAX_TARGETS)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter()
            .map(|row| {
                Ok(CollectionTarget {
                    target_id: row.get("target_id"),
                    registration_revision: u64::try_from(
                        row.get::<i64, _>("registration_revision"),
                    )
                    .map_err(|_| StoreError::Corrupt)?,
                })
            })
            .collect()
    }

    /// Earliest durable metrics deadline among active, unexpired targets.
    pub(crate) async fn next_metric_due_at(&self, now: i64) -> Result<Option<i64>, StoreError> {
        sqlx::query_scalar(
            "SELECT MIN(COALESCE(p.next_due_at, 0))
             FROM targets t LEFT JOIN metric_poll_state p ON p.target_id=t.target_id
             WHERE t.status='active' AND t.lease_until > ?",
        )
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// Reserve an attempt and its next deadline before any network work starts.
    #[cfg(test)]
    pub(crate) async fn begin_metric_work(
        &self,
        target: &CollectionTarget,
        now: i64,
        cadence_seconds: i64,
    ) -> Result<Option<WorkTarget>, StoreError> {
        if !self
            .reserve_metric_attempt(target, now, cadence_seconds)
            .await?
        {
            return Ok(None);
        }
        self.begin_collection_work(target, now).await
    }

    /// Commit the cadence fence before target resolution or network work.
    pub(crate) async fn reserve_metric_attempt(
        &self,
        target: &CollectionTarget,
        now: i64,
        cadence_seconds: i64,
    ) -> Result<bool, StoreError> {
        #[cfg(test)]
        if let Some(calls) = &self.metric_reservation_calls_before_failure
            && calls
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_err()
        {
            return Err(StoreError::Corrupt);
        }
        let next_due = now
            .checked_add(cadence_seconds)
            .ok_or(StoreError::Corrupt)?;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let eligible: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM targets t LEFT JOIN metric_poll_state p ON p.target_id=t.target_id
             WHERE t.target_id=? AND t.registration_revision=? AND t.status='active'
               AND t.lease_until>? AND (p.next_due_at IS NULL OR p.next_due_at<=?)",
        )
        .bind(&target.target_id)
        .bind(i64::try_from(target.registration_revision).map_err(|_| StoreError::Corrupt)?)
        .bind(now)
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await?;
        if eligible.is_none() {
            transaction.rollback().await?;
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO metric_poll_state(target_id,last_attempt_at,next_due_at,last_complete_at)
             VALUES(?,?,?,NULL) ON CONFLICT(target_id) DO UPDATE SET
             last_attempt_at=excluded.last_attempt_at,next_due_at=excluded.next_due_at",
        )
        .bind(&target.target_id)
        .bind(now)
        .bind(next_due)
        .execute(&mut *transaction)
        .await?;
        #[cfg(test)]
        wait_test_hook(&self.metric_reservation_hook);
        transaction.commit().await?;
        Ok(true)
    }

    pub(crate) async fn ready(&self) -> bool {
        let sentinel_valid = async {
            let metadata = sqlx::query(
                "SELECT key_id, key_sentinel_nonce, key_sentinel_ciphertext
                 FROM service_metadata WHERE singleton = 1",
            )
            .fetch_one(&self.pool)
            .await
            .ok()?;
            let key_id: String = metadata.get("key_id");
            if key_id != self.key_id {
                return None;
            }
            self.cipher
                .decrypt(
                    &metadata.get::<Vec<u8>, _>("key_sentinel_nonce"),
                    &metadata.get::<Vec<u8>, _>("key_sentinel_ciphertext"),
                    key_sentinel_aad(&self.trust_profile, &self.key_id).as_bytes(),
                )
                .ok()
                .filter(|plaintext| plaintext == KEY_SENTINEL)
        }
        .await
        .is_some();
        if !sentinel_valid {
            return false;
        }
        if crate::data_root_lock::verify_sqlite_files(&self.data_dir).is_err() {
            return false;
        }
        let Ok(invalid_statuses) = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM targets WHERE status NOT IN ('active', 'quarantined')",
        )
        .fetch_one(&self.pool)
        .await
        else {
            return false;
        };
        if invalid_statuses != 0 {
            return false;
        }
        let Ok(mut transaction) = self.pool.begin().await else {
            return false;
        };
        sqlx::query("UPDATE service_metadata SET trust_profile = trust_profile WHERE singleton = 1")
            .execute(&mut *transaction)
            .await
            .is_ok()
            && transaction.rollback().await.is_ok()
    }

    /// Return bounded, non-secret snapshots of active targets.
    pub(crate) async fn active_collection_targets(
        &self,
        now: i64,
    ) -> Result<Vec<CollectionTarget>, StoreError> {
        let rows = sqlx::query(
            "SELECT target_id, registration_revision FROM targets
             WHERE status = 'active' AND lease_until > ? ORDER BY target_id LIMIT ?",
        )
        .bind(now)
        .bind(MAX_TARGETS)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(CollectionTarget {
                    target_id: row.get("target_id"),
                    registration_revision: u64::try_from(
                        row.get::<i64, _>("registration_revision"),
                    )
                    .map_err(|_| StoreError::Corrupt)?,
                })
            })
            .collect()
    }

    /// Recheck a target fence and decrypt its work material at actual work start.
    pub(crate) async fn begin_collection_work(
        &self,
        target: &CollectionTarget,
        now: i64,
    ) -> Result<Option<WorkTarget>, StoreError> {
        let row = sqlx::query(
            "SELECT fman_pubkey, fman_name, key_id, generation, auth_created_at,
                    secret_nonce, secret_ciphertext
             FROM targets WHERE target_id = ? AND registration_revision = ?
             AND status = 'active' AND lease_until > ?",
        )
        .bind(&target.target_id)
        .bind(i64::try_from(target.registration_revision).map_err(|_| StoreError::Corrupt)?)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let fman_pubkey: String = row.get("fman_pubkey");
        let stored_key_id: String = row.get("key_id");
        if stored_key_id != self.key_id {
            return Err(StoreError::Secret);
        }
        let aad = target_secret_aad(
            &self.trust_profile,
            &stored_key_id,
            &fman_pubkey,
            &target.target_id,
            row.get("generation"),
            row.get("auth_created_at"),
        )?;
        let plaintext = self
            .cipher
            .decrypt(
                &row.get::<Vec<u8>, _>("secret_nonce"),
                &row.get::<Vec<u8>, _>("secret_ciphertext"),
                &aad,
            )
            .map_err(|_| StoreError::Secret)?;
        let secret: TargetSecret =
            serde_json::from_slice(&plaintext).map_err(|_| StoreError::Secret)?;
        if secret.format != TARGET_SECRET_FORMAT {
            return Err(StoreError::UnsupportedSecretFormat);
        }
        Ok(Some(WorkTarget::new(
            target.target_id.clone(),
            target.registration_revision,
            secret.endpoint_id,
            TelemetryCapability::from_bytes(secret.capability),
            fman_pubkey,
            row.get("fman_name"),
        )))
    }

    /// Atomically replace successful seat snapshots if the target fence is still current.
    pub(crate) async fn commit_metrics(
        &self,
        target: &WorkTarget,
        commit: MetricsCommit,
        now: i64,
    ) -> Result<CommitOutcome, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "DELETE FROM metric_snapshots WHERE target_id IN
             (SELECT target_id FROM targets WHERE status!='active' OR lease_until <= ?)",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        if !target_is_current(&mut transaction, target, now).await? {
            return Ok(CommitOutcome::Stale);
        }
        if commit.complete {
            sqlx::query("UPDATE metric_poll_state SET last_complete_at=? WHERE target_id=?")
                .bind(now)
                .bind(target.target_id())
                .execute(&mut *transaction)
                .await?;
        }
        if let Some(listed_seats) = &commit.listed_seats {
            let existing: Vec<String> = sqlx::query_scalar(
                "SELECT guardian_seat_id FROM metric_snapshots WHERE target_id = ?",
            )
            .bind(target.target_id())
            .fetch_all(&mut *transaction)
            .await?;
            for seat in existing {
                if !listed_seats.contains(&seat) {
                    sqlx::query(
                        "DELETE FROM metric_snapshots
                         WHERE target_id = ? AND guardian_seat_id = ?",
                    )
                    .bind(target.target_id())
                    .bind(seat)
                    .execute(&mut *transaction)
                    .await?;
                }
            }
        }
        for snapshot in commit.snapshots {
            let sample_count =
                i64::try_from(snapshot.samples.len()).map_err(|_| StoreError::Saturated)?;
            let samples = serde_json::to_vec(&snapshot.samples).map_err(|_| StoreError::Corrupt)?;
            if samples.len() > 4 * 1024 * 1024 {
                return Err(StoreError::Saturated);
            }
            sqlx::query(
                "INSERT INTO metric_snapshots(
                    target_id,guardian_seat_id,asserted_federation_id,observed_at_ms,samples_json,sample_count
                 ) VALUES (?,?,?,?,?,?)
                 ON CONFLICT(target_id,guardian_seat_id) DO UPDATE SET
                     asserted_federation_id=excluded.asserted_federation_id,
                     observed_at_ms=excluded.observed_at_ms,
                     samples_json=excluded.samples_json,sample_count=excluded.sample_count",
            )
            .bind(target.target_id())
            .bind(snapshot.guardian_seat_id)
            .bind(snapshot.asserted_federation_id)
            .bind(snapshot.observed_at_ms)
            .bind(samples)
            .bind(sample_count)
            .execute(&mut *transaction)
            .await?;
        }
        let (stored_bytes, stored_samples): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(length(samples_json)),0),COALESCE(SUM(sample_count),0)
             FROM metric_snapshots",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if stored_bytes > MAX_METRIC_STATE_BYTES || stored_samples > MAX_METRIC_STATE_SAMPLES {
            return Err(StoreError::Saturated);
        }
        sqlx::query("UPDATE metric_exposition_revision SET revision=revision+1 WHERE singleton=1")
            .execute(&mut *transaction)
            .await?;
        #[cfg(test)]
        wait_test_hook(&self.metric_commit_hook);
        transaction.commit().await?;
        Ok(CommitOutcome::Committed)
    }

    /// Read one lifecycle-consistent private metrics exposition view.
    pub(crate) async fn metric_exposition(
        &self,
        policy: &MetricsPolicy<'_>,
        now: i64,
        now_ms: i64,
        stale_after: i64,
    ) -> Result<MetricExpositionView, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let removed = sqlx::query(
            "DELETE FROM metric_snapshots WHERE target_id IN
             (SELECT target_id FROM targets WHERE status!='active' OR lease_until <= ?)",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if removed > 0 {
            sqlx::query(
                "UPDATE metric_exposition_revision SET revision=revision+1 WHERE singleton=1",
            )
            .execute(&mut *transaction)
            .await?;
        }
        let revision: i64 =
            sqlx::query_scalar("SELECT revision FROM metric_exposition_revision WHERE singleton=1")
                .fetch_one(&mut *transaction)
                .await?;
        let next_lease_expiry: Option<i64> = sqlx::query_scalar(
            "SELECT MIN(lease_until) FROM targets
             WHERE status='active' AND lease_until > ?",
        )
        .bind(now)
        .fetch_one(&mut *transaction)
        .await?;
        let version = MetricExpositionVersion {
            revision,
            next_lease_expiry,
        };
        let snapshot_rows = sqlx::query(
            "WITH active_targets AS (
                 SELECT target_id,fman_pubkey,fman_name FROM targets
                 WHERE status='active' AND lease_until > ?
                 ORDER BY fman_pubkey,target_id LIMIT ?
             ), candidates AS (
                  SELECT t.target_id,t.fman_pubkey,t.fman_name,s.guardian_seat_id,s.asserted_federation_id,
                         typeof(s.samples_json) AS samples_type,
                         length(CAST(s.samples_json AS BLOB)) AS stored_bytes
                   FROM metric_snapshots s JOIN active_targets t ON t.target_id=s.target_id
                 ORDER BY t.fman_pubkey,t.target_id,s.guardian_seat_id LIMIT ?
             ), evaluated AS (
                 SELECT *,
                        SUM(
                            CASE WHEN samples_type='blob' AND stored_bytes <= ? THEN stored_bytes
                                 ELSE 0 END
                        ) OVER (
                            ORDER BY fman_pubkey,target_id,guardian_seat_id
                        ) AS cumulative_bytes
                 FROM candidates
             ), accepted AS (
                  SELECT target_id,fman_pubkey,fman_name,guardian_seat_id,asserted_federation_id
                 FROM evaluated
                 WHERE samples_type='blob' AND stored_bytes <= ? AND cumulative_bytes <= ?
             ), summary AS (
                 SELECT COUNT(*) AS rejected_resources FROM evaluated
                 WHERE samples_type!='blob' OR stored_bytes > ? OR cumulative_bytes > ?
             )
            SELECT summary.rejected_resources,accepted.target_id IS NULL AS no_accepted,
                     accepted.fman_pubkey,accepted.fman_name,accepted.guardian_seat_id,
                     accepted.asserted_federation_id,
                    s.observed_at_ms,s.samples_json
             FROM summary LEFT JOIN accepted ON true
             LEFT JOIN metric_snapshots s
                 ON s.target_id=accepted.target_id
                AND s.guardian_seat_id=accepted.guardian_seat_id
             ORDER BY accepted.fman_pubkey,accepted.target_id,accepted.guardian_seat_id",
        )
        .bind(now)
        .bind(MAX_TARGETS)
        .bind(MAX_METRIC_SNAPSHOT_ROWS)
        .bind(4 * 1024 * 1024)
        .bind(4 * 1024 * 1024)
        .bind(MAX_METRIC_STATE_BYTES)
        .bind(4 * 1024 * 1024)
        .bind(MAX_METRIC_STATE_BYTES)
        .fetch_all(&mut *transaction)
        .await?;
        #[cfg(test)]
        wait_test_hook(&self.metric_exposition_hook);
        let cutoff = now.saturating_sub(stale_after);
        let target_rows = sqlx::query(
            "SELECT t.fman_pubkey,t.fman_name,p.last_complete_at
             FROM targets t LEFT JOIN metric_poll_state p ON p.target_id=t.target_id
             WHERE t.status='active' AND t.lease_until > ? ORDER BY t.fman_pubkey",
        )
        .bind(now)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;

        let mut snapshots = Vec::new();
        let mut rejected = snapshot_rows
            .first()
            .and_then(|row| row.try_get::<i64, _>("rejected_resources").ok())
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(usize::MAX);
        let mut sample_count = 0i64;
        let mut sample_bytes = 0i64;
        for row in snapshot_rows {
            if row.try_get::<i64, _>("no_accepted").unwrap_or(0) != 0 {
                continue;
            }
            let loaded = (|| {
                Ok::<_, sqlx::Error>((
                    row.try_get::<Option<String>, _>("fman_pubkey")?,
                    row.try_get::<Option<String>, _>("fman_name")?,
                    row.try_get::<Option<String>, _>("guardian_seat_id")?,
                    row.try_get::<Option<String>, _>("asserted_federation_id")?,
                    row.try_get::<Option<i64>, _>("observed_at_ms")?,
                    row.try_get::<Option<Vec<u8>>, _>("samples_json")?,
                ))
            })();
            let Ok((
                Some(fman_id),
                Some(fman_name),
                Some(guardian_seat_id),
                Some(asserted_federation_id),
                Some(observed_at_ms),
                Some(samples_json),
            )) = loaded
            else {
                rejected = rejected.saturating_add(1);
                continue;
            };
            if observed_at_ms < 0 || observed_at_ms > now_ms {
                rejected = rejected.saturating_add(1);
                continue;
            }
            if !canonical_asserted_federation_id(&asserted_federation_id) {
                rejected = rejected.saturating_add(1);
                continue;
            }
            let samples: Vec<String> = match serde_json::from_slice(&samples_json) {
                Ok(samples) => samples,
                Err(_) => {
                    rejected = rejected.saturating_add(1);
                    continue;
                }
            };
            if policy
                .revalidate_persisted(
                    &samples,
                    MetricsIdentity {
                        fman_id: &fman_id,
                        fman_name: &fman_name,
                        guardian_seat_id: &guardian_seat_id,
                        asserted_federation_id: &asserted_federation_id,
                    },
                )
                .is_err()
            {
                rejected = rejected.saturating_add(1);
                continue;
            }
            let row_samples = match i64::try_from(samples.len()) {
                Ok(count) => count,
                Err(_) => {
                    rejected = rejected.saturating_add(1);
                    continue;
                }
            };
            let row_bytes = samples.iter().try_fold(0i64, |total, sample| {
                total
                    .checked_add(i64::try_from(sample.len()).ok()?)?
                    .checked_add(1)
            });
            let Some(row_bytes) = row_bytes else {
                rejected = rejected.saturating_add(1);
                continue;
            };
            let Some(next_samples) = sample_count.checked_add(row_samples) else {
                rejected = rejected.saturating_add(1);
                continue;
            };
            let Some(next_bytes) = sample_bytes.checked_add(row_bytes) else {
                rejected = rejected.saturating_add(1);
                continue;
            };
            if next_samples > MAX_METRIC_STATE_SAMPLES || next_bytes > MAX_METRIC_STATE_BYTES {
                rejected = rejected.saturating_add(1);
                continue;
            }
            sample_count = next_samples;
            sample_bytes = next_bytes;
            snapshots.push(MetricsSnapshot {
                fman_id,
                fman_name,
                guardian_seat_id,
                asserted_federation_id,
                observed_at_ms,
                samples,
            });
        }
        let snapshots = LoadedMetricSnapshots {
            snapshots,
            rejected,
        };
        let targets = target_rows
            .into_iter()
            .map(|row| {
                let completed: Option<i64> = row.get("last_complete_at");
                MetricsTargetHealth {
                    fman_id: row.get("fman_pubkey"),
                    fman_name: row.get("fman_name"),
                    fresh: completed.is_some_and(|value| value >= cutoff),
                }
            })
            .collect();
        Ok(MetricExpositionView {
            version,
            snapshots,
            targets,
        })
    }

    /// Load and revalidate bounded snapshots before a private scrape can expose them.
    #[cfg(test)]
    pub(crate) async fn metric_snapshots(
        &self,
        policy: &MetricsPolicy<'_>,
        now_seconds: i64,
        now_ms: i64,
    ) -> Result<LoadedMetricSnapshots, StoreError> {
        sqlx::query(
            "DELETE FROM metric_snapshots WHERE target_id IN
             (SELECT target_id FROM targets WHERE status!='active' OR lease_until <= ?)",
        )
        .bind(now_seconds)
        .execute(&self.pool)
        .await?;
        let rows = sqlx::query(
            "WITH active_targets AS (
                 SELECT target_id,fman_pubkey,fman_name FROM targets
                 WHERE status='active' AND lease_until > ?
                 ORDER BY fman_pubkey,target_id LIMIT ?
             ), candidates AS (
                  SELECT t.target_id,t.fman_pubkey,t.fman_name,s.guardian_seat_id,s.asserted_federation_id,
                         typeof(s.samples_json) AS samples_type,
                         length(CAST(s.samples_json AS BLOB)) AS stored_bytes
                   FROM metric_snapshots s JOIN active_targets t ON t.target_id=s.target_id
                 ORDER BY t.fman_pubkey,t.target_id,s.guardian_seat_id LIMIT ?
             ), evaluated AS (
                 SELECT *,
                        SUM(
                            CASE WHEN samples_type='blob' AND stored_bytes <= ? THEN stored_bytes
                                 ELSE 0 END
                        ) OVER (
                            ORDER BY fman_pubkey,target_id,guardian_seat_id
                        ) AS cumulative_bytes
                 FROM candidates
             ), accepted AS (
                  SELECT target_id,fman_pubkey,fman_name,guardian_seat_id,asserted_federation_id
                 FROM evaluated
                 WHERE samples_type='blob' AND stored_bytes <= ? AND cumulative_bytes <= ?
             ), summary AS (
                 SELECT COUNT(*) AS rejected_resources FROM evaluated
                 WHERE samples_type!='blob' OR stored_bytes > ? OR cumulative_bytes > ?
             )
            SELECT summary.rejected_resources,accepted.target_id IS NULL AS no_accepted,
                     accepted.fman_pubkey,accepted.fman_name,accepted.guardian_seat_id,
                     accepted.asserted_federation_id,
                    s.observed_at_ms,s.samples_json
             FROM summary LEFT JOIN accepted ON true
             LEFT JOIN metric_snapshots s
                 ON s.target_id=accepted.target_id
                AND s.guardian_seat_id=accepted.guardian_seat_id
             ORDER BY accepted.fman_pubkey,accepted.target_id,accepted.guardian_seat_id",
        )
        .bind(now_seconds)
        .bind(MAX_TARGETS)
        .bind(MAX_METRIC_SNAPSHOT_ROWS)
        .bind(4 * 1024 * 1024)
        .bind(4 * 1024 * 1024)
        .bind(MAX_METRIC_STATE_BYTES)
        .bind(4 * 1024 * 1024)
        .bind(MAX_METRIC_STATE_BYTES)
        .fetch_all(&self.pool)
        .await?;
        let mut snapshots = Vec::new();
        let mut rejected = rows
            .first()
            .and_then(|row| row.try_get::<i64, _>("rejected_resources").ok())
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(usize::MAX);
        let mut sample_count = 0i64;
        let mut sample_bytes = 0i64;
        for row in rows {
            if row.try_get::<i64, _>("no_accepted").unwrap_or(0) != 0 {
                continue;
            }
            let loaded = (|| {
                Ok::<_, sqlx::Error>((
                    row.try_get::<Option<String>, _>("fman_pubkey")?,
                    row.try_get::<Option<String>, _>("fman_name")?,
                    row.try_get::<Option<String>, _>("guardian_seat_id")?,
                    row.try_get::<Option<String>, _>("asserted_federation_id")?,
                    row.try_get::<Option<i64>, _>("observed_at_ms")?,
                    row.try_get::<Option<Vec<u8>>, _>("samples_json")?,
                ))
            })();
            let Ok((
                Some(fman_id),
                Some(fman_name),
                Some(guardian_seat_id),
                Some(asserted_federation_id),
                Some(observed_at_ms),
                Some(samples_json),
            )) = loaded
            else {
                rejected = rejected.saturating_add(1);
                continue;
            };
            if observed_at_ms < 0 || observed_at_ms > now_ms {
                rejected = rejected.saturating_add(1);
                continue;
            }
            if !canonical_asserted_federation_id(&asserted_federation_id) {
                rejected = rejected.saturating_add(1);
                continue;
            }
            let samples: Vec<String> = match serde_json::from_slice(&samples_json) {
                Ok(samples) => samples,
                Err(_) => {
                    rejected = rejected.saturating_add(1);
                    continue;
                }
            };
            if policy
                .revalidate_persisted(
                    &samples,
                    MetricsIdentity {
                        fman_id: &fman_id,
                        fman_name: &fman_name,
                        guardian_seat_id: &guardian_seat_id,
                        asserted_federation_id: &asserted_federation_id,
                    },
                )
                .is_err()
            {
                rejected = rejected.saturating_add(1);
                continue;
            }
            let row_samples = match i64::try_from(samples.len()) {
                Ok(count) => count,
                Err(_) => {
                    rejected = rejected.saturating_add(1);
                    continue;
                }
            };
            let row_bytes = samples.iter().try_fold(0i64, |total, sample| {
                total
                    .checked_add(i64::try_from(sample.len()).ok()?)?
                    .checked_add(1)
            });
            let Some(row_bytes) = row_bytes else {
                rejected = rejected.saturating_add(1);
                continue;
            };
            let Some(next_samples) = sample_count.checked_add(row_samples) else {
                rejected = rejected.saturating_add(1);
                continue;
            };
            let Some(next_bytes) = sample_bytes.checked_add(row_bytes) else {
                rejected = rejected.saturating_add(1);
                continue;
            };
            if next_samples > MAX_METRIC_STATE_SAMPLES || next_bytes > MAX_METRIC_STATE_BYTES {
                rejected = rejected.saturating_add(1);
                continue;
            }
            sample_count = next_samples;
            sample_bytes = next_bytes;
            snapshots.push(MetricsSnapshot {
                fman_id,
                fman_name,
                guardian_seat_id,
                asserted_federation_id,
                observed_at_ms,
                samples,
            });
        }
        Ok(LoadedMetricSnapshots {
            snapshots,
            rejected,
        })
    }

    /// Report remote freshness without coupling it to process readiness.
    #[cfg(test)]
    pub(crate) async fn metric_target_health(
        &self,
        now: i64,
        stale_after: i64,
    ) -> Result<Vec<MetricsTargetHealth>, StoreError> {
        let cutoff = now.saturating_sub(stale_after);
        let rows = sqlx::query(
            "SELECT t.fman_pubkey,t.fman_name,p.last_complete_at
             FROM targets t LEFT JOIN metric_poll_state p ON p.target_id=t.target_id
             WHERE t.status='active' AND t.lease_until > ? ORDER BY t.fman_pubkey",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let completed: Option<i64> = row.get("last_complete_at");
                MetricsTargetHealth {
                    fman_id: row.get("fman_pubkey"),
                    fman_name: row.get("fman_name"),
                    fresh: completed.is_some_and(|value| value >= cutoff),
                }
            })
            .collect())
    }

    /// Version of the immutable cached private exposition.
    pub(crate) async fn metric_exposition_version(
        &self,
        now: i64,
    ) -> Result<MetricExpositionVersion, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let removed = sqlx::query(
            "DELETE FROM metric_snapshots WHERE target_id IN
             (SELECT target_id FROM targets WHERE status!='active' OR lease_until <= ?)",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if removed > 0 {
            sqlx::query(
                "UPDATE metric_exposition_revision SET revision=revision+1 WHERE singleton=1",
            )
            .execute(&mut *transaction)
            .await?;
        }
        let revision: i64 =
            sqlx::query_scalar("SELECT revision FROM metric_exposition_revision WHERE singleton=1")
                .fetch_one(&mut *transaction)
                .await?;
        let next_lease_expiry: Option<i64> = sqlx::query_scalar(
            "SELECT MIN(lease_until) FROM targets
             WHERE status='active' AND lease_until > ?",
        )
        .bind(now)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(MetricExpositionVersion {
            revision,
            next_lease_expiry,
        })
    }

    /// Open a stream and durably record a listed incarnation discontinuity.
    pub(crate) async fn open_journal_stream(
        &self,
        target: &WorkTarget,
        journal: &SafeEventJournal,
        listed_incarnation: &SafeEventJournalIncarnation,
        now: i64,
    ) -> Result<Option<JournalStreamState>, StoreError> {
        let selector = serde_json::to_vec(journal).map_err(|_| StoreError::Corrupt)?;
        if selector.len() > 512 {
            return Err(StoreError::Corrupt);
        }
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if !target_is_current(&mut transaction, target, now).await? {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT stream_id, source_incarnation, cursor_segment, cursor_offset,
                    observed_generation
             FROM journal_streams WHERE target_id = ? AND journal_selector = ?",
        )
        .bind(target.target_id())
        .bind(&selector)
        .fetch_optional(&mut *transaction)
        .await?;
        let state = if let Some(row) = row {
            let stream_id =
                JournalStreamId::parse(row.get("stream_id")).map_err(|_| StoreError::Corrupt)?;
            let stored: Option<Vec<u8>> = row.get("source_incarnation");
            let mut generation = u64::try_from(row.get::<i64, _>("observed_generation"))
                .map_err(|_| StoreError::Corrupt)?;
            let incarnation = match stored {
                Some(value) => {
                    let value = String::from_utf8(value).map_err(|_| StoreError::Corrupt)?;
                    value.parse().map_err(|_| StoreError::Corrupt)?
                }
                None => listed_incarnation.clone(),
            };
            if incarnation != *listed_incarnation {
                generation = generation.checked_add(1).ok_or(StoreError::Corrupt)?;
                sqlx::query(
                    "UPDATE journal_streams SET source_incarnation = ?,
                        cursor_segment = NULL, cursor_offset = NULL,
                        observed_generation = ?, gap_count = gap_count + 1, status = 'active'
                     WHERE stream_id = ?",
                )
                .bind(listed_incarnation.as_str().as_bytes())
                .bind(i64::try_from(generation).map_err(|_| StoreError::Corrupt)?)
                .bind(stream_id.as_str())
                .execute(&mut *transaction)
                .await?;
                JournalStreamState {
                    stream_id,
                    journal: journal.clone(),
                    incarnation: listed_incarnation.clone(),
                    cursor: None,
                    observed_generation: generation,
                }
            } else {
                let cursor = cursor_from_row(&row, &incarnation)?;
                JournalStreamState {
                    stream_id,
                    journal: journal.clone(),
                    incarnation,
                    cursor,
                    observed_generation: generation,
                }
            }
        } else {
            if sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM journal_streams WHERE target_id = ?",
            )
            .bind(target.target_id())
            .fetch_one(&mut *transaction)
            .await?
                >= MAX_JOURNAL_STREAMS_PER_TARGET
            {
                return Err(StoreError::Saturated);
            }
            let stream_id = JournalStreamId::parse(random_id()).map_err(|_| StoreError::Corrupt)?;
            sqlx::query(
                "INSERT INTO journal_streams(
                    stream_id,target_id,journal_selector,source_incarnation,
                    observed_generation,status
                 ) VALUES(?,?,?,?,0,'active')",
            )
            .bind(stream_id.as_str())
            .bind(target.target_id())
            .bind(selector)
            .bind(listed_incarnation.as_str().as_bytes())
            .execute(&mut *transaction)
            .await?;
            JournalStreamState {
                stream_id,
                journal: journal.clone(),
                incarnation: listed_incarnation.clone(),
                cursor: None,
                observed_generation: 0,
            }
        };
        transaction.commit().await?;
        Ok(Some(state))
    }

    /// Commit an archive frame and returned source state under one revision/cursor CAS.
    pub(crate) async fn commit_journal_batch(
        &self,
        target: &WorkTarget,
        expected: &JournalStreamState,
        batch: &ValidatedJournalBatch,
        frame: Option<&ArchiveFrame>,
        now: i64,
    ) -> Result<CommitOutcome, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if !target_is_current(&mut transaction, target, now).await? {
            return Ok(CommitOutcome::Stale);
        }
        let row = sqlx::query(
            "SELECT source_incarnation,cursor_segment,cursor_offset,observed_generation
             FROM journal_streams WHERE stream_id = ? AND target_id = ?",
        )
        .bind(expected.stream_id.as_str())
        .bind(target.target_id())
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(row) = row else {
            return Ok(CommitOutcome::Stale);
        };
        let incarnation = String::from_utf8(row.get::<Vec<u8>, _>("source_incarnation"))
            .map_err(|_| StoreError::Corrupt)?;
        let current_cursor = cursor_from_row(&row, &expected.incarnation)?;
        if incarnation != expected.incarnation.as_str()
            || current_cursor != expected.cursor
            || u64::try_from(row.get::<i64, _>("observed_generation"))
                .map_err(|_| StoreError::Corrupt)?
                != expected.observed_generation
        {
            return Ok(CommitOutcome::Stale);
        }
        if batch
            .next_cursor()
            .is_some_and(|cursor| cursor.incarnation != expected.incarnation)
        {
            return Err(StoreError::Corrupt);
        }
        let generation = expected
            .observed_generation
            .checked_add(u64::from(batch.continuity_gap()))
            .ok_or(StoreError::Corrupt)?;
        let (segment, offset) = batch
            .next_cursor()
            .map(|cursor| {
                Ok::<_, StoreError>((
                    i64::try_from(cursor.segment).map_err(|_| StoreError::Corrupt)?,
                    i64::try_from(cursor.offset).map_err(|_| StoreError::Corrupt)?,
                ))
            })
            .transpose()?
            .unzip();
        if let Some(frame) = frame {
            sqlx::query(
                "INSERT INTO archive_frames(
                    stream_id,reception_day,start_offset,end_offset,frame_hash,
                    observed_generation,continuity_gap
                 ) VALUES(?,?,?,?,?,?,?)",
            )
            .bind(expected.stream_id.as_str())
            .bind(frame.day.as_str())
            .bind(i64::try_from(frame.start).map_err(|_| StoreError::Corrupt)?)
            .bind(i64::try_from(frame.end).map_err(|_| StoreError::Corrupt)?)
            .bind(frame.hash.as_slice())
            .bind(i64::try_from(generation).map_err(|_| StoreError::Corrupt)?)
            .bind(batch.continuity_gap())
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query(
            "UPDATE journal_streams SET cursor_segment = ?, cursor_offset = ?,
                observed_generation = ?, gap_count = gap_count + ?,
                archive_day = COALESCE(?, archive_day),
                archive_offset = COALESCE(?, archive_offset),
                archive_hash = COALESCE(?, archive_hash)
             WHERE stream_id = ?",
        )
        .bind(segment)
        .bind(offset)
        .bind(i64::try_from(generation).map_err(|_| StoreError::Corrupt)?)
        .bind(batch.continuity_gap())
        .bind(frame.map(|frame| frame.day.as_str()))
        .bind(
            frame
                .map(|frame| i64::try_from(frame.end))
                .transpose()
                .map_err(|_| StoreError::Corrupt)?,
        )
        .bind(frame.map(|frame| frame.hash.as_slice()))
        .bind(expected.stream_id.as_str())
        .execute(&mut *transaction)
        .await?;
        #[cfg(test)]
        if let Some(hook) = &self.commit_hook
            && !hook
                .entered_once
                .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            hook.entered.wait();
            hook.release.wait();
        }
        transaction.commit().await?;
        Ok(CommitOutcome::Committed)
    }

    /// Record a newly observed incarnation and reset its cursor without ordering UUIDs.
    pub(crate) async fn commit_incarnation_change(
        &self,
        target: &WorkTarget,
        expected: &JournalStreamState,
        incarnation: &SafeEventJournalIncarnation,
        now: i64,
    ) -> Result<CommitOutcome, StoreError> {
        if *incarnation == expected.incarnation {
            return Err(StoreError::Corrupt);
        }
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if !target_is_current(&mut transaction, target, now).await? {
            return Ok(CommitOutcome::Stale);
        }
        let updated = sqlx::query(
            "UPDATE journal_streams SET source_incarnation = ?,
                cursor_segment = NULL, cursor_offset = NULL,
                observed_generation = observed_generation + 1,
                gap_count = gap_count + 1
             WHERE stream_id = ? AND target_id = ? AND source_incarnation = ?
             AND observed_generation = ?",
        )
        .bind(incarnation.as_str().as_bytes())
        .bind(expected.stream_id.as_str())
        .bind(target.target_id())
        .bind(expected.incarnation.as_str().as_bytes())
        .bind(i64::try_from(expected.observed_generation).map_err(|_| StoreError::Corrupt)?)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Ok(CommitOutcome::Stale);
        }
        transaction.commit().await?;
        Ok(CommitOutcome::Committed)
    }

    /// Return the final committed frame for each stream/day archive.
    pub(crate) async fn final_frame_boundaries(&self) -> Result<Vec<FrameBoundary>, StoreError> {
        let rows = sqlx::query(
            "SELECT f.stream_id,f.reception_day,f.start_offset,f.end_offset,f.frame_hash
             FROM archive_frames f
             JOIN (
                SELECT stream_id,reception_day,MAX(end_offset) AS end_offset
                FROM archive_frames GROUP BY stream_id,reception_day
             ) last ON last.stream_id=f.stream_id
                 AND last.reception_day=f.reception_day AND last.end_offset=f.end_offset",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let hash: Vec<u8> = row.get("frame_hash");
                Ok(FrameBoundary {
                    stream_id: JournalStreamId::parse(row.get("stream_id"))
                        .map_err(|_| StoreError::Corrupt)?,
                    day: ReceptionDay::parse(row.get("reception_day"))
                        .map_err(|_| StoreError::Corrupt)?,
                    start: u64::try_from(row.get::<i64, _>("start_offset"))
                        .map_err(|_| StoreError::Corrupt)?,
                    end: u64::try_from(row.get::<i64, _>("end_offset"))
                        .map_err(|_| StoreError::Corrupt)?,
                    hash: hash.try_into().map_err(|_| StoreError::Corrupt)?,
                })
            })
            .collect()
    }

    /// Forget archive recovery metadata older than the configured retention day.
    pub(crate) async fn prune_archive_ledger(
        &self,
        cutoff_day: &ReceptionDay,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "UPDATE journal_streams SET archive_day=NULL,archive_offset=0,archive_hash=NULL
             WHERE archive_day < ?",
        )
        .bind(cutoff_day.as_str())
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM archive_frames WHERE reception_day < ?")
            .bind(cutoff_day.as_str())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }
}

async fn target_is_current(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    target: &WorkTarget,
    now: i64,
) -> Result<bool, StoreError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM targets WHERE target_id = ? AND registration_revision = ?
         AND status = 'active' AND lease_until > ?",
    )
    .bind(target.target_id())
    .bind(i64::try_from(target.registration_revision()).map_err(|_| StoreError::Corrupt)?)
    .bind(now)
    .fetch_one(&mut **transaction)
    .await?
        == 1)
}

fn cursor_from_row(
    row: &sqlx::sqlite::SqliteRow,
    incarnation: &SafeEventJournalIncarnation,
) -> Result<Option<SafeEventCursor>, StoreError> {
    let segment: Option<i64> = row.get("cursor_segment");
    let offset: Option<i64> = row.get("cursor_offset");
    match (segment, offset) {
        (None, None) => Ok(None),
        (Some(segment), Some(offset)) => Ok(Some(SafeEventCursor {
            incarnation: incarnation.clone(),
            segment: u64::try_from(segment).map_err(|_| StoreError::Corrupt)?,
            offset: u64::try_from(offset).map_err(|_| StoreError::Corrupt)?,
        })),
        _ => Err(StoreError::Corrupt),
    }
}

fn random_id() -> String {
    let mut bytes = [0; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Sanitized durable-admission failure.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StoreError {
    /// SQLite operation failed.
    #[error("collector state operation failed")]
    Database(#[from] sqlx::Error),
    /// Schema migration failed.
    #[error("collector state migration failed")]
    Migration(#[from] sqlx::migrate::MigrateError),
    /// Data root belongs to another trust environment.
    #[error("collector data root environment does not match configuration")]
    EnvironmentMismatch,
    /// Configured key cannot authenticate the persisted data root.
    #[error("collector data root encryption key does not match configuration")]
    KeyMismatch,
    /// Persisted target ciphertext uses an unsupported candidate format.
    #[error("collector target secret format is unsupported")]
    UnsupportedSecretFormat,
    /// Authorization event was already accepted.
    #[error("registration replay refused")]
    Replay,
    /// Durable replay admission reached its configured bound.
    #[error("registration admission is saturated")]
    Saturated,
    /// Registration violated a durable ordering rule.
    #[error("registration ordering refused")]
    Refused,
    /// Secret serialization or encryption failed.
    #[error("registration secret processing failed")]
    Secret,
    /// Persisted collector state violated its schema-level semantic invariants.
    #[error("collector state is inconsistent")]
    Corrupt,
    /// Persisted status was outside the closed supported set.
    #[cfg(test)]
    #[error("collector target status is invalid")]
    InvalidStatus,
    /// Data-root path or permissions are unsafe.
    #[error("collector data root is unsafe")]
    UnsafeDataRoot,
    /// Data-root filesystem operation failed.
    #[error("collector data root operation failed")]
    Filesystem(#[from] std::io::Error),
}

const MAX_TARGETS: i64 = 4096;
const MAX_JOURNAL_STREAMS_PER_TARGET: i64 = 32;
const KEY_SENTINEL: &[u8] = b"cloud-fman-telemetry-key-sentinel-v1";
const TARGET_SECRET_FORMAT: i64 = 2;

#[derive(Serialize, Deserialize)]
struct TargetSecret {
    format: i64,
    endpoint_id: String,
    capability: [u8; 32],
}

#[derive(Serialize)]
struct TargetSecretAad<'a> {
    format: i64,
    secret_kind: &'static str,
    trust_profile: &'a str,
    key_id: &'a str,
    fman_pubkey: &'a str,
    target_id: &'a str,
    generation: i64,
    auth_created_at: i64,
}

fn target_secret_aad(
    trust_profile: &str,
    key_id: &str,
    fman_pubkey: &str,
    target_id: &str,
    generation: i64,
    auth_created_at: i64,
) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(&TargetSecretAad {
        format: TARGET_SECRET_FORMAT,
        secret_kind: "registration-target",
        trust_profile,
        key_id,
        fman_pubkey,
        target_id,
        generation,
        auth_created_at,
    })
    .map_err(|_| StoreError::Secret)
}

async fn finish_immediate<T>(
    transaction: sqlx::Transaction<'_, sqlx::Sqlite>,
    result: Result<T, StoreError>,
) -> Result<T, StoreError> {
    match result {
        Ok(value) => {
            transaction.commit().await?;
            Ok(value)
        }
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

fn key_sentinel_aad(trust_profile: &str, key_id: &str) -> String {
    format!("cloud-fman-telemetry/key-sentinel/v1:{trust_profile}:{key_id}")
}

fn canonical_asserted_federation_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl StoreError {
    pub(crate) fn is_refusal(&self) -> bool {
        matches!(self, Self::Replay | Self::Refused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        metrics_policy::{MetricsIdentity, MetricsPolicy},
        metrics_types::SeatObservation,
    };
    use tempfile::TempDir;

    fn data_dir(dir: &TempDir) -> PathBuf {
        dir.path().join("collector")
    }

    async fn store(dir: &TempDir) -> Store {
        let data_dir = data_dir(dir);
        let _lock = crate::data_root_lock::DataRootLock::acquire(&data_dir).unwrap();
        Store::open(
            &data_dir.join("state.sqlite"),
            "development",
            SecretCipher::new(&[7; 32]),
            "test".into(),
            120,
        )
        .await
        .unwrap()
    }

    fn auth(id: &str, created_at: i64) -> VerifiedHttpAuth {
        VerifiedHttpAuth {
            signer: "11".repeat(32),
            event_id: id.into(),
            created_at,
        }
    }

    fn target(generation: u64) -> TargetMaterial<'static> {
        target_for("11", "endpoint-secret", generation)
    }

    fn target_for(
        fman_pubkey: &'static str,
        endpoint_id: &'static str,
        generation: u64,
    ) -> TargetMaterial<'static> {
        TargetMaterial {
            fman_pubkey,
            fman_name: "calm-tern",
            endpoint_id,
            capability: &[9; 32],
            generation,
        }
    }

    fn metrics_policy() -> MetricsPolicy<'static> {
        MetricsPolicy {
            version: "test-version",
            version_hash: "test-hash",
            canonical_method_labels: false,
        }
    }

    fn stored_samples(seat: &str, value: u64) -> Vec<String> {
        metrics_policy()
            .admit_until(
                format!(
                    "fm_app_start_ts{{version=\"test-version\",version_hash=\"test-hash\"}} 1\n\
                     fm_consensus_session_count {value}"
                )
                .as_bytes(),
                MetricsIdentity {
                    fman_id: "11",
                    fman_name: "calm-tern",
                    guardian_seat_id: seat,
                    asserted_federation_id: "0000000000000000000000000000000000000000000000000000000000000000",
                },
                None,
            )
            .unwrap()
            .samples
    }

    fn stored_peer_samples(seat: &str) -> Vec<String> {
        let mut body =
            "fm_app_start_ts{version=\"test-version\",version_hash=\"test-hash\"} 1\n".to_owned();
        for peer_id in 0..9_999 {
            body.push_str(&format!(
                "fm_consensus_items_processed_total{{peer_id=\"{peer_id}\"}} 1\n"
            ));
        }
        metrics_policy()
            .admit_until(
                body.as_bytes(),
                MetricsIdentity {
                    fman_id: "11",
                    fman_name: "calm-tern",
                    guardian_seat_id: seat,
                    asserted_federation_id: "0000000000000000000000000000000000000000000000000000000000000000",
                },
                None,
            )
            .unwrap()
            .samples
    }

    async fn insert_snapshot(store: &Store, seat: &str, observed_at_ms: i64, samples: &[String]) {
        let target_id: String =
            sqlx::query_scalar("SELECT target_id FROM targets WHERE fman_pubkey='11'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO metric_snapshots(
                 target_id,guardian_seat_id,asserted_federation_id,observed_at_ms,samples_json,sample_count
             ) VALUES (?,?,?,?,?,?)",
        )
        .bind(target_id)
        .bind(seat)
        .bind("0000000000000000000000000000000000000000000000000000000000000000")
        .bind(observed_at_ms)
        .bind(serde_json::to_vec(samples).unwrap())
        .bind(i64::try_from(samples.len()).unwrap())
        .execute(&store.pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn ordering_replay_expiry_and_restart_are_durable() {
        let dir = tempfile::tempdir().unwrap();
        let first = store(&dir).await;
        first.reserve_auth(&auth("a", 100), 100).await.unwrap();
        assert_eq!(
            first.admit(&auth("a", 100), target(4), 100).await.unwrap(),
            AdmissionOutcome::Updated
        );
        assert!(matches!(
            first.reserve_auth(&auth("a", 100), 100).await,
            Err(StoreError::Replay)
        ));
        first.reserve_auth(&auth("b", 101), 101).await.unwrap();
        assert!(matches!(
            first.admit(&auth("b", 101), target(3), 101).await,
            Err(StoreError::Refused)
        ));
        first.reserve_auth(&auth("c", 101), 101).await.unwrap();
        assert_eq!(
            first.admit(&auth("c", 101), target(4), 101).await.unwrap(),
            AdmissionOutcome::Idempotent
        );
        let changed_same_generation = TargetMaterial {
            capability: &[8; 32],
            ..target(4)
        };
        first.reserve_auth(&auth("d", 102), 102).await.unwrap();
        assert!(matches!(
            first
                .admit(&auth("d", 102), changed_same_generation, 102)
                .await,
            Err(StoreError::Refused)
        ));
        first.reserve_auth(&auth("e", 103), 103).await.unwrap();
        let moved_endpoint = TargetMaterial {
            endpoint_id: "replacement-endpoint",
            ..target(4)
        };
        assert_eq!(
            first
                .admit(&auth("e", 103), moved_endpoint, 103)
                .await
                .unwrap(),
            AdmissionOutcome::Updated
        );
        assert!(first.quarantine("11").await.unwrap());
        first.reserve_auth(&auth("f", 104), 104).await.unwrap();
        assert_eq!(
            first.admit(&auth("f", 104), target(4), 104).await.unwrap(),
            AdmissionOutcome::Updated
        );
        assert_eq!(
            first.target_status("11", 104).await.unwrap(),
            Some(TargetStatus::Quarantined)
        );
        assert!(first.reactivate("11").await.unwrap());
        drop(first);

        let restarted = store(&dir).await;
        assert_eq!(
            restarted.target_status("11", 200).await.unwrap(),
            Some(TargetStatus::Active)
        );
        assert_eq!(
            restarted.target_status("11", 225).await.unwrap(),
            Some(TargetStatus::Expired)
        );
        let database = std::fs::read(data_dir(&dir).join("state.sqlite")).unwrap();
        assert!(
            !database
                .windows(b"endpoint-secret".len())
                .any(|window| window == b"endpoint-secret")
        );
        sqlx::query("DELETE FROM service_metadata")
            .execute(&restarted.pool)
            .await
            .unwrap();
        drop(restarted);
        assert!(matches!(
            Store::open(
                &data_dir(&dir).join("state.sqlite"),
                "development",
                SecretCipher::new(&[8; 32]),
                "replacement".into(),
                120,
            )
            .await,
            Err(StoreError::KeyMismatch)
        ));
    }

    #[tokio::test]
    async fn data_root_is_environment_bound_and_path_is_not_uri_decoded() {
        let outer = tempfile::tempdir().unwrap();
        let directory = outer.path().join("literal%2Fname");
        std::fs::create_dir(&directory).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(
            &directory,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
        )
        .unwrap();
        let path = directory.join("state.sqlite");
        let first = Store::open(
            &path,
            "development",
            SecretCipher::new(&[7; 32]),
            "test".into(),
            120,
        )
        .await
        .unwrap();
        assert!(path.exists());
        drop(first);
        assert!(matches!(
            Store::open(
                &path,
                "production",
                SecretCipher::new(&[7; 32]),
                "test".into(),
                120,
            )
            .await,
            Err(StoreError::EnvironmentMismatch)
        ));
        assert!(matches!(
            Store::open(
                &path,
                "development",
                SecretCipher::new(&[8; 32]),
                "test".into(),
                120,
            )
            .await,
            Err(StoreError::KeyMismatch)
        ));
    }

    #[tokio::test]
    async fn concurrent_admissions_acquire_write_reservation_before_reading() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let auth_one = auth("one", 100);
        let auth_two = auth("two", 100);
        store.reserve_auth(&auth_one, 100).await.unwrap();
        store.reserve_auth(&auth_two, 100).await.unwrap();
        let (one, two) = tokio::join!(
            store.admit(&auth_one, target(1), 100),
            store.admit(&auth_two, target(1), 100),
        );
        assert!(one.is_ok(), "{one:?}");
        assert!(two.is_ok(), "{two:?}");

        let auth_three = auth("three", 101);
        let auth_four = auth("four", 101);
        store.reserve_auth(&auth_three, 101).await.unwrap();
        store.reserve_auth(&auth_four, 101).await.unwrap();
        let (three, four) = tokio::join!(
            store.admit(&auth_three, target_for("22", "endpoint-two", 1), 101),
            store.admit(&auth_four, target_for("33", "endpoint-three", 1), 101),
        );
        assert!(three.is_ok(), "{three:?}");
        assert!(four.is_ok(), "{four:?}");
    }

    #[tokio::test]
    async fn cancelled_immediate_transaction_does_not_poison_pool() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let transaction = store.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        drop(transaction);
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            store.reserve_auth(&auth("after-cancel", 100), 100),
        )
        .await
        .expect("pool connection was returned without a live write transaction")
        .unwrap();
    }

    #[tokio::test]
    async fn target_ciphertext_rejects_bound_metadata_and_deployment_transplants() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        store.reserve_auth(&auth("one", 100), 100).await.unwrap();
        store
            .admit(&auth("one", 100), target(1), 100)
            .await
            .unwrap();

        store.reserve_auth(&auth("other", 100), 100).await.unwrap();
        store
            .admit(
                &auth("other", 100),
                target_for("22", "other-endpoint", 1),
                100,
            )
            .await
            .unwrap();
        sqlx::query(
            "UPDATE targets
             SET secret_nonce = (SELECT secret_nonce FROM targets WHERE fman_pubkey = '11'),
                 secret_ciphertext = (SELECT secret_ciphertext FROM targets WHERE fman_pubkey = '11')
             WHERE fman_pubkey = '22'",
        )
        .execute(&store.pool)
        .await
        .unwrap();
        store
            .reserve_auth(&auth("other-retry", 101), 101)
            .await
            .unwrap();
        assert!(matches!(
            store
                .admit(
                    &auth("other-retry", 101),
                    target_for("22", "other-endpoint", 1),
                    101,
                )
                .await,
            Err(StoreError::Secret)
        ));

        sqlx::query("UPDATE targets SET generation = 2 WHERE fman_pubkey = '11'")
            .execute(&store.pool)
            .await
            .unwrap();
        store.reserve_auth(&auth("two", 101), 101).await.unwrap();
        assert!(matches!(
            store.admit(&auth("two", 101), target(2), 101).await,
            Err(StoreError::Secret)
        ));

        sqlx::query("UPDATE targets SET generation = 1, key_id = 'other' WHERE fman_pubkey = '11'")
            .execute(&store.pool)
            .await
            .unwrap();
        store.reserve_auth(&auth("three", 102), 102).await.unwrap();
        assert!(matches!(
            store.admit(&auth("three", 102), target(1), 102).await,
            Err(StoreError::Secret)
        ));

        sqlx::query("UPDATE targets SET key_id = 'test' WHERE fman_pubkey = '11'")
            .execute(&store.pool)
            .await
            .unwrap();
        let other_profile = Store {
            trust_profile: "production".into(),
            ..store.clone()
        };
        store.reserve_auth(&auth("four", 103), 103).await.unwrap();
        assert!(matches!(
            other_profile
                .admit(&auth("four", 103), target(1), 103)
                .await,
            Err(StoreError::Secret)
        ));
    }

    #[tokio::test]
    async fn unsupported_candidate_format_is_rejected_at_startup() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let mut connection = store.pool.acquire().await.unwrap();
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("UPDATE service_metadata SET target_secret_format = 1")
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);
        drop(store);
        assert!(matches!(
            Store::open(
                &data_dir(&dir).join("state.sqlite"),
                "development",
                SecretCipher::new(&[7; 32]),
                "test".into(),
                120,
            )
            .await,
            Err(StoreError::UnsupportedSecretFormat)
        ));
    }

    #[tokio::test]
    async fn unknown_status_fails_closed_in_queries_and_readiness() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        store.reserve_auth(&auth("one", 100), 100).await.unwrap();
        store
            .admit(&auth("one", 100), target(1), 100)
            .await
            .unwrap();
        let mut connection = store.pool.acquire().await.unwrap();
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("UPDATE targets SET status = 'unknown' WHERE fman_pubkey = '11'")
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);
        assert!(matches!(
            store.target_status("11", 100).await,
            Err(StoreError::InvalidStatus)
        ));
        assert!(!store.ready().await);
    }

    #[tokio::test]
    async fn readiness_fails_when_state_file_mode_drifts() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        std::fs::set_permissions(
            data_dir(&dir).join("state.sqlite"),
            std::fs::Permissions::from_mode(0o400),
        )
        .unwrap();
        assert!(!store.ready().await);
    }

    #[tokio::test]
    async fn metric_snapshots_survive_restart_and_obey_revision_and_seat_cas() {
        let directory = tempfile::tempdir().unwrap();
        let first = store(&directory).await;
        first
            .reserve_auth(&auth("metrics-a", 100), 100)
            .await
            .unwrap();
        first
            .admit(&auth("metrics-a", 100), target(1), 100)
            .await
            .unwrap();
        let scheduled = first
            .active_collection_targets(100)
            .await
            .unwrap()
            .remove(0);
        let stale_work = first
            .begin_metric_work(&scheduled, 100, 1_800)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            first
                .commit_metrics(
                    &stale_work,
                    MetricsCommit {
                        listed_seats: Some(["aa".to_owned()].into()),
                        snapshots: vec![SeatObservation {
                            guardian_seat_id: "aa".into(),
                            asserted_federation_id:
                                "0000000000000000000000000000000000000000000000000000000000000000"
                                    .to_owned(),
                            observed_at_ms: 100_000,
                            samples: stored_samples("aa", 1),
                        }],
                        complete: true,
                    },
                    100,
                )
                .await
                .unwrap(),
            CommitOutcome::Committed
        );
        drop(first);

        let restarted = store(&directory).await;
        assert_eq!(
            restarted
                .metric_snapshots(&metrics_policy(), 100, 100_000)
                .await
                .unwrap()
                .snapshots[0]
                .observed_at_ms,
            100_000
        );
        assert!(
            restarted.metric_target_health(200, 100).await.unwrap()[0].fresh,
            "a completion exactly two intervals old remains within the freshness window"
        );
        restarted
            .reserve_auth(&auth("metrics-b", 101), 101)
            .await
            .unwrap();
        restarted
            .admit(
                &auth("metrics-b", 101),
                TargetMaterial {
                    endpoint_id: "replacement-endpoint",
                    ..target(1)
                },
                101,
            )
            .await
            .unwrap();
        assert_eq!(
            restarted
                .commit_metrics(
                    &stale_work,
                    MetricsCommit {
                        listed_seats: Some(Default::default()),
                        snapshots: Vec::new(),
                        complete: true,
                    },
                    101,
                )
                .await
                .unwrap(),
            CommitOutcome::Stale
        );
        assert_eq!(
            restarted
                .metric_snapshots(&metrics_policy(), 101, 101_000)
                .await
                .unwrap()
                .snapshots
                .len(),
            1
        );

        let scheduled = restarted
            .active_collection_targets(101)
            .await
            .unwrap()
            .remove(0);
        let current = restarted
            .begin_collection_work(&scheduled, 101)
            .await
            .unwrap()
            .unwrap();
        restarted
            .commit_metrics(
                &current,
                MetricsCommit {
                    listed_seats: Some(["aa".to_owned(), "bb".to_owned()].into()),
                    snapshots: vec![SeatObservation {
                        guardian_seat_id: "bb".into(),
                        asserted_federation_id:
                            "0000000000000000000000000000000000000000000000000000000000000000"
                                .to_owned(),
                        observed_at_ms: 101_000,
                        samples: stored_samples("bb", 2),
                    }],
                    complete: false,
                },
                101,
            )
            .await
            .unwrap();
        assert_eq!(
            restarted
                .metric_snapshots(&metrics_policy(), 101, 101_000)
                .await
                .unwrap()
                .snapshots
                .len(),
            2
        );
        restarted
            .commit_metrics(
                &current,
                MetricsCommit {
                    listed_seats: Some(["bb".to_owned()].into()),
                    snapshots: Vec::new(),
                    complete: true,
                },
                202,
            )
            .await
            .unwrap();
        let snapshots = restarted
            .metric_snapshots(&metrics_policy(), 202, 202_000)
            .await
            .unwrap()
            .snapshots;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].guardian_seat_id, "bb");
        assert!(restarted.due_metric_targets(222).await.unwrap().is_empty());
        let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metric_snapshots")
            .fetch_one(&restarted.pool)
            .await
            .unwrap();
        assert_eq!(retained, 0);
        restarted
            .reserve_auth(&auth("metrics-renew", 222), 222)
            .await
            .unwrap();
        restarted
            .admit(&auth("metrics-renew", 222), target(2), 222)
            .await
            .unwrap();
        assert!(
            restarted
                .metric_snapshots(&metrics_policy(), 222, 222_000)
                .await
                .unwrap()
                .snapshots
                .is_empty()
        );
        restarted
            .configure_metrics_policy("first-policy")
            .await
            .unwrap();
        // Establishing the first policy resets state collected without a fingerprint.
        assert!(
            restarted
                .metric_snapshots(&metrics_policy(), 202, 202_000)
                .await
                .unwrap()
                .snapshots
                .is_empty()
        );
    }

    #[tokio::test]
    async fn later_successful_poll_replaces_snapshot_federation_attribution() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory).await;
        store
            .reserve_auth(&auth("federation-attribution", 100), 100)
            .await
            .unwrap();
        store
            .admit(&auth("federation-attribution", 100), target(1), 100)
            .await
            .unwrap();
        let scheduled = store
            .active_collection_targets(100)
            .await
            .unwrap()
            .remove(0);
        let work = store
            .begin_metric_work(&scheduled, 100, 1_800)
            .await
            .unwrap()
            .unwrap();
        let commit = |asserted_federation_id: &str, observed_at_ms| MetricsCommit {
            listed_seats: Some(["aa".to_owned()].into()),
            snapshots: vec![SeatObservation {
                guardian_seat_id: "aa".to_owned(),
                asserted_federation_id: asserted_federation_id.to_owned(),
                observed_at_ms,
                samples: stored_samples("aa", 1)
                    .into_iter()
                    .map(|sample| {
                        sample.replace(
                            "0000000000000000000000000000000000000000000000000000000000000000",
                            asserted_federation_id,
                        )
                    })
                    .collect(),
            }],
            complete: true,
        };
        assert_eq!(
            store
                .commit_metrics(
                    &work,
                    commit(
                        "0000000000000000000000000000000000000000000000000000000000000000",
                        100_000,
                    ),
                    100,
                )
                .await
                .unwrap(),
            CommitOutcome::Committed
        );
        // A listed seat whose current invite cannot be admitted has no successful
        // replacement snapshot, so the previous assertion remains the latest
        // successful observation.
        assert_eq!(
            store
                .commit_metrics(
                    &work,
                    MetricsCommit {
                        listed_seats: Some(["aa".to_owned()].into()),
                        snapshots: vec![],
                        complete: false,
                    },
                    100,
                )
                .await
                .unwrap(),
            CommitOutcome::Committed
        );
        let retained = store
            .metric_snapshots(&metrics_policy(), 100, 100_000)
            .await
            .unwrap();
        assert_eq!(
            retained.snapshots[0].asserted_federation_id,
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(
            store
                .commit_metrics(
                    &work,
                    commit(
                        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                        101_000,
                    ),
                    100,
                )
                .await
                .unwrap(),
            CommitOutcome::Committed
        );
        let loaded = store
            .metric_snapshots(&metrics_policy(), 100, 101_000)
            .await
            .unwrap();
        assert_eq!(loaded.snapshots.len(), 1);
        assert_eq!(
            loaded.snapshots[0].asserted_federation_id,
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        );
        assert_eq!(loaded.snapshots[0].observed_at_ms, 101_000);
    }

    #[tokio::test]
    async fn restarted_same_policy_omits_hostile_snapshots_and_keeps_valid_neighbors() {
        let directory = tempfile::tempdir().unwrap();
        let first = store(&directory).await;
        first
            .reserve_auth(&auth("metrics", 100), 100)
            .await
            .unwrap();
        first
            .admit(&auth("metrics", 100), target(1), 100)
            .await
            .unwrap();
        first
            .configure_metrics_policy(&metrics_policy().fingerprint())
            .await
            .unwrap();

        insert_snapshot(&first, "aa", 100_000, &stored_samples("aa", 1)).await;

        let foreign_identity = stored_samples("bb", 2)
            .into_iter()
            .map(|sample| sample.replace("fman_id=\"11\"", "fman_id=\"other\""))
            .collect::<Vec<_>>();
        insert_snapshot(&first, "bb", 100_001, &foreign_identity).await;

        let forbidden_label = stored_samples("cc", 3)
            .into_iter()
            .map(|sample| sample.replace(",fman_name=", ",capability=\"held-secret\",fman_name="))
            .collect::<Vec<_>>();
        insert_snapshot(&first, "cc", 100_002, &forbidden_label).await;

        let mut duplicate_series = stored_samples("dd", 4);
        duplicate_series.push(duplicate_series[1].clone());
        insert_snapshot(&first, "dd", 100_003, &duplicate_series).await;

        let cardinality = vec![stored_samples("ee", 5)[1].clone(); 20_001];
        insert_snapshot(&first, "ee", 100_004, &cardinality).await;

        insert_snapshot(&first, "ff", 101_001, &stored_samples("ff", 6)).await;

        let target_id: String =
            sqlx::query_scalar("SELECT target_id FROM targets WHERE fman_pubkey='11'")
                .fetch_one(&first.pool)
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO metric_snapshots(
                 target_id,guardian_seat_id,asserted_federation_id,observed_at_ms,samples_json,sample_count
             ) VALUES (?,?,?,?,?,?)",
        )
        .bind(target_id)
        .bind("gg")
        .bind("0000000000000000000000000000000000000000000000000000000000000000")
        .bind(100_006)
        .bind(123_i64)
        .bind(0_i64)
        .execute(&first.pool)
        .await
        .unwrap();
        insert_snapshot(&first, "hh", 100_007, &stored_samples("hh", 7)).await;
        insert_snapshot(&first, "ii", 100_008, &stored_samples("ii", 8)).await;
        let mut connection = first.pool.acquire().await.unwrap();
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE metric_snapshots SET asserted_federation_id='NOT-CANONICAL'
             WHERE guardian_seat_id='hh'",
        )
        .execute(&mut *connection)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE metric_snapshots
             SET asserted_federation_id=?
             WHERE guardian_seat_id='ii'",
        )
        .bind("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
        .execute(&mut *connection)
        .await
        .unwrap();
        drop(connection);

        drop(first);
        let restarted = store(&directory).await;
        restarted
            .configure_metrics_policy(&metrics_policy().fingerprint())
            .await
            .unwrap();
        let loaded = restarted
            .metric_snapshots(&metrics_policy(), 101, 101_000)
            .await
            .unwrap();
        assert_eq!(loaded.rejected, 8);
        assert_eq!(loaded.snapshots.len(), 1);
        assert_eq!(loaded.snapshots[0].guardian_seat_id, "aa");
        assert_eq!(loaded.snapshots[0].observed_at_ms, 100_000);
    }

    #[tokio::test]
    async fn changed_metrics_policy_discards_snapshots_before_exposition() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory).await;
        store
            .reserve_auth(&auth("metrics", 100), 100)
            .await
            .unwrap();
        store
            .admit(&auth("metrics", 100), target(1), 100)
            .await
            .unwrap();
        store
            .configure_metrics_policy(&metrics_policy().fingerprint())
            .await
            .unwrap();
        insert_snapshot(&store, "aa", 100_000, &stored_samples("aa", 1)).await;

        store
            .configure_metrics_policy("changed-policy")
            .await
            .unwrap();
        let loaded = store
            .metric_snapshots(&metrics_policy(), 101, 101_000)
            .await
            .unwrap();
        assert!(loaded.snapshots.is_empty());
        assert_eq!(loaded.rejected, 0);
    }

    #[tokio::test]
    async fn persisted_snapshot_load_enforces_the_aggregate_sample_cap() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory).await;
        store
            .reserve_auth(&auth("metrics", 100), 100)
            .await
            .unwrap();
        store
            .admit(&auth("metrics", 100), target(1), 100)
            .await
            .unwrap();
        store
            .configure_metrics_policy(&metrics_policy().fingerprint())
            .await
            .unwrap();
        for index in 0..11 {
            let seat = format!("seat-{index:02}");
            insert_snapshot(&store, &seat, 100_000, &stored_peer_samples(&seat)).await;
        }

        let loaded = store
            .metric_snapshots(&metrics_policy(), 101, 101_000)
            .await
            .unwrap();
        assert_eq!(
            loaded
                .snapshots
                .iter()
                .map(|snapshot| snapshot.samples.len())
                .sum::<usize>(),
            100_000
        );
        assert_eq!(loaded.rejected, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn metrics_exposition_and_quarantine_have_one_lifecycle_order() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory).await;
        store
            .reserve_auth(&auth("metrics-linearization", 100), 100)
            .await
            .unwrap();
        store
            .admit(&auth("metrics-linearization", 100), target(1), 100)
            .await
            .unwrap();
        let scheduled = store
            .active_collection_targets(100)
            .await
            .unwrap()
            .remove(0);
        let work = store
            .begin_metric_work(&scheduled, 100, 1_800)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            store
                .commit_metrics(
                    &work,
                    MetricsCommit {
                        listed_seats: Some(["aa".to_owned()].into()),
                        snapshots: vec![SeatObservation {
                            guardian_seat_id: "aa".into(),
                            asserted_federation_id:
                                "0000000000000000000000000000000000000000000000000000000000000000"
                                    .to_owned(),
                            observed_at_ms: 100_000,
                            samples: stored_samples("aa", 1),
                        }],
                        complete: true,
                    },
                    100,
                )
                .await
                .unwrap(),
            CommitOutcome::Committed
        );

        let hook = std::sync::Arc::new(TestCommitHook {
            entered_once: std::sync::atomic::AtomicBool::new(false),
            entered: std::sync::Barrier::new(2),
            release: std::sync::Barrier::new(2),
        });
        let scrape_store = store.clone().with_metric_exposition_hook(hook.clone());
        let scrape = tokio::spawn(async move {
            scrape_store
                .metric_exposition(&metrics_policy(), 100, 100_000, 100)
                .await
                .unwrap()
        });
        let entered = hook.clone();
        tokio::task::spawn_blocking(move || entered.entered.wait())
            .await
            .unwrap();
        let mut contending = store.pool.acquire().await.unwrap();
        sqlx::query("PRAGMA busy_timeout = 0")
            .execute(&mut *contending)
            .await
            .unwrap();
        let error = sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *contending)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("database is locked"),
            "exposition must hold the write reservation: {error}"
        );
        drop(contending);

        let release = hook.clone();
        tokio::task::spawn_blocking(move || release.release.wait())
            .await
            .unwrap();
        let MetricExpositionView {
            snapshots, targets, ..
        } = scrape.await.unwrap();
        assert_eq!(snapshots.snapshots.len(), 1);
        assert_eq!(targets.len(), 1);
        assert!(store.quarantine("11").await.unwrap());

        let MetricExpositionView {
            snapshots, targets, ..
        } = store
            .metric_exposition(&metrics_policy(), 100, 100_000, 100)
            .await
            .unwrap();
        assert!(snapshots.snapshots.is_empty());
        assert!(targets.is_empty());
    }

    #[tokio::test]
    async fn metric_attempt_deadline_survives_restart_and_policy_changes_reset_state() {
        let directory = tempfile::tempdir().unwrap();
        let _lock = crate::data_root_lock::DataRootLock::acquire(&data_dir(&directory)).unwrap();
        let first = Store::open(
            &data_dir(&directory).join("state.sqlite"),
            "development",
            SecretCipher::new(&[7; 32]),
            "test".into(),
            3600,
        )
        .await
        .unwrap();
        first
            .reserve_auth(&auth("metrics-due", 100), 100)
            .await
            .unwrap();
        first
            .admit(&auth("metrics-due", 100), target(1), 100)
            .await
            .unwrap();
        first.configure_metrics_policy("policy-a").await.unwrap();
        let target = first.due_metric_targets(100).await.unwrap().remove(0);
        assert!(
            first
                .begin_metric_work(&target, 100, 1800)
                .await
                .unwrap()
                .is_some()
        );
        assert!(first.due_metric_targets(1899).await.unwrap().is_empty());
        drop(first);
        let restarted = Store::open(
            &data_dir(&directory).join("state.sqlite"),
            "development",
            SecretCipher::new(&[7; 32]),
            "test".into(),
            3600,
        )
        .await
        .unwrap();
        assert_eq!(restarted.next_metric_due_at(101).await.unwrap(), Some(1900));
        assert!(restarted.due_metric_targets(1899).await.unwrap().is_empty());
        assert_eq!(restarted.due_metric_targets(1900).await.unwrap().len(), 1);
        restarted
            .configure_metrics_policy("policy-b")
            .await
            .unwrap();
        assert_eq!(restarted.due_metric_targets(101).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn expiry_renewal_preserves_the_durable_attempt_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let first = store(&directory).await;
        first
            .reserve_auth(&auth("expiring", 100), 100)
            .await
            .unwrap();
        first
            .admit(&auth("expiring", 100), target(1), 100)
            .await
            .unwrap();
        first.configure_metrics_policy("policy").await.unwrap();
        let scheduled = first.due_metric_targets(100).await.unwrap().remove(0);
        first
            .begin_metric_work(&scheduled, 100, 1800)
            .await
            .unwrap()
            .unwrap();
        drop(first);

        let renewed = Store::open(
            &data_dir(&directory).join("state.sqlite"),
            "development",
            SecretCipher::new(&[7; 32]),
            "test".into(),
            3600,
        )
        .await
        .unwrap();
        renewed
            .reserve_auth(&auth("renewed", 222), 222)
            .await
            .unwrap();
        renewed
            .admit(&auth("renewed", 222), target(2), 222)
            .await
            .unwrap();
        drop(renewed);

        let restarted = Store::open(
            &data_dir(&directory).join("state.sqlite"),
            "development",
            SecretCipher::new(&[7; 32]),
            "test".into(),
            3600,
        )
        .await
        .unwrap();
        assert!(restarted.due_metric_targets(1899).await.unwrap().is_empty());
        assert_eq!(restarted.due_metric_targets(1900).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reservation_survives_failed_resolution_and_restart() {
        let directory = tempfile::tempdir().unwrap();
        let _lock = crate::data_root_lock::DataRootLock::acquire(&data_dir(&directory)).unwrap();
        let first = Store::open(
            &data_dir(&directory).join("state.sqlite"),
            "development",
            SecretCipher::new(&[7; 32]),
            "test".into(),
            3600,
        )
        .await
        .unwrap();
        first
            .reserve_auth(&auth("resolution-fence", 100), 100)
            .await
            .unwrap();
        first
            .admit(&auth("resolution-fence", 100), target(1), 100)
            .await
            .unwrap();
        let scheduled = first.due_metric_targets(100).await.unwrap().remove(0);
        assert!(
            first
                .reserve_metric_attempt(&scheduled, 100, 1800)
                .await
                .unwrap()
        );
        first.quarantine(target(1).fman_pubkey).await.unwrap();
        assert!(
            first
                .begin_collection_work(&scheduled, 100)
                .await
                .unwrap()
                .is_none()
        );
        first.reactivate(target(1).fman_pubkey).await.unwrap();
        drop(first);

        let restarted = Store::open(
            &data_dir(&directory).join("state.sqlite"),
            "development",
            SecretCipher::new(&[7; 32]),
            "test".into(),
            3600,
        )
        .await
        .unwrap();
        assert_eq!(restarted.next_metric_due_at(101).await.unwrap(), Some(1900));
        assert!(restarted.due_metric_targets(1899).await.unwrap().is_empty());
        assert_eq!(restarted.due_metric_targets(1900).await.unwrap().len(), 1);
    }
}
