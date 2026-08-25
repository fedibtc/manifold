use std::collections::BTreeSet;

use serde::Serialize;
use sqlx::{AnyPool, Row};

use fedi_decentralized_push_gateway_types::{
    DeviceInstallationId, FcmRegistrationToken, HookId, Notification, Platform, PushRegistration,
    RecipientId, random_url_token,
};

use crate::{DatabaseBackend, log_sanitizer::sanitize_log_value, time::unix_timestamp};

/// Maximum number of provider attempts before a transient failure is dead-lettered.
pub const MAX_DELIVERY_ATTEMPTS: i64 = 5;
/// Maximum elapsed time from durable enqueue to a terminal delivery outcome.
///
/// The worker derives this deadline from the durable `created_at` timestamp, so
/// a restart cannot reset it. A row that misses the deadline becomes the
/// actionable `dead_letter` outcome with reason `resolution_deadline_exceeded`.
pub const DELIVERY_RESOLUTION_DEADLINE_SECONDS: i64 = 300;
/// Seconds before an interrupted in-progress delivery can be reclaimed.
pub const IN_PROGRESS_LEASE_SECONDS: i64 = 300;

/// Persistent delivery outbox repository.
#[derive(Clone, Debug)]
pub struct DeliveryOutboxRepository {
    /// Database pool used by repository operations.
    pool: AnyPool,
    /// Backend used to evaluate deadline predicates at statement execution time.
    backend: DatabaseBackend,
    /// Deterministic statement-time clock used only by repository unit tests.
    #[cfg(test)]
    database_now_cte_override: Option<&'static str>,
}

/// Result of durably recording one notification event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnqueueOutcome {
    /// Durable event id used for delivery rows.
    pub event_id: String,
    /// Number of target registrations represented by outbox rows.
    pub target_count: usize,
    /// True when this call inserted a new event; false for idempotent replay.
    pub inserted: bool,
}

/// One claimed delivery row ready to send to the provider.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimedDelivery {
    /// Outbox row identifier.
    pub outbox_id: String,
    /// Target push registration snapshot.
    pub registration: PushRegistration,
    /// Notification snapshot to send.
    pub notification: Notification,
    /// Claim fencing token for this in-progress attempt.
    pub claim_id: String,
    /// Durable acceptance time used for the absolute resolution deadline.
    pub created_at: i64,
}

/// Queue-depth counters grouped by delivery outbox status.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct OutboxStatusCounts {
    /// Rows ready for initial delivery.
    pub pending: i64,
    /// Rows currently claimed by this process.
    pub in_progress: i64,
    /// Rows waiting for transient retry backoff.
    pub retrying: i64,
    /// Rows delivered successfully.
    pub succeeded: i64,
    /// Rows terminally failed because the token was invalid.
    pub invalid_token: i64,
    /// Rows terminally failed after transient retry exhaustion or a permanent payload failure.
    pub dead_letter: i64,
}

/// Maximum number of dead-letter rows one admin selector may target.
pub const MAX_DEAD_LETTER_ADMIN_SELECTION: i64 = 1_000;

/// Sanitized delivery outbox row metadata for operator/admin inspection.
///
/// This intentionally omits FCM tokens and notification JSON because both can
/// contain sensitive bearer material or user-visible notification content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OutboxAdminRow {
    /// Outbox row identifier.
    pub outbox_id: String,
    /// Durable notification event id.
    pub event_id: String,
    /// Recipient identifier.
    pub recipient_id: String,
    /// Device installation identifier.
    pub installation_id: String,
    /// Optional platform label.
    pub platform: Option<String>,
    /// Current delivery status.
    pub status: String,
    /// Number of attempted provider sends.
    pub attempts: i64,
    /// Sanitized last error reason, if any.
    pub last_error: Option<String>,
    /// Next attempt timestamp.
    pub next_attempt_at: i64,
    /// Last attempt timestamp, if any.
    pub last_attempt_at: Option<i64>,
    /// Row creation timestamp.
    pub created_at: i64,
    /// Last row update timestamp.
    pub updated_at: i64,
}

/// Aggregate count for a sanitized dead-letter reason.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OutboxDeadLetterReasonCount {
    /// Sanitized last-error reason, or `unknown` when absent.
    pub reason: String,
    /// Number of dead-letter rows with this reason.
    pub count: i64,
}

/// Bounded selector for dead-letter administrative mutations.
///
/// Explicit `outbox_ids` take precedence over `limit`. When ids are supplied,
/// `reason`, if present, filters the matching id rows. Repository methods reject
/// selectors with too many ids, duplicate ids, or out-of-range limits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboxDeadLetterSelector {
    /// Explicit outbox ids to select.
    pub outbox_ids: Vec<String>,
    /// Optional upper bound for oldest dead-letter rows when ids are not supplied.
    pub limit: Option<i64>,
    /// Optional sanitized reason filter.
    pub reason: Option<String>,
}

/// Operational outbox gauges derived from persisted delivery rows.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct OutboxOperationalMetrics {
    /// Queue-depth counters grouped by status.
    pub status_counts: OutboxStatusCounts,
    /// Age in seconds of the oldest due row among pending/retrying/reclaimable rows.
    pub oldest_due_age_seconds: i64,
    /// Age in seconds of the oldest pending row.
    pub oldest_pending_age_seconds: i64,
    /// Age in seconds since the oldest retrying row was last updated.
    pub oldest_retrying_age_seconds: i64,
    /// Current number of dead-letter rows.
    pub dead_letter_current: i64,
    /// Total retained dead-letter rows.
    pub dead_letter_total: i64,
}

/// Counts of sensitive terminal rows purged by retention cleanup.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RetentionPurgeCounts {
    /// Terminal delivery rows deleted from `delivery_outbox`.
    pub delivery_outbox_rows: u64,
    /// Old disabled registration rows deleted from `push_registrations`.
    pub disabled_registration_rows: u64,
    /// Old notification events left without retained outbox rows.
    pub notification_event_rows: u64,
    /// Expired compact accepted-idempotency markers.
    pub idempotency_tombstone_rows: u64,
}

/// Non-token delivery failure that can be recorded on an outbox row without
/// touching push registration state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeliveryOutboxFailure {
    /// Static sanitized reason code.
    reason: &'static str,
    /// Outbox-only failure classification.
    kind: DeliveryOutboxFailureKind,
}

/// Result of claiming due outbox work.
#[derive(Clone, Debug, PartialEq)]
pub enum ClaimDueOutcome {
    /// A row was claimed and decoded for provider delivery.
    Claimed(Box<ClaimedDelivery>),
    /// No claimable row was available.
    Empty,
    /// A row was claimed but had corrupted notification JSON and was dead-lettered.
    CorruptedDeadLetter,
}

/// Result of recording a non-token delivery failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkFailedOutcome {
    /// The claim fence did not match an in-progress row.
    NotUpdated,
    /// The row was updated to wait for a retry.
    Retrying,
    /// The row was updated to terminal dead-letter.
    DeadLettered,
}

impl DeliveryOutboxFailure {
    /// Creates a storage-owned failure input.
    #[must_use]
    pub fn new(reason: &'static str, kind: DeliveryOutboxFailureKind) -> Self {
        Self { reason, kind }
    }

    /// Creates a permanent-payload failure.
    #[must_use]
    pub fn permanent_payload(reason: &'static str) -> Self {
        Self::new(reason, DeliveryOutboxFailureKind::PermanentPayload)
    }

    /// Creates a transient failure.
    #[must_use]
    pub fn transient(reason: &'static str) -> Self {
        Self::new(reason, DeliveryOutboxFailureKind::Transient)
    }
}

/// Outbox-only failure class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryOutboxFailureKind {
    /// The notification payload is permanently invalid, but the token is not.
    PermanentPayload,
    /// The provider, quota, or network failure may succeed later.
    Transient,
}

impl DeliveryOutboxRepository {
    /// Creates a delivery outbox repository from a database pool.
    #[must_use]
    pub fn new(pool: AnyPool, backend: DatabaseBackend) -> Self {
        Self {
            pool,
            backend,
            #[cfg(test)]
            database_now_cte_override: None,
        }
    }

    /// Overrides the database clock CTE for a deterministic unit test.
    #[cfg(test)]
    #[must_use]
    fn with_database_now_cte_for_test(mut self, cte: &'static str) -> Self {
        self.database_now_cte_override = Some(cte);
        self
    }

    /// Resets interrupted in-process deliveries so startup can retry them.
    pub async fn reset_in_progress(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE delivery_outbox
             SET status = 'pending', updated_at = $1
             WHERE status = 'in_progress'",
        )
        .bind(unix_timestamp())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Terminally resolves active rows that exceeded their absolute deadline.
    ///
    /// This intentionally does not increment `attempts`: the transition is a
    /// scheduler deadline, not another provider send. The deadline is derived
    /// from `created_at`, rather than from process-local startup time, so
    /// restarting the worker cannot extend accepted work indefinitely.
    pub async fn expire_delivery_resolution_deadlines(&self) -> Result<u64, sqlx::Error> {
        let now = unix_timestamp();
        let result = sqlx::query(&format!(
            "{} UPDATE delivery_outbox
             SET status = 'dead_letter',
                 next_attempt_at = $1,
                 updated_at = $1,
                 last_error = 'resolution_deadline_exceeded',
                 claim_id = NULL
             WHERE status IN ('pending', 'retrying', 'in_progress')
                AND created_at <= ((SELECT epoch FROM database_now) - $2)",
            self.database_now_cte()
        ))
        .bind(now)
        .bind(DELIVERY_RESOLUTION_DEADLINE_SECONDS)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Enqueues an event and one outbox row per target registration.
    pub async fn enqueue_event(
        &self,
        hook_id: &HookId,
        caller_idempotency_key: Option<&str>,
        notification: &Notification,
        registrations: &[PushRegistration],
    ) -> Result<EnqueueOutcome, sqlx::Error> {
        let now = unix_timestamp();
        let existing = if let Some(caller_idempotency_key) = caller_idempotency_key {
            self.find_event(hook_id, caller_idempotency_key).await?
        } else {
            None
        };
        if let Some((event_id, target_count)) = existing {
            return Ok(EnqueueOutcome {
                event_id,
                target_count: target_count.try_into().unwrap_or(0),
                inserted: false,
            });
        }

        let event_id = random_url_token(18);
        let notification_json = notification_to_json(notification)?;
        let mut transaction = self.pool.begin().await?;
        let insert_result = sqlx::query(
            "INSERT INTO notification_events (
                 event_id, hook_id, caller_idempotency_key, recipient_id, notification_json,
                 target_count, created_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT(hook_id, caller_idempotency_key) DO NOTHING",
        )
        .bind(&event_id)
        .bind(&hook_id.0)
        .bind(caller_idempotency_key)
        .bind(&notification.recipient_id.0)
        .bind(&notification_json)
        .bind(i64::try_from(registrations.len()).unwrap_or(i64::MAX))
        .bind(now)
        .execute(&mut *transaction)
        .await?;

        if insert_result.rows_affected() == 0 {
            transaction.rollback().await?;
            if let Some(caller_idempotency_key) = caller_idempotency_key
                && let Some((event_id, target_count)) =
                    self.find_event(hook_id, caller_idempotency_key).await?
            {
                return Ok(EnqueueOutcome {
                    event_id,
                    target_count: target_count.try_into().unwrap_or(0),
                    inserted: false,
                });
            }
            return Ok(EnqueueOutcome {
                event_id,
                target_count: 0,
                inserted: false,
            });
        }

        for registration in registrations {
            sqlx::query(
                "INSERT INTO delivery_outbox (
                     outbox_id, event_id, recipient_id, installation_id, fcm_token, platform,
                     notification_json, status, attempts, next_attempt_at, created_at, updated_at
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending', 0, $8, $8, $8)",
            )
            .bind(random_url_token(18))
            .bind(&event_id)
            .bind(&registration.recipient_id.0)
            .bind(&registration.installation_id.0)
            .bind(&registration.fcm_token.0)
            .bind(
                registration
                    .platform
                    .as_ref()
                    .map(|platform| platform.0.as_str()),
            )
            .bind(&notification_json)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(EnqueueOutcome {
            event_id,
            target_count: registrations.len(),
            inserted: true,
        })
    }

    /// Finds an already-recorded caller event for one hook idempotency key.
    pub async fn find_existing_event(
        &self,
        hook_id: &HookId,
        caller_idempotency_key: &str,
    ) -> Result<Option<EnqueueOutcome>, sqlx::Error> {
        Ok(self.find_event(hook_id, caller_idempotency_key).await?.map(
            |(event_id, target_count)| EnqueueOutcome {
                event_id,
                target_count: target_count.try_into().unwrap_or(0),
                inserted: false,
            },
        ))
    }

    /// Claims one due delivery row for worker processing.
    ///
    /// If a claimed row contains corrupted `notification_json`, the row is
    /// terminally marked `dead_letter` with
    /// `last_error = "notification_json_invalid"` and this method returns
    /// [`ClaimDueOutcome::CorruptedDeadLetter`] instead of constructing a fallback
    /// notification.
    pub async fn claim_due(&self) -> Result<ClaimDueOutcome, sqlx::Error> {
        let now = unix_timestamp();
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT outbox_id, recipient_id, installation_id, fcm_token, platform,
                    notification_json, created_at
             FROM delivery_outbox
             WHERE (
                 status IN ('pending', 'retrying')
                 OR (status = 'in_progress' AND next_attempt_at <= $1)
             )
             AND next_attempt_at <= $1
             ORDER BY next_attempt_at ASC, created_at ASC
             LIMIT 1",
        )
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(row) = row else {
            transaction.commit().await?;
            return Ok(ClaimDueOutcome::Empty);
        };
        let outbox_id: String = row.get("outbox_id");
        let claim_id = random_url_token(18);
        let result = sqlx::query(
            "UPDATE delivery_outbox
             SET status = 'in_progress', claim_id = $4, next_attempt_at = $3, updated_at = $2
             WHERE outbox_id = $1
               AND (
                   status IN ('pending', 'retrying')
                   OR (status = 'in_progress' AND next_attempt_at <= $2)
               )",
        )
        .bind(&outbox_id)
        .bind(now)
        .bind(now + IN_PROGRESS_LEASE_SECONDS)
        .bind(&claim_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        if result.rows_affected() == 0 {
            return Ok(ClaimDueOutcome::Empty);
        }
        match row_to_claimed(outbox_id.clone(), claim_id.clone(), row) {
            Ok(claimed) => Ok(ClaimDueOutcome::Claimed(Box::new(claimed))),
            Err(err) => {
                let dead_letter_result = self
                    .mark_corrupted_notification_dead_letter(&outbox_id, &claim_id)
                    .await;
                match dead_letter_result {
                    Ok(true) => {
                        eprintln!(
                            "event=outbox_notification_decode_failure outbox_id={} action=dead_letter error={}",
                            sanitize_log_value(&outbox_id),
                            sanitize_log_value(&err.to_string())
                        );
                        return Ok(ClaimDueOutcome::CorruptedDeadLetter);
                    }
                    Ok(false) => eprintln!(
                        "event=outbox_notification_decode_failure outbox_id={} action=dead_letter_skipped error={}",
                        sanitize_log_value(&outbox_id),
                        sanitize_log_value(&err.to_string())
                    ),
                    Err(mark_err) => eprintln!(
                        "event=outbox_notification_decode_failure outbox_id={} action=dead_letter_failed error={}",
                        sanitize_log_value(&outbox_id),
                        sanitize_log_value(&mark_err.to_string())
                    ),
                }
                Err(sqlx::Error::Decode(Box::new(err)))
            }
        }
    }

    /// Returns the nearest time at which a pending, retrying, or reclaimable
    /// in-progress row should be claimable.
    pub async fn next_claim_due_at(&self) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT MIN(next_attempt_at)
             FROM delivery_outbox
             WHERE status IN ('pending', 'retrying', 'in_progress')",
        )
        .fetch_one(&self.pool)
        .await
    }

    /// Marks a claimed delivery as successfully sent.
    pub async fn mark_succeeded(
        &self,
        outbox_id: &str,
        claim_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let now = unix_timestamp();
        let result = sqlx::query(&format!(
            "{} UPDATE delivery_outbox
             SET status = 'succeeded', claim_id = NULL, last_attempt_at = $2, updated_at = $2, last_error = NULL
             WHERE outbox_id = $1
                AND claim_id = $3
                AND status = 'in_progress'
                AND created_at > ((SELECT epoch FROM database_now) - $4)",
            self.database_now_cte()
        ))
        .bind(outbox_id)
        .bind(now)
        .bind(claim_id)
        .bind(DELIVERY_RESOLUTION_DEADLINE_SECONDS)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Marks a claimed delivery according to a sanitized non-token provider error.
    ///
    /// Permanent-token errors must use
    /// [`Self::mark_invalid_token_and_disable_registration`] so the terminal
    /// outbox update and matching registration disable happen atomically.
    pub async fn mark_failed(
        &self,
        outbox_id: &str,
        claim_id: &str,
        error: &DeliveryOutboxFailure,
    ) -> Result<MarkFailedOutcome, sqlx::Error> {
        let now = unix_timestamp();
        let (status, next_attempt_at, last_error) = match error.kind {
            DeliveryOutboxFailureKind::PermanentPayload => ("dead_letter", now, error.reason),
            DeliveryOutboxFailureKind::Transient => {
                let row = sqlx::query(
                    "SELECT attempts
                     FROM delivery_outbox
                     WHERE outbox_id = $1",
                )
                .bind(outbox_id)
                .fetch_one(&self.pool)
                .await?;
                let attempts: i64 = row.get("attempts");
                if attempts + 1 >= MAX_DELIVERY_ATTEMPTS {
                    ("dead_letter", now, error.reason)
                } else {
                    (
                        "retrying",
                        now + backoff_seconds(attempts + 1),
                        error.reason,
                    )
                }
            }
        };
        let result = sqlx::query(&mark_failed_query(self.database_now_cte()))
            .bind(outbox_id)
            .bind(status)
            .bind(next_attempt_at)
            .bind(now)
            .bind(last_error)
            .bind(claim_id)
            .bind(DELIVERY_RESOLUTION_DEADLINE_SECONDS)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            Ok(MarkFailedOutcome::NotUpdated)
        } else {
            let status: String =
                sqlx::query_scalar("SELECT status FROM delivery_outbox WHERE outbox_id = $1")
                    .bind(outbox_id)
                    .fetch_one(&self.pool)
                    .await?;
            if status == "dead_letter" {
                Ok(MarkFailedOutcome::DeadLettered)
            } else {
                Ok(MarkFailedOutcome::Retrying)
            }
        }
    }

    /// Atomically marks a claimed delivery as invalid-token and disables the
    /// registration only if its current token still matches the outbox snapshot.
    ///
    /// Returns `Ok(true)` when the claimed outbox row was marked invalid-token.
    /// A concurrent token rotation can leave the registration row enabled while
    /// still returning `Ok(true)`.
    pub async fn mark_invalid_token_and_disable_registration(
        &self,
        delivery: &ClaimedDelivery,
        reason: &'static str,
    ) -> Result<bool, sqlx::Error> {
        let now = unix_timestamp();
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE delivery_outbox
             SET status = 'invalid_token',
                 attempts = attempts + 1,
                 next_attempt_at = $2,
                 last_attempt_at = $2,
                 updated_at = $2,
                 last_error = $3,
                 claim_id = NULL
             WHERE outbox_id = $1 AND claim_id = $4 AND status = 'in_progress'",
        )
        .bind(&delivery.outbox_id)
        .bind(now)
        .bind(reason)
        .bind(&delivery.claim_id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() == 0 {
            transaction.commit().await?;
            return Ok(false);
        }

        sqlx::query(
            "UPDATE push_registrations
             SET disabled_at = COALESCE(disabled_at, $4),
                 disabled_reason = COALESCE(disabled_reason, $5)
             WHERE recipient_id = $1 AND installation_id = $2 AND fcm_token = $3",
        )
        .bind(&delivery.registration.recipient_id.0)
        .bind(&delivery.registration.installation_id.0)
        .bind(&delivery.registration.fcm_token.0)
        .bind(now)
        .bind(Some(reason))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    async fn mark_corrupted_notification_dead_letter(
        &self,
        outbox_id: &str,
        claim_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let now = unix_timestamp();
        let result = sqlx::query(
            "UPDATE delivery_outbox
             SET status = 'dead_letter',
                 attempts = attempts + 1,
                 next_attempt_at = $2,
                 last_attempt_at = $2,
                 updated_at = $2,
                 last_error = 'notification_json_invalid',
                 claim_id = NULL
             WHERE outbox_id = $1 AND claim_id = $3 AND status = 'in_progress'",
        )
        .bind(outbox_id)
        .bind(now)
        .bind(claim_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Counts outbox rows in a given status, for tests and local diagnostics.
    pub async fn count_by_status(&self, status: &str) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM delivery_outbox WHERE status = $1")
            .bind(status)
            .fetch_one(&self.pool)
            .await
    }

    /// Counts delivery outbox rows by all known statuses.
    pub async fn status_counts(&self) -> Result<OutboxStatusCounts, sqlx::Error> {
        let rows =
            sqlx::query("SELECT status, COUNT(*) AS count FROM delivery_outbox GROUP BY status")
                .fetch_all(&self.pool)
                .await?;
        let mut counts = OutboxStatusCounts::default();
        for row in rows {
            let status: String = row.get("status");
            let count: i64 = row.get("count");
            match status.as_str() {
                "pending" => counts.pending = count,
                "in_progress" => counts.in_progress = count,
                "retrying" => counts.retrying = count,
                "succeeded" => counts.succeeded = count,
                "invalid_token" => counts.invalid_token = count,
                "dead_letter" => counts.dead_letter = count,
                _ => {}
            }
        }
        Ok(counts)
    }

    /// Lists sanitized dead-letter rows, ordered oldest first, with a hard limit.
    pub async fn list_dead_letter_rows(
        &self,
        limit: i64,
    ) -> Result<Vec<OutboxAdminRow>, sqlx::Error> {
        validate_admin_limit(limit)?;
        let rows = sqlx::query(
            "SELECT outbox_id, event_id, recipient_id, installation_id, platform,
                    status, attempts, last_error, next_attempt_at, last_attempt_at,
                    created_at, updated_at
             FROM delivery_outbox
             WHERE status = 'dead_letter'
             ORDER BY updated_at ASC, created_at ASC, outbox_id ASC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_admin_row).collect())
    }

    /// Counts dead-letter rows grouped by sanitized last-error reason.
    pub async fn dead_letter_reason_counts(
        &self,
    ) -> Result<Vec<OutboxDeadLetterReasonCount>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT COALESCE(last_error, 'unknown') AS reason, COUNT(*) AS count
             FROM delivery_outbox
             WHERE status = 'dead_letter'
             GROUP BY COALESCE(last_error, 'unknown')
             ORDER BY count DESC, reason ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| OutboxDeadLetterReasonCount {
                reason: row.get("reason"),
                count: row.get("count"),
            })
            .collect())
    }

    /// Returns sanitized dead-letter rows selected for an administrative mutation.
    pub async fn select_dead_letter_rows(
        &self,
        selector: &OutboxDeadLetterSelector,
    ) -> Result<Vec<OutboxAdminRow>, sqlx::Error> {
        validate_dead_letter_selector(selector)?;
        if !selector.outbox_ids.is_empty() {
            let mut rows = Vec::with_capacity(selector.outbox_ids.len());
            for outbox_id in &selector.outbox_ids {
                if let Some(row) = sqlx::query(
                    "SELECT outbox_id, event_id, recipient_id, installation_id, platform,
                            status, attempts, last_error, next_attempt_at, last_attempt_at,
                            created_at, updated_at
                     FROM delivery_outbox
                     WHERE status = 'dead_letter' AND outbox_id = $1",
                )
                .bind(outbox_id)
                .fetch_optional(&self.pool)
                .await?
                {
                    let admin_row = row_to_admin_row(row);
                    if selector
                        .reason
                        .as_deref()
                        .is_none_or(|reason| admin_row.last_error.as_deref() == Some(reason))
                    {
                        rows.push(admin_row);
                    }
                }
            }
            return Ok(rows);
        }

        let Some(limit) = selector.limit else {
            return Ok(Vec::new());
        };
        if let Some(reason) = &selector.reason {
            let rows = sqlx::query(
                "SELECT outbox_id, event_id, recipient_id, installation_id, platform,
                        status, attempts, last_error, next_attempt_at, last_attempt_at,
                        created_at, updated_at
                 FROM delivery_outbox
                 WHERE status = 'dead_letter' AND last_error = $1
                 ORDER BY updated_at ASC, created_at ASC, outbox_id ASC
                 LIMIT $2",
            )
            .bind(reason)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
            Ok(rows.into_iter().map(row_to_admin_row).collect())
        } else {
            self.list_dead_letter_rows(limit).await
        }
    }

    /// Replays selected dead-letter rows by moving them back to `pending`.
    pub async fn replay_dead_letter_rows(
        &self,
        selector: &OutboxDeadLetterSelector,
        dry_run: bool,
    ) -> Result<u64, sqlx::Error> {
        let rows = self.select_dead_letter_rows(selector).await?;
        let now = unix_timestamp();
        if rows.is_empty() {
            return Ok(rows.len().try_into().unwrap_or(u64::MAX));
        }
        let mut transaction = self.pool.begin().await?;
        let mut changed = 0;
        for row in rows {
            let result = sqlx::query(&format!(
                "{} UPDATE delivery_outbox
                  SET status = 'pending',
                      claim_id = NULL,
                      next_attempt_at = $2,
                      updated_at = $2
                  WHERE outbox_id = $1
                    AND status = 'dead_letter'
                    AND created_at > ((SELECT epoch FROM database_now) - $3)",
                self.database_now_cte()
            ))
            .bind(&row.outbox_id)
            .bind(now)
            .bind(DELIVERY_RESOLUTION_DEADLINE_SECONDS)
            .execute(&mut *transaction)
            .await?;
            if result.rows_affected() == 0 {
                transaction.rollback().await?;
                return Err(replay_error(
                    "dead-letter row is past its delivery resolution deadline and cannot be replayed",
                ));
            }
            changed += result.rows_affected();
        }
        if dry_run {
            transaction.rollback().await?;
        } else {
            transaction.commit().await?;
        }
        Ok(changed)
    }

    /// Permanently deletes selected dead-letter rows.
    pub async fn delete_dead_letter_rows(
        &self,
        selector: &OutboxDeadLetterSelector,
        dry_run: bool,
    ) -> Result<u64, sqlx::Error> {
        let rows = self.select_dead_letter_rows(selector).await?;
        if dry_run || rows.is_empty() {
            return Ok(rows.len().try_into().unwrap_or(u64::MAX));
        }
        let mut transaction = self.pool.begin().await?;
        let mut changed = 0;
        for row in rows {
            let result = sqlx::query(
                "DELETE FROM delivery_outbox WHERE outbox_id = $1 AND status = 'dead_letter'",
            )
            .bind(&row.outbox_id)
            .execute(&mut *transaction)
            .await?;
            changed += result.rows_affected();
        }
        transaction.commit().await?;
        Ok(changed)
    }

    /// Returns low-cardinality operational metrics backed by delivery outbox state.
    pub async fn operational_metrics(&self) -> Result<OutboxOperationalMetrics, sqlx::Error> {
        let now = unix_timestamp();
        let status_counts = self.status_counts().await?;
        let oldest_due_at: Option<i64> = sqlx::query_scalar(
            "SELECT MIN(next_attempt_at)
             FROM delivery_outbox
             WHERE status IN ('pending', 'retrying', 'in_progress') AND next_attempt_at <= $1",
        )
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        let oldest_pending_created_at: Option<i64> = sqlx::query_scalar(
            "SELECT MIN(created_at) FROM delivery_outbox WHERE status = 'pending'",
        )
        .fetch_one(&self.pool)
        .await?;
        let oldest_retrying_updated_at: Option<i64> = sqlx::query_scalar(
            "SELECT MIN(updated_at) FROM delivery_outbox WHERE status = 'retrying'",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(OutboxOperationalMetrics {
            dead_letter_current: status_counts.dead_letter,
            dead_letter_total: status_counts.dead_letter,
            status_counts,
            oldest_due_age_seconds: age_seconds(now, oldest_due_at),
            oldest_pending_age_seconds: age_seconds(now, oldest_pending_created_at),
            oldest_retrying_age_seconds: age_seconds(now, oldest_retrying_updated_at),
        })
    }

    /// Purges sensitive terminal push data older than `cutoff_timestamp`.
    ///
    /// The cleanup is intentionally conservative: it deletes only terminal outbox
    /// rows (`succeeded`, `invalid_token`, `dead_letter`) whose terminal
    /// `updated_at` is older than the cutoff, disabled registrations whose
    /// `disabled_at` is older than the cutoff, and notification events older than
    /// the cutoff after all of their outbox rows have been removed. Pending,
    /// retrying, and in-progress delivery state is never purged.
    pub async fn purge_retained_sensitive_data(
        &self,
        cutoff_timestamp: i64,
        idempotency_cutoff_timestamp: i64,
    ) -> Result<RetentionPurgeCounts, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let delivery_outbox_rows = sqlx::query(
            "DELETE FROM delivery_outbox
             WHERE status IN ('succeeded', 'invalid_token', 'dead_letter')
               AND updated_at < $1",
        )
        .bind(cutoff_timestamp)
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        let disabled_registration_rows = sqlx::query(
            "DELETE FROM push_registrations
             WHERE disabled_at IS NOT NULL AND disabled_at < $1",
        )
        .bind(cutoff_timestamp)
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        let notification_event_rows = sqlx::query(
            "DELETE FROM notification_events
             WHERE created_at < $1
               AND NOT EXISTS (
                   SELECT 1 FROM delivery_outbox
                   WHERE delivery_outbox.event_id = notification_events.event_id
               )",
        )
        .bind(cutoff_timestamp)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        let idempotency_tombstone_rows =
            sqlx::query("DELETE FROM hook_idempotency_tombstones WHERE retain_until < $1")
                .bind(idempotency_cutoff_timestamp)
                .execute(&mut *transaction)
                .await?
                .rows_affected();
        transaction.commit().await?;

        Ok(RetentionPurgeCounts {
            delivery_outbox_rows,
            disabled_registration_rows,
            notification_event_rows,
            idempotency_tombstone_rows,
        })
    }

    async fn find_event(
        &self,
        hook_id: &HookId,
        caller_idempotency_key: &str,
    ) -> Result<Option<(String, i64)>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT event_id, target_count FROM notification_events
             WHERE hook_id = $1 AND caller_idempotency_key = $2",
        )
        .bind(&hook_id.0)
        .bind(caller_idempotency_key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| (row.get("event_id"), row.get("target_count"))))
    }

    fn database_now_cte(&self) -> &'static str {
        #[cfg(test)]
        if let Some(cte) = self.database_now_cte_override {
            return cte;
        }
        database_now_cte(self.backend)
    }
}

/// Returns one materialized, integer-second database clock for a mutation.
///
/// PostgreSQL's `clock_timestamp` is volatile, so the CTE establishes one
/// value for all of a statement's deadline decisions. Both backends floor to
/// whole Unix seconds, matching the integer timestamp columns.
fn database_now_cte(backend: DatabaseBackend) -> &'static str {
    match backend {
        DatabaseBackend::Sqlite => {
            "WITH database_now(epoch) AS MATERIALIZED (SELECT unixepoch('now'))"
        }
        DatabaseBackend::Postgres => {
            "WITH database_now(epoch) AS MATERIALIZED \
             (SELECT FLOOR(EXTRACT(EPOCH FROM clock_timestamp()))::BIGINT)"
        }
    }
}

/// Builds the failure transition around one materialized statement-time clock.
fn mark_failed_query(database_now_cte: &str) -> String {
    format!(
        "{database_now_cte} UPDATE delivery_outbox
         SET status = CASE
                 WHEN created_at <= ((SELECT epoch FROM database_now) - $7) THEN 'dead_letter'
                 ELSE $2
             END,
             attempts = attempts + 1,
             next_attempt_at = CASE
                 WHEN created_at <= ((SELECT epoch FROM database_now) - $7) THEN $4
                 ELSE $3
             END,
             last_attempt_at = $4,
             updated_at = $4,
             last_error = CASE
                 WHEN created_at <= ((SELECT epoch FROM database_now) - $7) THEN 'resolution_deadline_exceeded'
                 ELSE $5
             END,
             claim_id = NULL
          WHERE outbox_id = $1 AND claim_id = $6 AND status = 'in_progress'"
    )
}

fn notification_to_json(notification: &Notification) -> Result<String, sqlx::Error> {
    serde_json::to_string(notification).map_err(|err| sqlx::Error::Encode(Box::new(err)))
}

fn row_to_claimed(
    outbox_id: String,
    claim_id: String,
    row: sqlx::any::AnyRow,
) -> Result<ClaimedDelivery, serde_json::Error> {
    let notification_json: String = row.get("notification_json");
    let notification = serde_json::from_str(&notification_json)?;
    Ok(ClaimedDelivery {
        outbox_id,
        registration: PushRegistration {
            recipient_id: RecipientId(row.get("recipient_id")),
            installation_id: DeviceInstallationId(row.get("installation_id")),
            fcm_token: FcmRegistrationToken(row.get("fcm_token")),
            platform: row.get::<Option<String>, _>("platform").map(Platform),
            created_at: row.get("created_at"),
            last_seen_at: row.get("created_at"),
            disabled_at: None,
            disabled_reason: None,
        },
        notification,
        claim_id,
        created_at: row.get("created_at"),
    })
}

fn row_to_admin_row(row: sqlx::any::AnyRow) -> OutboxAdminRow {
    OutboxAdminRow {
        outbox_id: row.get("outbox_id"),
        event_id: row.get("event_id"),
        recipient_id: row.get("recipient_id"),
        installation_id: row.get("installation_id"),
        platform: row.get("platform"),
        status: row.get("status"),
        attempts: row.get("attempts"),
        last_error: row.get("last_error"),
        next_attempt_at: row.get("next_attempt_at"),
        last_attempt_at: row.get("last_attempt_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn validate_dead_letter_selector(selector: &OutboxDeadLetterSelector) -> Result<(), sqlx::Error> {
    if selector.outbox_ids.len() > usize::try_from(MAX_DEAD_LETTER_ADMIN_SELECTION).unwrap_or(1000)
    {
        return Err(admin_selector_error("too many outbox ids in selector"));
    }
    let unique_ids = selector.outbox_ids.iter().collect::<BTreeSet<_>>();
    if unique_ids.len() != selector.outbox_ids.len() {
        return Err(admin_selector_error("duplicate outbox ids in selector"));
    }
    if let Some(limit) = selector.limit {
        validate_admin_limit(limit)?;
    }
    Ok(())
}

fn validate_admin_limit(limit: i64) -> Result<(), sqlx::Error> {
    if !(1..=MAX_DEAD_LETTER_ADMIN_SELECTION).contains(&limit) {
        return Err(admin_selector_error(
            "dead-letter admin limit must be between 1 and 1000",
        ));
    }
    Ok(())
}

fn admin_selector_error(message: &'static str) -> sqlx::Error {
    sqlx::Error::Configuration(
        std::io::Error::new(std::io::ErrorKind::InvalidInput, message).into(),
    )
}

fn replay_error(message: &'static str) -> sqlx::Error {
    sqlx::Error::Configuration(
        std::io::Error::new(std::io::ErrorKind::InvalidInput, message).into(),
    )
}

fn backoff_seconds(attempts: i64) -> i64 {
    match attempts {
        0 | 1 => 1,
        2 => 5,
        3 => 30,
        _ => 300,
    }
}

fn age_seconds(now: i64, timestamp: Option<i64>) -> i64 {
    timestamp.map_or(0, |timestamp| now.saturating_sub(timestamp).max(0))
}

#[cfg(test)]
mod tests;
