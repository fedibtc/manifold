use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fedi_decentralized_service_manifold_guardian::{
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
    metrics_types::{MetricsCommit, MetricsCommitOutcome},
    mg_reports::{MgReport, Stated},
};

/// Bounds on the seats' latest snapshots, in uncompressed samples JSON.
const MAX_METRIC_STATE_BYTES: i64 = 64 * 1024 * 1024;
const MAX_METRIC_STATE_SAMPLES: i64 = 200_000;
const MAX_METRIC_SNAPSHOT_ROWS: i64 = MAX_TARGETS * 64;
/// Bound on one stored observation, uncompressed and compressed.
const MAX_METRIC_ROW_BYTES: usize = 4 * 1024 * 1024;
/// A seat keeps at most one observation per bucket. Migration
/// `0008_metric_observations` indexes the same bucket.
const METRIC_HISTORY_BUCKET_MS: i64 = 15 * 60 * 1000;
/// Observations older than this are pruned, except each seat's latest.
const METRIC_HISTORY_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Bounds on all stored observations, latest included. When they are reached
/// the oldest observations that are not a seat's latest are pruned first.
const MAX_METRIC_HISTORY_BYTES: i64 = 1024 * 1024 * 1024;
const MAX_METRIC_HISTORY_ROWS: i64 = 1_000_000;

/// How much decoded sample text one read may hold in memory.
#[derive(Clone, Copy)]
struct SnapshotBudget {
    bytes: i64,
    samples: i64,
}

/// One private exposition of every seat's latest snapshot.
const LATEST_BUDGET: SnapshotBudget = SnapshotBudget {
    bytes: MAX_METRIC_STATE_BYTES,
    samples: MAX_METRIC_STATE_SAMPLES,
};

/// One seat history response.
const HISTORY_BUDGET: SnapshotBudget = SnapshotBudget {
    bytes: 128 * 1024 * 1024,
    samples: 1_000_000,
};

/// One federation whose invite an authenticated MG asserted for a current seat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ExportedFederation {
    pub(crate) federation_id: String,
    /// Invite from the federation's most recently observed seat.
    pub(crate) invite_code: String,
    /// Current seats that stored an invite for this federation.
    pub(crate) seats: u32,
    pub(crate) last_seen_ms: i64,
}

/// The newest report of one kind an active target journaled.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportedMgReport<T> {
    pub(crate) mg_pubkey: String,
    pub(crate) observed_at_ms: i64,
    pub(crate) report: T,
}

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
    pub(crate) mg_pubkey: &'a str,
    pub(crate) mg_name: &'a str,
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
    max_journals_per_target: u16,
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
    pub(crate) fn max_journals_per_target(&self) -> u16 {
        self.max_journals_per_target
    }

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
        max_journals_per_target: u16,
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
        compress_uncompressed_metric_snapshots(&pool).await?;
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
            max_journals_per_target,
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
        .bind(target.mg_pubkey)
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
                    target.mg_pubkey,
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
            target.mg_pubkey,
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
        .bind(target.mg_pubkey).bind(target_id).bind(target.mg_name).bind(&self.key_id)
        .bind(nonce).bind(ciphertext).bind(generation).bind(auth.created_at).bind(lease_until)
        .bind(now).bind(now).bind(revision_bump).execute(&mut *transaction).await?;
        if expired {
            sqlx::query(
                "DELETE FROM metric_observations WHERE target_id =
                 (SELECT target_id FROM targets WHERE fman_pubkey = ?)",
            )
            .bind(target.mg_pubkey)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "DELETE FROM metric_listed_seats WHERE target_id =
                 (SELECT target_id FROM targets WHERE fman_pubkey = ?)",
            )
            .bind(target.mg_pubkey)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "UPDATE metric_poll_state SET last_complete_at=NULL WHERE target_id =
                 (SELECT target_id FROM targets WHERE fman_pubkey = ?)",
            )
            .bind(target.mg_pubkey)
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
        mg_pubkey: &str,
        now: i64,
    ) -> Result<Option<TargetStatus>, StoreError> {
        let row = sqlx::query("SELECT status, lease_until FROM targets WHERE fman_pubkey = ?")
            .bind(mg_pubkey)
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
    pub async fn quarantine(&self, mg_pubkey: &str) -> Result<bool, StoreError> {
        self.set_status(mg_pubkey, "quarantined").await
    }

    /// Explicitly reactivate a quarantined target and advance its worker fence.
    #[cfg(test)]
    pub async fn reactivate(&self, mg_pubkey: &str) -> Result<bool, StoreError> {
        self.set_status(mg_pubkey, "active").await
    }

    #[cfg(test)]
    async fn set_status(&self, mg_pubkey: &str, status: &str) -> Result<bool, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let target_id: Option<String> =
            sqlx::query_scalar("SELECT target_id FROM targets WHERE fman_pubkey = ?")
                .bind(mg_pubkey)
                .fetch_optional(&mut *transaction)
                .await?;
        let changed = sqlx::query(
            "UPDATE targets SET status = ?, registration_revision = registration_revision + 1
             WHERE fman_pubkey = ? AND status != ?",
        )
        .bind(status)
        .bind(mg_pubkey)
        .bind(status)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if status == "quarantined"
            && let Some(target_id) = target_id
        {
            sqlx::query("DELETE FROM metric_observations WHERE target_id = ?")
                .bind(&target_id)
                .execute(&mut *transaction)
                .await?;
            sqlx::query("DELETE FROM metric_listed_seats WHERE target_id = ?")
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

    /// Bind durable metrics state to the exact inventory policy.
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
            sqlx::query("DELETE FROM metric_observations")
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
            "DELETE FROM metric_observations WHERE target_id IN
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
        let mg_pubkey: String = row.get("fman_pubkey");
        let stored_key_id: String = row.get("key_id");
        if stored_key_id != self.key_id {
            return Err(StoreError::Secret);
        }
        let aad = target_secret_aad(
            &self.trust_profile,
            &stored_key_id,
            &mg_pubkey,
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
            mg_pubkey,
            row.get("fman_name"),
        )))
    }

    /// Atomically add successful seat observations as the seats' latest
    /// snapshots if the target fence is still current, then prune history.
    ///
    /// Latest snapshots are capped across all targets. A snapshot that
    /// would take the total over the cap is skipped: the seat keeps its previous
    /// snapshot, which ages into staleness, and the commit is not complete.
    /// Within one commit, snapshots of seats that already have one are applied
    /// before new seats, so new seats do not take capacity ahead of existing
    /// replacements. A replacement that itself grows past the remaining
    /// capacity is skipped the same way and keeps its old observation time.
    /// Older observations stay as history, one per seat and bucket, until
    /// retention or the history bounds prune them.
    pub(crate) async fn commit_metrics(
        &self,
        target: &WorkTarget,
        commit: MetricsCommit,
        now: i64,
    ) -> Result<MetricsCommitOutcome, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "DELETE FROM metric_observations WHERE target_id IN
             (SELECT target_id FROM targets WHERE status!='active' OR lease_until <= ?)",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        if !target_is_current(&mut transaction, target, now).await? {
            return Ok(MetricsCommitOutcome::Stale);
        }
        if let Some(listed_seats) = &commit.listed_seats {
            sqlx::query("DELETE FROM metric_listed_seats WHERE target_id = ?")
                .bind(target.target_id())
                .execute(&mut *transaction)
                .await?;
            for seat in listed_seats {
                sqlx::query(
                    "INSERT INTO metric_listed_seats(target_id,guardian_seat_id) VALUES (?,?)",
                )
                .bind(target.target_id())
                .bind(seat)
                .execute(&mut *transaction)
                .await?;
            }
            let existing: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT guardian_seat_id FROM metric_observations WHERE target_id = ?",
            )
            .bind(target.target_id())
            .fetch_all(&mut *transaction)
            .await?;
            for seat in existing {
                if !listed_seats.contains(&seat) {
                    sqlx::query(
                        "DELETE FROM metric_observations
                         WHERE target_id = ? AND guardian_seat_id = ?",
                    )
                    .bind(target.target_id())
                    .bind(seat)
                    .execute(&mut *transaction)
                    .await?;
                }
            }
        }
        let (mut stored_bytes, mut stored_samples): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(samples_json_bytes),0),COALESCE(SUM(sample_count),0)
             FROM metric_snapshots",
        )
        .fetch_one(&mut *transaction)
        .await?;
        let existing: BTreeMap<String, (i64, i64)> = sqlx::query_as(
            "SELECT guardian_seat_id,samples_json_bytes,sample_count
             FROM metric_snapshots WHERE target_id = ?",
        )
        .bind(target.target_id())
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|(seat, bytes, samples)| (seat, (bytes, samples)))
        .collect();
        let mut snapshots = commit.snapshots;
        snapshots.sort_by_key(|snapshot| !existing.contains_key(&snapshot.guardian_seat_id));
        let mut saturated = false;
        for snapshot in snapshots {
            let sample_count =
                i64::try_from(snapshot.samples.len()).map_err(|_| StoreError::Saturated)?;
            let samples = serde_json::to_vec(&snapshot.samples).map_err(|_| StoreError::Corrupt)?;
            if samples.len() > MAX_METRIC_ROW_BYTES {
                return Err(StoreError::Saturated);
            }
            let row_bytes = i64::try_from(samples.len()).map_err(|_| StoreError::Saturated)?;
            let (old_bytes, old_samples) = existing
                .get(&snapshot.guardian_seat_id)
                .copied()
                .unwrap_or((0, 0));
            let next_bytes = stored_bytes - old_bytes + row_bytes;
            let next_samples = stored_samples - old_samples + sample_count;
            if next_bytes > MAX_METRIC_STATE_BYTES || next_samples > MAX_METRIC_STATE_SAMPLES {
                saturated = true;
                continue;
            }
            let samples_zstd = compress_samples(&samples)?;
            stored_bytes = next_bytes;
            stored_samples = next_samples;
            let invite_aad = seat_invite_aad(
                &self.trust_profile,
                &self.key_id,
                target.target_id(),
                &snapshot.guardian_seat_id,
                &snapshot.federation_id,
            )?;
            let (invite_nonce, invite_ciphertext) = match &snapshot.invite_code {
                Some(invite) => {
                    let (nonce, ciphertext) = self
                        .cipher
                        .encrypt(invite.as_bytes(), &invite_aad)
                        .map_err(|_| StoreError::Secret)?;
                    (Some(nonce), Some(ciphertext))
                }
                None => (None, None),
            };
            // The new observation becomes the seat's latest: it replaces the
            // row of its own bucket and any row a wall-clock step back left
            // dated after it, and only it keeps the seat invite.
            sqlx::query(
                "DELETE FROM metric_observations
                 WHERE target_id = ? AND guardian_seat_id = ?
                   AND (observed_at_ms >= ? OR observed_at_ms / ? = ? / ?)",
            )
            .bind(target.target_id())
            .bind(&snapshot.guardian_seat_id)
            .bind(snapshot.observed_at_ms)
            .bind(METRIC_HISTORY_BUCKET_MS)
            .bind(snapshot.observed_at_ms)
            .bind(METRIC_HISTORY_BUCKET_MS)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "UPDATE metric_observations SET invite_nonce=NULL,invite_ciphertext=NULL
                 WHERE target_id = ? AND guardian_seat_id = ? AND invite_ciphertext IS NOT NULL",
            )
            .bind(target.target_id())
            .bind(&snapshot.guardian_seat_id)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "INSERT INTO metric_observations(
                    target_id,guardian_seat_id,observed_at_ms,federation_id,
                    samples_zstd,samples_json_bytes,sample_count,invite_nonce,invite_ciphertext
                 ) VALUES (?,?,?,?,?,?,?,?,?)",
            )
            .bind(target.target_id())
            .bind(snapshot.guardian_seat_id)
            .bind(snapshot.observed_at_ms)
            .bind(snapshot.federation_id)
            .bind(samples_zstd)
            .bind(row_bytes)
            .bind(sample_count)
            .bind(invite_nonce)
            .bind(invite_ciphertext)
            .execute(&mut *transaction)
            .await?;
        }
        prune_metric_history(
            &mut transaction,
            now.checked_mul(1000)
                .and_then(|now_ms| now_ms.checked_sub(METRIC_HISTORY_RETENTION_MS))
                .ok_or(StoreError::Corrupt)?,
            MAX_METRIC_HISTORY_BYTES,
            MAX_METRIC_HISTORY_ROWS,
        )
        .await?;
        if commit.complete && !saturated {
            sqlx::query("UPDATE metric_poll_state SET last_complete_at=? WHERE target_id=?")
                .bind(now)
                .bind(target.target_id())
                .execute(&mut *transaction)
                .await?;
        }
        sqlx::query("UPDATE metric_exposition_revision SET revision=revision+1 WHERE singleton=1")
            .execute(&mut *transaction)
            .await?;
        #[cfg(test)]
        wait_test_hook(&self.metric_commit_hook);
        transaction.commit().await?;
        Ok(if saturated {
            MetricsCommitOutcome::Saturated
        } else {
            MetricsCommitOutcome::Committed
        })
    }

    /// Federations of the current seats' latest admitted snapshots, one entry per
    /// federation, ordered by federation id.
    ///
    /// Only active targets with an unexpired lease contribute, so an invite leaves
    /// this view together with its seat's snapshot.
    pub(crate) async fn exported_federations(
        &self,
        now: i64,
    ) -> Result<Vec<ExportedFederation>, StoreError> {
        let rows = sqlx::query(
            "SELECT s.target_id,s.guardian_seat_id,s.federation_id,s.observed_at_ms,
                    s.invite_nonce,s.invite_ciphertext
             FROM metric_snapshots s JOIN targets t ON t.target_id = s.target_id
             WHERE t.status = 'active' AND t.lease_until > ? AND s.invite_ciphertext IS NOT NULL
             ORDER BY s.federation_id, s.observed_at_ms DESC, s.target_id, s.guardian_seat_id",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        let mut federations: Vec<ExportedFederation> = Vec::new();
        for row in rows {
            let target_id: String = row.try_get("target_id")?;
            let guardian_seat_id: String = row.try_get("guardian_seat_id")?;
            let federation_id: String = row.try_get("federation_id")?;
            let observed_at_ms: i64 = row.try_get("observed_at_ms")?;
            let nonce: Vec<u8> = row.try_get("invite_nonce")?;
            let ciphertext: Vec<u8> = row.try_get("invite_ciphertext")?;
            if !canonical_federation_id(&federation_id) {
                return Err(StoreError::Corrupt);
            }
            let aad = seat_invite_aad(
                &self.trust_profile,
                &self.key_id,
                &target_id,
                &guardian_seat_id,
                &federation_id,
            )?;
            let invite_code = String::from_utf8(
                self.cipher
                    .decrypt(&nonce, &ciphertext, &aad)
                    .map_err(|_| StoreError::Secret)?,
            )
            .map_err(|_| StoreError::Corrupt)?;
            match federations.last_mut() {
                Some(last) if last.federation_id == federation_id => {
                    last.seats = last.seats.saturating_add(1);
                }
                _ => federations.push(ExportedFederation {
                    federation_id,
                    invite_code,
                    seats: 1,
                    last_seen_ms: observed_at_ms,
                }),
            }
        }
        Ok(federations)
    }

    /// The stored report of kind `T` per active, unexpired target, ordered by
    /// MG public key. `now_secs` is Unix seconds; a report observed after it
    /// is left out, and a stored row that no longer validates is skipped and
    /// counted in a warning.
    pub(crate) async fn mg_reports<T: MgReport>(
        &self,
        now_secs: i64,
    ) -> Result<Vec<ExportedMgReport<T>>, StoreError> {
        let rows = sqlx::query(
            "SELECT t.fman_pubkey,r.observed_at_ms,r.report_json
             FROM mg_reports r JOIN targets t ON t.target_id = r.target_id
             WHERE t.status = 'active' AND t.lease_until > ? AND r.kind = ?
                AND r.observed_at_ms <= ?
             ORDER BY t.fman_pubkey",
        )
        .bind(now_secs)
        .bind(T::KIND.as_str())
        .bind(now_secs.checked_mul(1000).ok_or(StoreError::Corrupt)?)
        .fetch_all(&self.pool)
        .await?;
        let mut reports = Vec::with_capacity(rows.len());
        let mut skipped = 0_usize;
        for row in rows {
            let report_json: String = row.try_get("report_json")?;
            if let Ok(report) = serde_json::from_str(&report_json) {
                reports.push(ExportedMgReport {
                    mg_pubkey: row.try_get("fman_pubkey")?,
                    observed_at_ms: row.try_get("observed_at_ms")?,
                    report,
                });
            } else {
                skipped += 1;
            }
        }
        if skipped > 0 {
            // The message and fields are fixed or counts. They contain no MG
            // identity and no stored value.
            tracing::warn!(
                kind = T::KIND.as_str(),
                skipped,
                "stored seat reports failed revalidation"
            );
        }
        Ok(reports)
    }

    /// Read one lifecycle-consistent private metrics exposition view.
    pub(crate) async fn metric_exposition(
        &self,
        policy: &MetricsPolicy,
        now: i64,
        now_ms: i64,
        stale_after: i64,
    ) -> Result<MetricExpositionView, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let removed = sqlx::query(
            "DELETE FROM metric_observations WHERE target_id IN
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
                  SELECT t.target_id,t.fman_pubkey,t.fman_name,s.guardian_seat_id,s.federation_id,
                         typeof(s.samples_zstd) AS samples_type,
                         length(CAST(s.samples_zstd AS BLOB)) AS stored_bytes
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
                  SELECT target_id,fman_pubkey,fman_name,guardian_seat_id,federation_id
                 FROM evaluated
                 WHERE samples_type='blob' AND stored_bytes <= ? AND cumulative_bytes <= ?
             ), summary AS (
                 SELECT COUNT(*) AS rejected_resources FROM evaluated
                 WHERE samples_type!='blob' OR stored_bytes > ? OR cumulative_bytes > ?
             )
            SELECT summary.rejected_resources,accepted.target_id IS NULL AS no_accepted,
                     accepted.fman_pubkey,accepted.fman_name,accepted.guardian_seat_id,
                     accepted.federation_id,
                    s.observed_at_ms,s.samples_zstd
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
        let mut listed_seats = active_listed_seats(&mut transaction, now).await?;
        transaction.commit().await?;

        let rejected = snapshot_rows
            .first()
            .and_then(|row| row.try_get::<i64, _>("rejected_resources").ok())
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(usize::MAX);
        let snapshots =
            revalidate_snapshot_rows(policy, snapshot_rows, rejected, now_ms, LATEST_BUDGET);
        let targets = target_rows
            .into_iter()
            .map(|row| {
                let completed: Option<i64> = row.get("last_complete_at");
                let mg_id: String = row.get("fman_pubkey");
                MetricsTargetHealth {
                    seats: listed_seats.remove(&mg_id).unwrap_or_default(),
                    mg_id,
                    mg_name: row.get("fman_name"),
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
        policy: &MetricsPolicy,
        now_seconds: i64,
        now_ms: i64,
    ) -> Result<LoadedMetricSnapshots, StoreError> {
        sqlx::query(
            "DELETE FROM metric_observations WHERE target_id IN
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
                  SELECT t.target_id,t.fman_pubkey,t.fman_name,s.guardian_seat_id,s.federation_id,
                         typeof(s.samples_zstd) AS samples_type,
                         length(CAST(s.samples_zstd AS BLOB)) AS stored_bytes
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
                  SELECT target_id,fman_pubkey,fman_name,guardian_seat_id,federation_id
                 FROM evaluated
                 WHERE samples_type='blob' AND stored_bytes <= ? AND cumulative_bytes <= ?
             ), summary AS (
                 SELECT COUNT(*) AS rejected_resources FROM evaluated
                 WHERE samples_type!='blob' OR stored_bytes > ? OR cumulative_bytes > ?
             )
            SELECT summary.rejected_resources,accepted.target_id IS NULL AS no_accepted,
                     accepted.fman_pubkey,accepted.fman_name,accepted.guardian_seat_id,
                     accepted.federation_id,
                    s.observed_at_ms,s.samples_zstd
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
        let rejected = rows
            .first()
            .and_then(|row| row.try_get::<i64, _>("rejected_resources").ok())
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(usize::MAX);
        Ok(revalidate_snapshot_rows(
            policy,
            rows,
            rejected,
            now_ms,
            LATEST_BUDGET,
        ))
    }

    /// The revalidated observations of one active, unexpired target's seat
    /// observed in the retention window and before `before_ms`, oldest first.
    /// The newest that fit one response's budget are kept; `rejected` counts
    /// the rows that failed revalidation or did not fit.
    pub(crate) async fn metric_history(
        &self,
        policy: &MetricsPolicy,
        mg_id: &str,
        guardian_seat_id: &str,
        before_ms: i64,
        now_ms: i64,
    ) -> Result<LoadedMetricSnapshots, StoreError> {
        let rows = sqlx::query(
            "WITH candidates AS (
                 SELECT o.target_id,t.fman_pubkey,t.fman_name,o.guardian_seat_id,o.federation_id,
                        o.observed_at_ms,typeof(o.samples_zstd) AS samples_type,
                        length(CAST(o.samples_zstd AS BLOB)) AS stored_bytes
                 FROM metric_observations o JOIN targets t ON t.target_id=o.target_id
                 WHERE t.status='active' AND t.lease_until > ?
                   AND t.fman_pubkey=? AND o.guardian_seat_id=?
                   AND o.observed_at_ms >= ? AND o.observed_at_ms < ?
                 ORDER BY o.observed_at_ms DESC LIMIT ?
             ), evaluated AS (
                 SELECT *,
                        SUM(
                            CASE WHEN samples_type='blob' AND stored_bytes <= ? THEN stored_bytes
                                 ELSE 0 END
                        ) OVER (ORDER BY observed_at_ms DESC ROWS UNBOUNDED PRECEDING)
                            AS cumulative_bytes
                 FROM candidates
             )
             SELECT e.fman_pubkey,e.fman_name,e.guardian_seat_id,e.federation_id,
                    e.observed_at_ms,o.samples_zstd
             FROM evaluated e LEFT JOIN metric_observations o
                 ON o.target_id=e.target_id AND o.guardian_seat_id=e.guardian_seat_id
                AND o.observed_at_ms=e.observed_at_ms
                AND e.samples_type='blob' AND e.stored_bytes <= ? AND e.cumulative_bytes <= ?
             ORDER BY e.observed_at_ms DESC",
        )
        .bind(now_ms.div_euclid(1000))
        .bind(mg_id)
        .bind(guardian_seat_id)
        .bind(now_ms.saturating_sub(METRIC_HISTORY_RETENTION_MS))
        .bind(before_ms)
        .bind(METRIC_HISTORY_RETENTION_MS / METRIC_HISTORY_BUCKET_MS + 1)
        .bind(MAX_METRIC_ROW_BYTES as i64)
        .bind(MAX_METRIC_ROW_BYTES as i64)
        .bind(HISTORY_BUDGET.bytes)
        .fetch_all(&self.pool)
        .await?;
        let mut history = revalidate_snapshot_rows(policy, rows, 0, now_ms, HISTORY_BUDGET);
        history.snapshots.reverse();
        Ok(history)
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
        let mut listed_seats = active_listed_seats(&mut *self.pool.acquire().await?, now).await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let completed: Option<i64> = row.get("last_complete_at");
                let mg_id: String = row.get("fman_pubkey");
                MetricsTargetHealth {
                    seats: listed_seats.remove(&mg_id).unwrap_or_default(),
                    mg_id,
                    mg_name: row.get("fman_name"),
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
            "DELETE FROM metric_observations WHERE target_id IN
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
                >= i64::from(self.max_journals_per_target)
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
        // Only the MG's own journal carries its seat reports.
        if expected.journal == SafeEventJournal::Mg {
            let received_at_ms = now.checked_mul(1000).ok_or(StoreError::Corrupt)?;
            let reports = batch.reports();
            if let Some(stated) = &reports.seat_capacity {
                store_mg_report(&mut transaction, target, received_at_ms, stated).await?;
            }
            if let Some(stated) = &reports.seat_readiness {
                store_mg_report(&mut transaction, target, received_at_ms, stated).await?;
            }
        }
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

/// Replace the target's report of kind `T` with `stated`, unless the stored
/// one was observed later.
async fn store_mg_report<T: MgReport>(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    target: &WorkTarget,
    received_at_ms: i64,
    stated: &Stated<T>,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO mg_reports(target_id,kind,observed_at_ms,report_json) VALUES(?,?,?,?)
         ON CONFLICT(target_id,kind) DO UPDATE SET
            observed_at_ms=excluded.observed_at_ms,report_json=excluded.report_json
         WHERE excluded.observed_at_ms >= mg_reports.observed_at_ms",
    )
    .bind(target.target_id())
    .bind(T::KIND.as_str())
    .bind(stated.observed_at_ms(received_at_ms))
    .bind(serde_json::to_string(&stated.report).map_err(|_| StoreError::Corrupt)?)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn compress_samples(samples_json: &[u8]) -> Result<Vec<u8>, StoreError> {
    let compressed = zstd::bulk::compress(samples_json, 3).map_err(|_| StoreError::Corrupt)?;
    if compressed.len() > MAX_METRIC_ROW_BYTES {
        return Err(StoreError::Saturated);
    }
    Ok(compressed)
}

/// Samples of one stored observation, or `None` when the row does not decode
/// within the row bound.
fn decompress_samples(samples_zstd: &[u8]) -> Option<Vec<String>> {
    let samples_json = zstd::bulk::decompress(samples_zstd, MAX_METRIC_ROW_BYTES).ok()?;
    serde_json::from_slice(&samples_json).ok()
}

/// Prune observations that are not their seat's latest: those observed before
/// `cutoff_ms`, then, while all observations exceed the history bounds, the
/// oldest. Latest snapshots have their own smaller bounds, so they always fit.
async fn prune_metric_history(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    cutoff_ms: i64,
    max_bytes: i64,
    max_rows: i64,
) -> Result<(), StoreError> {
    sqlx::query(
        "DELETE FROM metric_observations WHERE observed_at_ms < ? AND observed_at_ms < (
             SELECT MAX(n.observed_at_ms) FROM metric_observations n
             WHERE n.target_id=metric_observations.target_id
               AND n.guardian_seat_id=metric_observations.guardian_seat_id
         )",
    )
    .bind(cutoff_ms)
    .execute(&mut **transaction)
    .await?;
    let (stored_bytes, stored_rows): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(length(samples_zstd)),0),COUNT(*) FROM metric_observations",
    )
    .fetch_one(&mut **transaction)
    .await?;
    if stored_bytes <= max_bytes && stored_rows <= max_rows {
        return Ok(());
    }
    sqlx::query(
        "DELETE FROM metric_observations WHERE rowid IN (
             SELECT id FROM (
                 SELECT o.rowid AS id,o.observed_at_ms=l.latest_ms AS latest,
                        SUM(length(o.samples_zstd)) OVER kept AS kept_bytes,
                        COUNT(*) OVER kept AS kept_rows
                 FROM metric_observations o JOIN (
                     SELECT target_id,guardian_seat_id,MAX(observed_at_ms) AS latest_ms
                     FROM metric_observations GROUP BY target_id,guardian_seat_id
                 ) l ON l.target_id=o.target_id AND l.guardian_seat_id=o.guardian_seat_id
                 WINDOW kept AS (
                     ORDER BY o.observed_at_ms=l.latest_ms DESC,o.observed_at_ms DESC,o.rowid DESC
                     ROWS UNBOUNDED PRECEDING
                 )
             ) WHERE NOT latest AND (kept_bytes > ? OR kept_rows > ?)
         )",
    )
    .bind(max_bytes)
    .bind(max_rows)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Finish migration `0008_metric_observations`: move the latest snapshots it
/// set aside into `metric_observations`, compressed, and drop their old table.
/// Runs at every open and does nothing once that table is gone.
async fn compress_uncompressed_metric_snapshots(pool: &SqlitePool) -> Result<(), StoreError> {
    let mut transaction = pool.begin_with("BEGIN IMMEDIATE").await?;
    let pending: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM sqlite_master
         WHERE type='table' AND name='metric_snapshots_uncompressed'",
    )
    .fetch_optional(&mut *transaction)
    .await?;
    if pending.is_none() {
        transaction.rollback().await?;
        return Ok(());
    }
    let rows = sqlx::query(
        "SELECT target_id,guardian_seat_id,observed_at_ms,federation_id,
                CAST(samples_json AS BLOB) AS samples_json,sample_count,
                invite_nonce,invite_ciphertext
         FROM metric_snapshots_uncompressed",
    )
    .fetch_all(&mut *transaction)
    .await?;
    let mut skipped = 0_usize;
    for row in rows {
        let samples_json: Vec<u8> = row.try_get("samples_json")?;
        // Only incompressible JSON within a few hundred bytes of the row bound
        // can grow past it; such a row could not hold admitted samples.
        let Ok(samples_zstd) = compress_samples(&samples_json) else {
            skipped += 1;
            continue;
        };
        sqlx::query(
            "INSERT INTO metric_observations(
                target_id,guardian_seat_id,observed_at_ms,federation_id,
                samples_zstd,samples_json_bytes,sample_count,invite_nonce,invite_ciphertext
             ) VALUES (?,?,?,?,?,?,?,?,?)",
        )
        .bind(row.try_get::<String, _>("target_id")?)
        .bind(row.try_get::<String, _>("guardian_seat_id")?)
        .bind(row.try_get::<i64, _>("observed_at_ms")?)
        .bind(row.try_get::<String, _>("federation_id")?)
        .bind(samples_zstd)
        .bind(i64::try_from(samples_json.len()).map_err(|_| StoreError::Corrupt)?)
        .bind(row.try_get::<i64, _>("sample_count")?)
        .bind(row.try_get::<Option<Vec<u8>>, _>("invite_nonce")?)
        .bind(row.try_get::<Option<Vec<u8>>, _>("invite_ciphertext")?)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query("DROP TABLE metric_snapshots_uncompressed")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    if skipped > 0 {
        // The message and field are fixed or a count.
        tracing::warn!(skipped, "stored metric snapshots too large to compress");
    }
    Ok(())
}

/// Decode and revalidate loaded snapshot rows in order until the budget is
/// spent. A row that is missing a field, future-dated, undecodable, fails the
/// current policy, or does not fit is counted in `rejected`.
fn revalidate_snapshot_rows(
    policy: &MetricsPolicy,
    rows: Vec<sqlx::sqlite::SqliteRow>,
    mut rejected: usize,
    now_ms: i64,
    budget: SnapshotBudget,
) -> LoadedMetricSnapshots {
    let mut snapshots = Vec::new();
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
                row.try_get::<Option<String>, _>("federation_id")?,
                row.try_get::<Option<i64>, _>("observed_at_ms")?,
                row.try_get::<Option<Vec<u8>>, _>("samples_zstd")?,
            ))
        })();
        let Ok((
            Some(mg_id),
            Some(mg_name),
            Some(guardian_seat_id),
            Some(federation_id),
            Some(observed_at_ms),
            Some(samples_zstd),
        )) = loaded
        else {
            rejected = rejected.saturating_add(1);
            continue;
        };
        if observed_at_ms < 0 || observed_at_ms > now_ms {
            rejected = rejected.saturating_add(1);
            continue;
        }
        if !canonical_federation_id(&federation_id) {
            rejected = rejected.saturating_add(1);
            continue;
        }
        let Some(samples) = decompress_samples(&samples_zstd) else {
            rejected = rejected.saturating_add(1);
            continue;
        };
        if policy
            .revalidate_persisted(
                &samples,
                MetricsIdentity {
                    mg_id: &mg_id,
                    mg_name: &mg_name,
                    guardian_seat_id: &guardian_seat_id,
                    federation_id: &federation_id,
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
        if next_samples > budget.samples || next_bytes > budget.bytes {
            rejected = rejected.saturating_add(1);
            continue;
        }
        sample_count = next_samples;
        sample_bytes = next_bytes;
        snapshots.push(MetricsSnapshot {
            mg_id,
            mg_name,
            guardian_seat_id,
            federation_id,
            observed_at_ms,
            samples,
        });
    }
    LoadedMetricSnapshots {
        snapshots,
        rejected,
    }
}

/// Seats from each active target's latest successful listing, by MG id.
///
/// A row that does not decode is skipped; the seat then has no freshness
/// series until the next listing replaces it.
async fn active_listed_seats(
    connection: &mut sqlx::SqliteConnection,
    now: i64,
) -> Result<BTreeMap<String, Vec<String>>, StoreError> {
    let rows = sqlx::query(
        "SELECT t.fman_pubkey,l.guardian_seat_id
         FROM metric_listed_seats l JOIN targets t ON t.target_id=l.target_id
         WHERE t.status='active' AND t.lease_until > ?
         ORDER BY t.fman_pubkey,l.guardian_seat_id",
    )
    .bind(now)
    .fetch_all(&mut *connection)
    .await?;
    let mut seats: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows {
        let (Ok(mg_id), Ok(seat)) = (
            row.try_get::<String, _>("fman_pubkey"),
            row.try_get::<String, _>("guardian_seat_id"),
        ) else {
            continue;
        };
        seats.entry(mg_id).or_default().push(seat);
    }
    Ok(seats)
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
    #[serde(rename = "fman_pubkey")]
    mg_pubkey: &'a str,
    target_id: &'a str,
    generation: i64,
    auth_created_at: i64,
}

fn target_secret_aad(
    trust_profile: &str,
    key_id: &str,
    mg_pubkey: &str,
    target_id: &str,
    generation: i64,
    auth_created_at: i64,
) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(&TargetSecretAad {
        format: TARGET_SECRET_FORMAT,
        secret_kind: "registration-target",
        trust_profile,
        key_id,
        mg_pubkey,
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

const SEAT_INVITE_FORMAT: i64 = 1;

#[derive(Serialize)]
struct SeatInviteAad<'a> {
    format: i64,
    secret_kind: &'static str,
    trust_profile: &'a str,
    key_id: &'a str,
    target_id: &'a str,
    guardian_seat_id: &'a str,
    federation_id: &'a str,
}

/// Binds an invite ciphertext to its deployment, key, target, seat, and federation.
fn seat_invite_aad(
    trust_profile: &str,
    key_id: &str,
    target_id: &str,
    guardian_seat_id: &str,
    federation_id: &str,
) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(&SeatInviteAad {
        format: SEAT_INVITE_FORMAT,
        secret_kind: "seat-invite",
        trust_profile,
        key_id,
        target_id,
        guardian_seat_id,
        federation_id,
    })
    .map_err(|_| StoreError::Secret)
}

fn key_sentinel_aad(trust_profile: &str, key_id: &str) -> String {
    format!("cloud-fman-telemetry/key-sentinel/v1:{trust_profile}:{key_id}")
}

fn canonical_federation_id(value: &str) -> bool {
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
    use std::collections::BTreeSet;
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
            32,
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
        mg_pubkey: &'static str,
        endpoint_id: &'static str,
        generation: u64,
    ) -> TargetMaterial<'static> {
        TargetMaterial {
            mg_pubkey,
            mg_name: "calm-tern",
            endpoint_id,
            capability: &[9; 32],
            generation,
        }
    }

    fn metrics_policy() -> MetricsPolicy {
        MetricsPolicy
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
                    mg_id: "11",
                    mg_name: "calm-tern",
                    guardian_seat_id: seat,
                    federation_id: "0000000000000000000000000000000000000000000000000000000000000000",
                },
                None,
            )
            .unwrap()
            .samples
    }

    fn stored_peer_samples(seat: &str) -> Vec<String> {
        stored_counter_samples(seat, 9_999)
    }

    /// One start sample plus `peers` per-peer counter samples.
    fn stored_counter_samples(seat: &str, peers: usize) -> Vec<String> {
        let mut body =
            "fm_app_start_ts{version=\"test-version\",version_hash=\"test-hash\"} 1\n".to_owned();
        for peer_id in 0..peers {
            body.push_str(&format!(
                "fm_consensus_items_processed_total{{peer_id=\"{peer_id}\"}} 1\n"
            ));
        }
        metrics_policy()
            .admit_until(
                body.as_bytes(),
                MetricsIdentity {
                    mg_id: "11",
                    mg_name: "calm-tern",
                    guardian_seat_id: seat,
                    federation_id: "0000000000000000000000000000000000000000000000000000000000000000",
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
        let samples_json = serde_json::to_vec(samples).unwrap();
        sqlx::query(
            "INSERT INTO metric_observations(
                 target_id,guardian_seat_id,federation_id,observed_at_ms,
                 samples_zstd,samples_json_bytes,sample_count
             ) VALUES (?,?,?,?,?,?,?)",
        )
        .bind(target_id)
        .bind(seat)
        .bind("0000000000000000000000000000000000000000000000000000000000000000")
        .bind(observed_at_ms)
        .bind(compress_samples(&samples_json).unwrap())
        .bind(i64::try_from(samples_json.len()).unwrap())
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
                32,
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
            32,
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
                32,
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
                32,
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
                32,
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
                            federation_id:
                                "0000000000000000000000000000000000000000000000000000000000000000"
                                    .to_owned(),
                            invite_code: Some("test-invite".to_owned()),
                            observed_at_ms: 100_000,
                            samples: stored_samples("aa", 1),
                        }],
                        complete: true,
                    },
                    100,
                )
                .await
                .unwrap(),
            MetricsCommitOutcome::Committed
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
            MetricsCommitOutcome::Stale
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
                        federation_id:
                            "0000000000000000000000000000000000000000000000000000000000000000"
                                .to_owned(),
                        invite_code: Some("test-invite".to_owned()),
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
        let commit = |federation_id: &str, observed_at_ms| MetricsCommit {
            listed_seats: Some(["aa".to_owned()].into()),
            snapshots: vec![SeatObservation {
                guardian_seat_id: "aa".to_owned(),
                federation_id: federation_id.to_owned(),
                invite_code: Some("test-invite".to_owned()),
                observed_at_ms,
                samples: stored_samples("aa", 1)
                    .into_iter()
                    .map(|sample| {
                        sample.replace(
                            "0000000000000000000000000000000000000000000000000000000000000000",
                            federation_id,
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
            MetricsCommitOutcome::Committed
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
            MetricsCommitOutcome::Committed
        );
        let loaded = store
            .metric_snapshots(&metrics_policy(), 100, 101_000)
            .await
            .unwrap();
        assert_eq!(loaded.snapshots.len(), 1);
        assert_eq!(
            loaded.snapshots[0].federation_id,
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        );
        assert_eq!(loaded.snapshots[0].observed_at_ms, 101_000);
    }

    #[tokio::test]
    async fn exported_federations_follow_current_seat_snapshots() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory).await;
        store
            .reserve_auth(&auth("federation-export", 100), 100)
            .await
            .unwrap();
        store
            .admit(&auth("federation-export", 100), target(1), 100)
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
        let first = "aa".repeat(32);
        let second = "bb".repeat(32);
        let seat =
            |seat: &str, federation_id: &str, invite: &str, observed_at_ms| SeatObservation {
                guardian_seat_id: seat.to_owned(),
                federation_id: federation_id.to_owned(),
                invite_code: Some(invite.to_owned()),
                observed_at_ms,
                samples: stored_samples(seat, 1),
            };
        store
            .commit_metrics(
                &work,
                MetricsCommit {
                    listed_seats: Some(["s1", "s2", "s3", "s4"].map(str::to_owned).into()),
                    snapshots: vec![
                        seat("s1", &first, "invite-first-old", 100_000),
                        seat("s2", &first, "invite-first-new", 101_000),
                        seat("s3", &second, "invite-second", 100_500),
                        SeatObservation {
                            invite_code: None,
                            ..seat("s4", &"cc".repeat(32), "", 100_200)
                        },
                    ],
                    complete: true,
                },
                100,
            )
            .await
            .unwrap();

        assert_eq!(
            store.exported_federations(100).await.unwrap(),
            vec![
                ExportedFederation {
                    federation_id: first.clone(),
                    invite_code: "invite-first-new".to_owned(),
                    seats: 2,
                    last_seen_ms: 101_000,
                },
                ExportedFederation {
                    federation_id: second.clone(),
                    invite_code: "invite-second".to_owned(),
                    seats: 1,
                    last_seen_ms: 100_500,
                },
            ]
        );
        let stored: Vec<Option<Vec<u8>>> = sqlx::query_scalar(
            "SELECT invite_ciphertext FROM metric_snapshots ORDER BY guardian_seat_id",
        )
        .fetch_all(&store.pool)
        .await
        .unwrap();
        assert_eq!(stored.len(), 4);
        assert!(
            stored[3].is_none(),
            "a seat without a canonical invite stores none"
        );
        let stored: Vec<Vec<u8>> = stored.into_iter().flatten().collect();
        assert!(stored.iter().all(|ciphertext| {
            !ciphertext
                .windows(b"invite-".len())
                .any(|window| window == b"invite-")
        }));

        // A seat the MG no longer lists leaves the export with its snapshot.
        store
            .commit_metrics(
                &work,
                MetricsCommit {
                    listed_seats: Some(["s1".to_owned(), "s3".to_owned()].into()),
                    snapshots: Vec::new(),
                    complete: true,
                },
                100,
            )
            .await
            .unwrap();
        let exported = store.exported_federations(100).await.unwrap();
        assert_eq!(exported[0].invite_code, "invite-first-old");
        assert_eq!(exported[0].seats, 1);

        // An expired lease hides every invite of the target.
        assert!(store.exported_federations(220).await.unwrap().is_empty());

        // A ciphertext moved to another seat no longer authenticates.
        sqlx::query(
            "UPDATE metric_observations SET guardian_seat_id='moved' WHERE guardian_seat_id='s3'",
        )
        .execute(&store.pool)
        .await
        .unwrap();
        assert!(matches!(
            store.exported_federations(100).await,
            Err(StoreError::Secret)
        ));
        sqlx::query(
            "UPDATE metric_observations SET guardian_seat_id='s3' WHERE guardian_seat_id='moved'",
        )
        .execute(&store.pool)
        .await
        .unwrap();

        assert!(store.quarantine("11").await.unwrap());
        assert!(store.exported_federations(100).await.unwrap().is_empty());
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
            "INSERT INTO metric_observations(
                 target_id,guardian_seat_id,federation_id,observed_at_ms,
                 samples_zstd,samples_json_bytes,sample_count
             ) VALUES (?,?,?,?,?,?,?)",
        )
        .bind(target_id)
        .bind("gg")
        .bind("0000000000000000000000000000000000000000000000000000000000000000")
        .bind(100_006)
        .bind(123_i64)
        .bind(0_i64)
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
            "UPDATE metric_observations SET federation_id='NOT-CANONICAL'
             WHERE guardian_seat_id='hh'",
        )
        .execute(&mut *connection)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE metric_observations
             SET federation_id=?
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
        // Each peer snapshot holds 10,000 samples; one more than fits.
        let seats = MAX_METRIC_STATE_SAMPLES / 10_000 + 1;
        for index in 0..seats {
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
            usize::try_from(MAX_METRIC_STATE_SAMPLES).unwrap()
        );
        assert_eq!(loaded.rejected, 1);
    }

    /// Renders the private exposition through the production store view.
    async fn rendered_exposition(store: &Store, now: i64, stale_after: i64) -> String {
        let view = store
            .metric_exposition(&metrics_policy(), now, now * 1000, stale_after)
            .await
            .unwrap();
        crate::metrics_snapshot::render_metrics(
            view.snapshots.snapshots,
            view.targets,
            view.snapshots.rejected,
            now * 1000,
            stale_after * 1000,
            false,
            &crate::metrics_observability::MetricsObservability::default(),
        )
        .unwrap()
    }

    fn seat_fresh_line(seat: &str, fresh: u8) -> String {
        format!(
            "cloud_fman_telemetry_seat_fresh{{fman_id=\"11\",fman_name=\"calm-tern\",guardian_seat_id=\"{seat}\"}} {fresh} 101000\n"
        )
    }

    #[tokio::test]
    async fn seat_freshness_counts_only_revalidated_observations_of_listed_seats() {
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
        let listing = |seats: &[&str]| MetricsCommit {
            listed_seats: Some(seats.iter().map(|seat| (*seat).to_owned()).collect()),
            snapshots: Vec::new(),
            complete: false,
        };
        store
            .commit_metrics(&work, listing(&["aa", "bb", "cc", "dd", "ee"]), 100)
            .await
            .unwrap();
        // aa is healthy. bb is future-dated, as after a backward clock step.
        // cc holds a sample the policy rejects. dd has a wrongly typed
        // timestamp. ee was listed but never observed.
        insert_snapshot(&store, "aa", 100_000, &stored_samples("aa", 1)).await;
        insert_snapshot(&store, "bb", 180_000, &stored_samples("bb", 1)).await;
        insert_snapshot(&store, "cc", 100_000, &["fm_not_inventoried 1".to_owned()]).await;
        insert_snapshot(&store, "dd", 100_000, &stored_samples("dd", 1)).await;
        sqlx::query(
            "UPDATE metric_observations SET observed_at_ms='soon' WHERE guardian_seat_id='dd'",
        )
        .execute(&store.pool)
        .await
        .unwrap();

        let output = rendered_exposition(&store, 101, 60).await;
        assert!(output.contains(&seat_fresh_line("aa", 1)), "{output}");
        for seat in ["bb", "cc", "dd", "ee"] {
            assert!(
                output.contains(&seat_fresh_line(seat, 0)),
                "{seat}: {output}"
            );
        }
        assert!(output.contains("cloud_fman_telemetry_snapshot_rejected_rows 3 101000\n"));

        // A later listing replaces the seat set; unlisted seats leave.
        store
            .commit_metrics(&work, listing(&["aa"]), 100)
            .await
            .unwrap();
        let output = rendered_exposition(&store, 101, 60).await;
        assert!(output.contains(&seat_fresh_line("aa", 1)), "{output}");
        assert_eq!(
            output.matches("cloud_fman_telemetry_seat_fresh{").count(),
            1
        );

        // A failed listing keeps the previous seats.
        store
            .commit_metrics(
                &work,
                MetricsCommit {
                    listed_seats: None,
                    snapshots: Vec::new(),
                    complete: false,
                },
                100,
            )
            .await
            .unwrap();
        let output = rendered_exposition(&store, 101, 60).await;
        assert!(output.contains(&seat_fresh_line("aa", 1)), "{output}");

        // The observation ages out after the freshness threshold.
        let output = rendered_exposition(&store, 161, 60).await;
        assert!(
            output.contains(&seat_fresh_line("aa", 0).replace("101000", "161000")),
            "{output}"
        );

        // Renewal after lease expiry starts without the previous listing.
        store
            .reserve_auth(&auth("metrics-renewed", 3_800), 3_800)
            .await
            .unwrap();
        store
            .admit(&auth("metrics-renewed", 3_800), target(1), 3_800)
            .await
            .unwrap();
        let output = rendered_exposition(&store, 3_801, 60).await;
        assert!(
            output.contains("cloud_fman_telemetry_target_fresh{"),
            "{output}"
        );
        assert!(
            !output.contains("cloud_fman_telemetry_seat_fresh{"),
            "{output}"
        );
    }

    #[tokio::test]
    async fn byte_cap_skips_a_grown_replacement_and_applies_a_later_one_that_fits() {
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
        let samples = |count: usize, length: usize| vec!["a".repeat(length); count];
        let stored_bytes = |store: Store| async move {
            sqlx::query_scalar::<_, i64>("SELECT SUM(samples_json_bytes) FROM metric_snapshots")
                .fetch_one(&store.pool)
                .await
                .unwrap()
        };
        // Fill the byte cap to within 10,000 bytes with few samples, so the
        // byte cap and not the sample cap is the binding limit.
        let row = samples(1_000, 4_000);
        let rows = MAX_METRIC_STATE_BYTES / 4_100_000;
        for index in 0..rows {
            insert_snapshot(&store, &format!("seat-{index:02}"), 90_000, &row).await;
        }
        let remaining = MAX_METRIC_STATE_BYTES - stored_bytes(store.clone()).await;
        let filler_samples = usize::try_from((remaining - 10_000) / 4_003).unwrap();
        insert_snapshot(&store, "filler", 90_000, &samples(filler_samples, 4_000)).await;
        let headroom = MAX_METRIC_STATE_BYTES - stored_bytes(store.clone()).await;
        assert!((1..20_000).contains(&headroom), "headroom {headroom}");
        let total_samples: i64 =
            sqlx::query_scalar("SELECT SUM(sample_count) FROM metric_snapshots")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(total_samples + 2 < MAX_METRIC_STATE_SAMPLES);

        let observation = |seat: &str, samples: Vec<String>| SeatObservation {
            guardian_seat_id: seat.to_owned(),
            federation_id: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_owned(),
            invite_code: None,
            observed_at_ms: 100_000,
            samples,
        };
        // seat-00 grows by more than the headroom; seat-01 is refreshed at its
        // stored size and comes after it.
        let mut grown = row.clone();
        grown.push("b".repeat(usize::try_from(headroom).unwrap()));
        assert_eq!(
            store
                .commit_metrics(
                    &work,
                    MetricsCommit {
                        listed_seats: None,
                        snapshots: vec![
                            observation("seat-00", grown),
                            observation("seat-01", row.clone()),
                        ],
                        complete: true,
                    },
                    100,
                )
                .await
                .unwrap(),
            MetricsCommitOutcome::Saturated
        );
        let observed: Vec<(String, i64)> = sqlx::query_as(
            "SELECT guardian_seat_id,observed_at_ms FROM metric_snapshots
             WHERE guardian_seat_id IN ('seat-00','seat-01') ORDER BY guardian_seat_id",
        )
        .fetch_all(&store.pool)
        .await
        .unwrap();
        assert_eq!(
            observed,
            vec![
                ("seat-00".to_owned(), 90_000),
                ("seat-01".to_owned(), 100_000)
            ]
        );
        assert!(stored_bytes(store.clone()).await <= MAX_METRIC_STATE_BYTES);
    }

    #[tokio::test]
    async fn saturated_commit_refreshes_existing_seats_and_skips_what_does_not_fit() {
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
        // Fill the store to 10,000 samples below the cap, plus seat "aa" with 2.
        let full_seats = MAX_METRIC_STATE_SAMPLES / 10_000 - 1;
        let mut listed: BTreeSet<String> = ["aa".to_owned(), "new".to_owned()].into();
        for index in 0..full_seats {
            let seat = format!("seat-{index:02}");
            insert_snapshot(&store, &seat, 90_000, &stored_peer_samples(&seat)).await;
            listed.insert(seat);
        }
        insert_snapshot(&store, "aa", 90_000, &stored_samples("aa", 1)).await;
        let headroom = 10_000 - 2;
        let observation = |seat: &str, samples: Vec<String>| SeatObservation {
            guardian_seat_id: seat.to_owned(),
            federation_id: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_owned(),
            invite_code: None,
            observed_at_ms: 100_000,
            samples,
        };
        // Either growth fits the headroom alone; both together do not. The new
        // seat comes first, so applying in arrival order would starve "aa".
        let grown = stored_counter_samples("aa", 5_999);
        let new_seat = stored_counter_samples("new", 5_999);
        assert!(grown.len() - 2 <= headroom && new_seat.len() <= headroom);
        assert!(grown.len() - 2 + new_seat.len() > headroom);

        assert_eq!(
            store
                .commit_metrics(
                    &work,
                    MetricsCommit {
                        listed_seats: Some(listed),
                        snapshots: vec![observation("new", new_seat), observation("aa", grown)],
                        complete: true,
                    },
                    100,
                )
                .await
                .unwrap(),
            MetricsCommitOutcome::Saturated
        );

        let stored: Vec<(String, i64, i64)> = sqlx::query_as(
            "SELECT guardian_seat_id,observed_at_ms,sample_count FROM metric_snapshots
             WHERE guardian_seat_id IN ('aa','new')",
        )
        .fetch_all(&store.pool)
        .await
        .unwrap();
        assert_eq!(stored, vec![("aa".to_owned(), 100_000, 6_000)]);
        let completed: Option<i64> =
            sqlx::query_scalar("SELECT last_complete_at FROM metric_poll_state")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(completed, None, "a saturated commit is not complete");
        let total: i64 = sqlx::query_scalar("SELECT SUM(sample_count) FROM metric_snapshots")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert!(total <= MAX_METRIC_STATE_SAMPLES);

        // A same-size refresh of a stored seat always fits, even near the cap.
        assert!(MAX_METRIC_STATE_SAMPLES - total < 6_000);
        assert_eq!(
            store
                .commit_metrics(
                    &work,
                    MetricsCommit {
                        listed_seats: None,
                        snapshots: vec![observation("aa", stored_counter_samples("aa", 5_999))],
                        complete: false,
                    },
                    100,
                )
                .await
                .unwrap(),
            MetricsCommitOutcome::Committed
        );
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
                            federation_id:
                                "0000000000000000000000000000000000000000000000000000000000000000"
                                    .to_owned(),
                            invite_code: Some("test-invite".to_owned()),
                            observed_at_ms: 100_000,
                            samples: stored_samples("aa", 1),
                        }],
                        complete: true,
                    },
                    100,
                )
                .await
                .unwrap(),
            MetricsCommitOutcome::Committed
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
            32,
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
            32,
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
            32,
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
            32,
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
            32,
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
        first.quarantine(target(1).mg_pubkey).await.unwrap();
        assert!(
            first
                .begin_collection_work(&scheduled, 100)
                .await
                .unwrap()
                .is_none()
        );
        first.reactivate(target(1).mg_pubkey).await.unwrap();
        drop(first);

        let restarted = Store::open(
            &data_dir(&directory).join("state.sqlite"),
            "development",
            SecretCipher::new(&[7; 32]),
            "test".into(),
            3600,
            32,
        )
        .await
        .unwrap();
        assert_eq!(restarted.next_metric_due_at(101).await.unwrap(), Some(1900));
        assert!(restarted.due_metric_targets(1899).await.unwrap().is_empty());
        assert_eq!(restarted.due_metric_targets(1900).await.unwrap().len(), 1);
    }
    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    const ZERO_FEDERATION: &str =
        "0000000000000000000000000000000000000000000000000000000000000000";

    /// A store whose one admitted target `11` stays leased for 30 days from
    /// `now`, with work ready to commit.
    async fn history_store(directory: &TempDir, now: i64) -> (Store, WorkTarget) {
        let data_dir = data_dir(directory);
        let _lock = crate::data_root_lock::DataRootLock::acquire(&data_dir).unwrap();
        let store = Store::open(
            &data_dir.join("state.sqlite"),
            "development",
            SecretCipher::new(&[7; 32]),
            "test".into(),
            30 * 24 * 60 * 60,
            32,
        )
        .await
        .unwrap();
        store
            .reserve_auth(&auth("history", now), now)
            .await
            .unwrap();
        store
            .admit(&auth("history", now), target(1), now)
            .await
            .unwrap();
        let scheduled = store
            .active_collection_targets(now)
            .await
            .unwrap()
            .remove(0);
        let work = store
            .begin_metric_work(&scheduled, now, 900)
            .await
            .unwrap()
            .unwrap();
        (store, work)
    }

    fn observation(seat: &str, observed_at_ms: i64, value: u64, invite: &str) -> SeatObservation {
        SeatObservation {
            guardian_seat_id: seat.to_owned(),
            federation_id: ZERO_FEDERATION.to_owned(),
            invite_code: Some(invite.to_owned()),
            observed_at_ms,
            samples: stored_samples(seat, value),
        }
    }

    /// Commit `observations` at their own time, with `seats` listed.
    async fn commit_at(
        store: &Store,
        work: &WorkTarget,
        seats: &[&str],
        observations: Vec<SeatObservation>,
        now_ms: i64,
    ) {
        assert_eq!(
            store
                .commit_metrics(
                    work,
                    MetricsCommit {
                        listed_seats: Some(seats.iter().map(|seat| (*seat).to_owned()).collect()),
                        snapshots: observations,
                        complete: true,
                    },
                    now_ms / 1000,
                )
                .await
                .unwrap(),
            MetricsCommitOutcome::Committed
        );
    }

    /// Stored observations as (seat, observed_at_ms, has invite), in order.
    async fn stored_observations(store: &Store) -> Vec<(String, i64, bool)> {
        sqlx::query_as(
            "SELECT guardian_seat_id,observed_at_ms,invite_ciphertext IS NOT NULL
             FROM metric_observations ORDER BY guardian_seat_id,observed_at_ms",
        )
        .fetch_all(&store.pool)
        .await
        .unwrap()
    }

    fn session_count(snapshot: &MetricsSnapshot) -> &str {
        snapshot
            .samples
            .iter()
            .find(|sample| sample.starts_with("fm_consensus_session_count"))
            .and_then(|sample| sample.rsplit_once(' '))
            .unwrap()
            .1
    }

    #[tokio::test]
    async fn history_keeps_the_last_observation_of_each_bucket_and_the_invite_only_on_the_latest() {
        let directory = tempfile::tempdir().unwrap();
        let (store, work) = history_store(&directory, 1).await;
        // 1.0 s and 1.1e3 s share the bucket [900 s, 1800 s); 1.9e3 s is in
        // the next one although it is less than 15 minutes after 1.1e3 s.
        for (observed_at_ms, value, invite) in [
            (1_000_000, 9, "invite-1"),
            (1_100_000, 10, "invite-2"),
            (1_900_000, 11, "invite-3"),
        ] {
            commit_at(
                &store,
                &work,
                &["aa"],
                vec![observation("aa", observed_at_ms, value, invite)],
                observed_at_ms,
            )
            .await;
        }
        assert_eq!(
            stored_observations(&store).await,
            vec![
                ("aa".to_owned(), 1_100_000, false),
                ("aa".to_owned(), 1_900_000, true),
            ]
        );
        let latest = store
            .metric_snapshots(&metrics_policy(), 2_000, 2_000_000)
            .await
            .unwrap();
        assert_eq!(latest.rejected, 0);
        assert_eq!(latest.snapshots.len(), 1);
        assert_eq!(latest.snapshots[0].observed_at_ms, 1_900_000);
        assert_eq!(session_count(&latest.snapshots[0]), "11");
        assert_eq!(
            store.exported_federations(2_000).await.unwrap()[0].invite_code,
            "invite-3"
        );

        let history = store
            .metric_history(&metrics_policy(), "11", "aa", i64::MAX, 2_000_000)
            .await
            .unwrap();
        assert_eq!(history.rejected, 0);
        assert_eq!(
            history
                .snapshots
                .iter()
                .map(|snapshot| (snapshot.observed_at_ms, session_count(snapshot)))
                .collect::<Vec<_>>(),
            vec![(1_100_000, "10"), (1_900_000, "11")]
        );
        let paged = store
            .metric_history(&metrics_policy(), "11", "aa", 1_900_000, 2_000_000)
            .await
            .unwrap();
        assert_eq!(
            paged
                .snapshots
                .iter()
                .map(|snapshot| snapshot.observed_at_ms)
                .collect::<Vec<_>>(),
            vec![1_100_000]
        );
        // Another MG's key or another seat reads nothing.
        for (mg_id, seat) in [("22", "aa"), ("11", "bb")] {
            let other = store
                .metric_history(&metrics_policy(), mg_id, seat, i64::MAX, 2_000_000)
                .await
                .unwrap();
            assert!(other.snapshots.is_empty() && other.rejected == 0);
        }
    }

    #[tokio::test]
    async fn an_observation_after_a_clock_step_back_replaces_the_later_dated_ones() {
        let directory = tempfile::tempdir().unwrap();
        let (store, work) = history_store(&directory, 1).await;
        for (observed_at_ms, value) in [(2_000_000, 1), (5_000_000, 2), (3_000_000, 3)] {
            commit_at(
                &store,
                &work,
                &["aa"],
                vec![observation("aa", observed_at_ms, value, "invite")],
                observed_at_ms,
            )
            .await;
        }
        assert_eq!(
            stored_observations(&store).await,
            vec![
                ("aa".to_owned(), 2_000_000, false),
                ("aa".to_owned(), 3_000_000, true),
            ]
        );
        let latest = store
            .metric_snapshots(&metrics_policy(), 3_000, 3_000_000)
            .await
            .unwrap();
        assert_eq!(session_count(&latest.snapshots[0]), "3");
    }

    #[tokio::test]
    async fn retention_prunes_week_old_history_but_keeps_every_seats_latest() {
        let directory = tempfile::tempdir().unwrap();
        let start = 10 * DAY_MS;
        let (store, work) = history_store(&directory, start / 1000).await;
        commit_at(
            &store,
            &work,
            &["aa", "bb"],
            vec![
                observation("aa", start, 1, "invite-a"),
                observation("bb", start, 1, "invite-b"),
            ],
            start,
        )
        .await;
        commit_at(
            &store,
            &work,
            &["aa", "bb"],
            vec![observation("aa", start + DAY_MS, 2, "invite-a")],
            start + DAY_MS,
        )
        .await;
        // One second (the commit clock's resolution) past 7 days after the
        // first commit, aa's first observation is past retention and its
        // second is not. bb's only one is its latest, so it stays.
        let now = start + 7 * DAY_MS + 1000;
        commit_at(
            &store,
            &work,
            &["aa", "bb"],
            vec![observation("aa", now, 3, "invite-a")],
            now,
        )
        .await;
        assert_eq!(
            stored_observations(&store).await,
            vec![
                ("aa".to_owned(), start + DAY_MS, false),
                ("aa".to_owned(), now, true),
                ("bb".to_owned(), start, true),
            ]
        );
        // The history read covers the same window.
        let history = store
            .metric_history(&metrics_policy(), "11", "bb", i64::MAX, now)
            .await
            .unwrap();
        assert!(history.snapshots.is_empty());
        let latest = store
            .metric_snapshots(&metrics_policy(), now / 1000, now)
            .await
            .unwrap();
        assert_eq!(latest.snapshots.len(), 2);

        // Unlisting a seat and quarantining the target remove their history.
        commit_at(&store, &work, &["aa"], Vec::new(), now).await;
        assert_eq!(
            stored_observations(&store)
                .await
                .into_iter()
                .map(|(seat, ..)| seat)
                .collect::<Vec<_>>(),
            ["aa", "aa"]
        );
        assert!(store.quarantine("11").await.unwrap());
        assert!(stored_observations(&store).await.is_empty());
        let history = store
            .metric_history(&metrics_policy(), "11", "aa", i64::MAX, now)
            .await
            .unwrap();
        assert!(history.snapshots.is_empty() && history.rejected == 0);
    }

    #[tokio::test]
    async fn history_bounds_prune_the_oldest_observations_that_are_not_latest() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _work) = history_store(&directory, 1).await;
        let bucket = METRIC_HISTORY_BUCKET_MS;
        for index in 0..6 {
            insert_snapshot(&store, "aa", index * bucket, &stored_samples("aa", 1)).await;
        }
        for index in 0..3 {
            insert_snapshot(&store, "bb", index * bucket, &stored_samples("bb", 1)).await;
        }
        let prune = |max_bytes, max_rows| {
            let store = store.clone();
            async move {
                let mut transaction = store.pool.begin().await.unwrap();
                prune_metric_history(&mut transaction, 0, max_bytes, max_rows)
                    .await
                    .unwrap();
                transaction.commit().await.unwrap();
                stored_observations(&store)
                    .await
                    .into_iter()
                    .map(|(seat, observed_at_ms, _)| (seat, observed_at_ms / bucket))
                    .collect::<Vec<_>>()
            }
        };
        let kept = |rows: &[(&str, i64)]| {
            rows.iter()
                .map(|(seat, index)| ((*seat).to_owned(), *index))
                .collect::<Vec<_>>()
        };
        // Under both bounds nothing goes.
        assert_eq!(prune(i64::MAX, 9).await.len(), 9);
        // Five rows: both latest, then the newest others, aa's before bb's
        // older one.
        assert_eq!(
            prune(i64::MAX, 5).await,
            kept(&[("aa", 2), ("aa", 3), ("aa", 4), ("aa", 5), ("bb", 2)])
        );
        // The byte bound counts compressed bytes the same way.
        let row_bytes: i64 =
            sqlx::query_scalar("SELECT MAX(length(samples_zstd)) FROM metric_observations")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(
            prune(3 * row_bytes, i64::MAX).await,
            kept(&[("aa", 4), ("aa", 5), ("bb", 2)])
        );
        // A bound below the latest snapshots never removes one.
        assert_eq!(prune(0, 0).await, kept(&[("aa", 5), ("bb", 2)]));
    }

    #[tokio::test]
    async fn history_read_skips_a_row_that_fails_revalidation() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _work) = history_store(&directory, 1).await;
        let bucket = METRIC_HISTORY_BUCKET_MS;
        insert_snapshot(&store, "aa", bucket, &stored_samples("aa", 1)).await;
        let foreign = stored_samples("bb", 2);
        insert_snapshot(&store, "aa", 2 * bucket, &foreign).await;
        insert_snapshot(&store, "aa", 3 * bucket, &stored_samples("aa", 3)).await;
        // Future-dated relative to the read.
        insert_snapshot(&store, "aa", 5 * bucket, &stored_samples("aa", 4)).await;
        let history = store
            .metric_history(&metrics_policy(), "11", "aa", i64::MAX, 4 * bucket)
            .await
            .unwrap();
        assert_eq!(history.rejected, 2);
        assert_eq!(
            history
                .snapshots
                .iter()
                .map(|snapshot| snapshot.observed_at_ms)
                .collect::<Vec<_>>(),
            vec![bucket, 3 * bucket]
        );
    }

    #[tokio::test]
    async fn migration_compresses_existing_latest_snapshots_without_loss() {
        let directory = tempfile::tempdir().unwrap();
        let (store, work) = history_store(&directory, 1).await;
        let mut long = stored_counter_samples("bb", 50);
        long.sort();
        commit_at(
            &store,
            &work,
            &["aa", "bb"],
            vec![
                observation("aa", 1_000_000, 7, "invite-a"),
                SeatObservation {
                    guardian_seat_id: "bb".to_owned(),
                    federation_id: ZERO_FEDERATION.to_owned(),
                    invite_code: None,
                    observed_at_ms: 1_100_000,
                    samples: long,
                },
            ],
            1_100_000,
        )
        .await;
        let before = store
            .metric_snapshots(&metrics_policy(), 1_200, 1_200_000)
            .await
            .unwrap();
        let federations = store.exported_federations(1_200).await.unwrap();
        assert_eq!(federations.len(), 1);

        // Put the database back at migration 7: the latest snapshots in the
        // uncompressed table that migrations 0004 and 0005 define, the one as
        // TEXT as a hand-edited row could hold it.
        let rows: Vec<(
            String,
            String,
            String,
            i64,
            Vec<u8>,
            i64,
            Option<Vec<u8>>,
            Option<Vec<u8>>,
        )> = sqlx::query_as(
            "SELECT target_id,guardian_seat_id,federation_id,observed_at_ms,samples_zstd,
                        sample_count,invite_nonce,invite_ciphertext
                 FROM metric_snapshots",
        )
        .fetch_all(&store.pool)
        .await
        .unwrap();
        let mut connection = store.pool.acquire().await.unwrap();
        sqlx::raw_sql(
            "DROP VIEW metric_snapshots;
             DROP TABLE metric_observations;
             DELETE FROM _sqlx_migrations WHERE version = 8;
             CREATE TABLE metric_snapshots (placeholder INTEGER);",
        )
        .execute(&mut *connection)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../migrations/0004_metric_snapshot_federation.sql"
        ))
        .execute(&mut *connection)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../migrations/0005_metric_snapshot_invite.sql"
        ))
        .execute(&mut *connection)
        .await
        .unwrap();
        for (target_id, seat, federation_id, observed_at_ms, samples_zstd, count, nonce, invite) in
            rows
        {
            let samples_json = zstd::bulk::decompress(&samples_zstd, MAX_METRIC_ROW_BYTES).unwrap();
            let insert = if seat == "bb" {
                "INSERT INTO metric_snapshots VALUES (?,?,?,?,CAST(? AS TEXT),?,?,?)"
            } else {
                "INSERT INTO metric_snapshots VALUES (?,?,?,?,?,?,?,?)"
            };
            sqlx::query(insert)
                .bind(target_id)
                .bind(seat)
                .bind(federation_id)
                .bind(observed_at_ms)
                .bind(samples_json)
                .bind(count)
                .bind(nonce)
                .bind(invite)
                .execute(&mut *connection)
                .await
                .unwrap();
        }
        let text_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM metric_snapshots WHERE typeof(samples_json)='text'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        assert_eq!(text_rows, 1);
        drop(connection);
        drop(store);

        let (migrated, _work) = {
            let data_dir = data_dir(&directory);
            let _lock = crate::data_root_lock::DataRootLock::acquire(&data_dir).unwrap();
            let store = Store::open(
                &data_dir.join("state.sqlite"),
                "development",
                SecretCipher::new(&[7; 32]),
                "test".into(),
                30 * 24 * 60 * 60,
                32,
            )
            .await
            .unwrap();
            (store, ())
        };
        let after = migrated
            .metric_snapshots(&metrics_policy(), 1_200, 1_200_000)
            .await
            .unwrap();
        assert_eq!(after.rejected, 0);
        assert_eq!(
            after
                .snapshots
                .iter()
                .map(|snapshot| (
                    &snapshot.guardian_seat_id,
                    snapshot.observed_at_ms,
                    &snapshot.samples
                ))
                .collect::<Vec<_>>(),
            before
                .snapshots
                .iter()
                .map(|snapshot| (
                    &snapshot.guardian_seat_id,
                    snapshot.observed_at_ms,
                    &snapshot.samples
                ))
                .collect::<Vec<_>>()
        );
        assert_eq!(after.snapshots.len(), 2);
        assert_eq!(
            migrated.exported_federations(1_200).await.unwrap(),
            federations
        );
        let legacy: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE name='metric_snapshots_uncompressed'",
        )
        .fetch_optional(&migrated.pool)
        .await
        .unwrap();
        assert!(legacy.is_none());
        let sizes: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT samples_json_bytes,length(samples_zstd) FROM metric_observations
             ORDER BY guardian_seat_id",
        )
        .fetch_all(&migrated.pool)
        .await
        .unwrap();
        for ((json_bytes, zstd_bytes), snapshot) in sizes.iter().zip(&before.snapshots) {
            assert_eq!(
                *json_bytes,
                i64::try_from(serde_json::to_vec(&snapshot.samples).unwrap().len()).unwrap()
            );
            assert!(zstd_bytes < json_bytes);
        }
    }
}
