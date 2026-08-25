use sqlx::{Any, AnyPool, Row, Transaction};

use fedi_decentralized_push_gateway_types::{
    CreateHookRequest, DeviceInstallationId, HookId, HookNotificationRecord, HookOpenBehavior,
    HookOpenRecord, HookPolicyRecord, HookPrivacy, HookRateLimitRecord, HookRecord, HookToken,
    NotificationKind, RecipientId, random_url_token,
};

use crate::{
    HookIdempotencyRepository, PushRegistrationRepository, RegistrationEligibility,
    log_sanitizer::sanitize_log_value, push_registration_repository::acquire_admission_lock,
    time::unix_timestamp,
};

/// Default fixed rate-limit window for newly created hooks.
pub const DEFAULT_RATE_LIMIT_WINDOW_SECONDS: i64 = 3_600;
/// Default maximum accepted invocations per rate-limit window for new hooks.
pub const DEFAULT_RATE_LIMIT_MAX_REQUESTS: i64 = 2;
/// Maximum accepted fixed rate-limit window duration.
pub const MAX_RATE_LIMIT_WINDOW_SECONDS: i64 = 86_400;
/// Maximum accepted invocation count per fixed rate-limit window.
pub const MAX_RATE_LIMIT_REQUESTS_PER_WINDOW: i64 = 10_000;

/// Durable hook-admission ceilings and bounded reclamation policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HookAdmissionLimits {
    /// Maximum active hooks owned by one recipient; zero disables the limit.
    pub max_active_per_recipient: u64,
    /// Maximum active hooks across the gateway; zero disables the limit.
    pub max_active_global: u64,
    /// Maximum physical hook rows across the gateway; zero disables the limit.
    pub max_total_rows: u64,
    /// Maximum terminal unreferenced rows reclaimed by this admission attempt.
    pub reclamation_batch_size: u64,
}

/// Result of an atomic hook-admission attempt.
#[derive(Debug)]
pub enum HookAdmissionOutcome {
    /// The hook and its one-time bearer token were created.
    Created(Box<CreatedHook>),
    /// The named installation is absent, disabled, stale, or not owned by the signer.
    TargetUnavailable,
    /// The recipient's active-hook ceiling would be exceeded.
    RecipientCapacityExceeded,
    /// The global active-hook ceiling would be exceeded.
    GlobalCapacityExceeded,
    /// The physical hook-row ceiling remains exhausted after bounded GC.
    GlobalRowCapacityExceeded,
}

/// Newly admitted hook and its one-time bearer token.
#[derive(Debug)]
pub struct CreatedHook {
    /// Persisted hook metadata.
    pub record: HookRecord,
    /// One-time bearer token returned to the creator.
    pub token: HookToken,
}

/// Low-cardinality physical hook-row metrics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HookRowMetrics {
    /// Total physical hook rows.
    pub total: i64,
    /// Active non-terminal hook rows.
    pub active: i64,
    /// Expired or revoked hook rows.
    pub terminal: i64,
}

/// Persistence operations for notification hook records.
#[derive(Clone, Debug)]
pub struct HookRepository {
    pool: AnyPool,
}

impl HookRepository {
    /// Creates a hook repository from a database pool.
    #[must_use]
    pub fn new(pool: AnyPool) -> Self {
        Self { pool }
    }

    /// Atomically validates the target, reclaims terminal rows, applies active
    /// and physical ceilings, and creates a hook.
    pub async fn admit_hook(
        &self,
        recipient_id: &RecipientId,
        request: &CreateHookRequest,
        registration_eligibility: RegistrationEligibility,
        limits: HookAdmissionLimits,
    ) -> Result<HookAdmissionOutcome, sqlx::Error> {
        let now = unix_timestamp();
        let hook_id = HookId(random_url_token(18));
        let hook_token = HookToken::generate();
        let expires_at = request
            .policy
            .expires_in_seconds
            .and_then(|seconds| (0 < seconds).then_some(now + seconds));
        let data_json = serde_json::to_string(&request.data)
            .map_err(|err| sqlx::Error::Encode(Box::new(err)))?;
        let rate_limit_window_seconds = request
            .policy
            .rate_limit
            .as_ref()
            .and_then(|rate_limit| rate_limit.window_seconds)
            .or(Some(DEFAULT_RATE_LIMIT_WINDOW_SECONDS));
        let rate_limit_max_requests = request
            .policy
            .rate_limit
            .as_ref()
            .and_then(|rate_limit| rate_limit.max_requests)
            .or(Some(DEFAULT_RATE_LIMIT_MAX_REQUESTS));

        let mut transaction = self.pool.begin().await?;
        acquire_admission_lock(&mut transaction, "hook", now).await?;
        purge_terminal_in_transaction(&mut transaction, now, limits.reclamation_batch_size).await?;

        if PushRegistrationRepository::eligible_installation_in_transaction(
            &mut transaction,
            recipient_id,
            &request.installation_id,
            registration_eligibility,
        )
        .await?
        .is_none()
        {
            transaction.rollback().await?;
            return Ok(HookAdmissionOutcome::TargetUnavailable);
        }

        if 0 < limits.max_active_per_recipient {
            let active: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM notification_hooks
                 WHERE recipient_id = $1 AND revoked_at IS NULL
                   AND (expires_at IS NULL OR $2 < expires_at)",
            )
            .bind(&recipient_id.0)
            .bind(now)
            .fetch_one(&mut *transaction)
            .await?;
            if limits.max_active_per_recipient <= u64::try_from(active).unwrap_or(u64::MAX) {
                transaction.rollback().await?;
                return Ok(HookAdmissionOutcome::RecipientCapacityExceeded);
            }
        }
        if 0 < limits.max_active_global {
            let active: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM notification_hooks
                 WHERE revoked_at IS NULL AND (expires_at IS NULL OR $1 < expires_at)",
            )
            .bind(now)
            .fetch_one(&mut *transaction)
            .await?;
            if limits.max_active_global <= u64::try_from(active).unwrap_or(u64::MAX) {
                transaction.rollback().await?;
                return Ok(HookAdmissionOutcome::GlobalCapacityExceeded);
            }
        }
        if 0 < limits.max_total_rows {
            let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_hooks")
                .fetch_one(&mut *transaction)
                .await?;
            if limits.max_total_rows <= u64::try_from(total).unwrap_or(u64::MAX) {
                transaction.rollback().await?;
                return Ok(HookAdmissionOutcome::GlobalRowCapacityExceeded);
            }
        }

        sqlx::query(
             "INSERT INTO notification_hooks (
                 hook_id, hook_secret_hash, recipient_id, installation_id, label, kind, workflow, action, deep_link,
                 open_behavior, privacy, title, body, data_json,
                 created_at, expires_at, max_uses, rate_limit_window_seconds,
                 rate_limit_max_requests
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)",
        )
        .bind(&hook_id.0)
        .bind(hook_token.hash_hex())
        .bind(&recipient_id.0)
        .bind(&request.installation_id.0)
        .bind(request.label.as_deref())
        .bind(request.notification.kind.as_ref().map(|kind| kind.0.as_str()))
        .bind(request.open.workflow.as_deref())
        .bind(request.open.action.as_deref())
        .bind(request.open.deep_link.as_deref())
        .bind(
            request
                .open
                .open_behavior
                .unwrap_or_default()
                .as_str(),
        )
        .bind(request.notification.privacy.unwrap_or_default().as_str())
        .bind(request.notification.title.as_deref())
        .bind(request.notification.body.as_deref())
        .bind(data_json)
        .bind(now)
        .bind(expires_at)
        .bind(request.policy.max_uses.filter(|max_uses| 0 < *max_uses))
        .bind(rate_limit_window_seconds)
        .bind(rate_limit_max_requests)
        .execute(&mut *transaction)
        .await?;

        let row = hook_row_query()
            .bind(&hook_id.0)
            .fetch_one(&mut *transaction)
            .await?;
        let record = row_to_record(row)?;
        transaction.commit().await?;
        Ok(HookAdmissionOutcome::Created(Box::new(CreatedHook {
            record,
            token: hook_token,
        })))
    }

    /// Lists hooks owned by a recipient without exposing raw tokens.
    pub async fn list_for_recipient(
        &self,
        recipient_id: &RecipientId,
    ) -> Result<Vec<HookRecord>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT hook_id, recipient_id, installation_id, label, kind, workflow, action, deep_link,
                     open_behavior, privacy, title, body, data_json, created_at,
                    expires_at, revoked_at, max_uses, rate_limit_window_seconds,
                    rate_limit_max_requests, rate_limit_window_started_at, rate_limit_count,
                    use_count, last_used_at
             FROM notification_hooks
             WHERE recipient_id = $1
             ORDER BY created_at DESC, hook_id DESC",
        )
        .bind(&recipient_id.0)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_record).collect()
    }

    /// Fetches one hook by id without exposing the token hash.
    pub async fn get(&self, hook_id: &HookId) -> Result<Option<HookRecord>, sqlx::Error> {
        let row = hook_row_query()
            .bind(&hook_id.0)
            .fetch_optional(&self.pool)
            .await?;

        row.map(row_to_record).transpose()
    }

    /// Revokes a hook owned by a recipient.
    pub async fn revoke(
        &self,
        hook_id: &HookId,
        recipient_id: &RecipientId,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE notification_hooks
             SET revoked_at = COALESCE(revoked_at, $3)
             WHERE hook_id = $1 AND recipient_id = $2",
        )
        .bind(&hook_id.0)
        .bind(&recipient_id.0)
        .bind(unix_timestamp())
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Verifies a hook bearer token and policy without consuming use/rate-limit counters.
    pub async fn verify_without_marking_used(
        &self,
        hook_id: &HookId,
        hook_token: &HookToken,
    ) -> Result<HookUseOutcome, sqlx::Error> {
        let now = unix_timestamp();
        let supplied_secret_hash = hook_token.hash_hex();
        let row = sqlx::query(
            "SELECT hook_secret_hash, hook_id, recipient_id, installation_id, label, kind, workflow, action, deep_link,
                     open_behavior, privacy, title, body, data_json,
                     created_at, expires_at, revoked_at, max_uses, rate_limit_window_seconds,
                     rate_limit_max_requests, rate_limit_window_started_at, rate_limit_count,
                     use_count, last_used_at
              FROM notification_hooks
              WHERE hook_id = $1",
        )
        .bind(&hook_id.0)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(HookUseOutcome::NotFound);
        };
        let stored_secret_hash: String = row.get("hook_secret_hash");
        if !constant_time_eq(
            stored_secret_hash.as_bytes(),
            supplied_secret_hash.as_bytes(),
        ) {
            return Ok(HookUseOutcome::NotFound);
        }
        let record = hook_record_from_row(&row)?;
        if let Some(rejection) = classify_rejection(&record, now) {
            return Ok(rejection);
        }
        Ok(HookUseOutcome::Accepted(Box::new(record)))
    }

    /// Verifies only a hook secret for a public hook id, without evaluating mutable policy state.
    pub async fn verify_token(
        &self,
        hook_id: &HookId,
        hook_token: &HookToken,
    ) -> Result<bool, sqlx::Error> {
        let Some(stored_hash): Option<String> = sqlx::query_scalar(
            "SELECT hook_secret_hash FROM notification_hooks WHERE hook_id = $1",
        )
        .bind(&hook_id.0)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(false);
        };
        Ok(constant_time_eq(
            stored_hash.as_bytes(),
            hook_token.hash_hex().as_bytes(),
        ))
    }

    /// Deletes expired or revoked hooks after their retained events are gone.
    pub async fn purge_terminal_unreferenced(
        &self,
        now_timestamp: i64,
    ) -> Result<u64, sqlx::Error> {
        Ok(sqlx::query(
            "DELETE FROM notification_hooks
             WHERE (expires_at IS NOT NULL AND expires_at <= $1 OR revoked_at IS NOT NULL)
                AND NOT EXISTS (
                    SELECT 1 FROM notification_events
                    WHERE notification_events.hook_id = notification_hooks.hook_id
                )",
        )
        .bind(now_timestamp)
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    /// Returns physical/active hook-row counts for operator metrics.
    pub async fn row_metrics(&self, now_timestamp: i64) -> Result<HookRowMetrics, sqlx::Error> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS total,
                    COALESCE(SUM(CASE WHEN revoked_at IS NULL AND (expires_at IS NULL OR $1 < expires_at) THEN 1 ELSE 0 END), 0) AS active,
                    COALESCE(SUM(CASE WHEN revoked_at IS NOT NULL OR (expires_at IS NOT NULL AND expires_at <= $1) THEN 1 ELSE 0 END), 0) AS terminal
             FROM notification_hooks",
        )
        .bind(now_timestamp)
        .fetch_one(&self.pool)
        .await?;
        Ok(HookRowMetrics {
            total: row.get("total"),
            active: row.get("active"),
            terminal: row.get("terminal"),
        })
    }
}

fn hook_row_query<'q>() -> sqlx::query::Query<'q, Any, sqlx::any::AnyArguments<'q>> {
    sqlx::query(
        "SELECT hook_id, recipient_id, installation_id, label, kind, workflow, action, deep_link,
                open_behavior, privacy, title, body, data_json, created_at,
                expires_at, revoked_at, max_uses, rate_limit_window_seconds,
                rate_limit_max_requests, rate_limit_window_started_at, rate_limit_count,
                use_count, last_used_at
         FROM notification_hooks WHERE hook_id = $1",
    )
}

async fn purge_terminal_in_transaction(
    transaction: &mut Transaction<'_, Any>,
    now_timestamp: i64,
    batch_size: u64,
) -> Result<u64, sqlx::Error> {
    if batch_size == 0 {
        return Ok(0);
    }
    HookIdempotencyRepository::purge_expired_in_transaction(transaction, now_timestamp, batch_size)
        .await?;
    let batch_size = i64::try_from(batch_size).unwrap_or(i64::MAX);
    Ok(sqlx::query(
        "DELETE FROM notification_hooks
         WHERE hook_id IN (
             SELECT hook_id FROM notification_hooks
             WHERE (expires_at IS NOT NULL AND expires_at <= $1 OR revoked_at IS NOT NULL)
                AND NOT EXISTS (
                    SELECT 1 FROM notification_events
                    WHERE notification_events.hook_id = notification_hooks.hook_id
                )
             ORDER BY created_at LIMIT $2
         )",
    )
    .bind(now_timestamp)
    .bind(batch_size)
    .execute(&mut **transaction)
    .await?
    .rows_affected())
}

/// Result of validating and consuming one hook invocation.
#[derive(Clone, Debug, PartialEq)]
pub enum HookUseOutcome {
    /// Hook was valid, and the record includes the post-increment use count.
    Accepted(Box<HookRecord>),
    /// Hook token was absent or did not match.
    NotFound,
    /// Hook exists but has expired.
    Expired,
    /// Hook exists but was revoked.
    Revoked,
    /// Hook exists but reached its configured maximum use count.
    MaxUsesExceeded,
    /// Hook exists but exceeded its fixed-window rate limit.
    RateLimited,
}

fn row_to_record(row: sqlx::any::AnyRow) -> Result<HookRecord, sqlx::Error> {
    hook_record_from_row(&row)
}

fn classify_rejection(record: &HookRecord, now: i64) -> Option<HookUseOutcome> {
    if record.revoked_at.is_some() {
        Some(HookUseOutcome::Revoked)
    } else if record
        .policy
        .expires_at
        .is_some_and(|expires_at| expires_at <= now)
    {
        Some(HookUseOutcome::Expired)
    } else if record
        .policy
        .max_uses
        .is_some_and(|max_uses| record.use_count >= max_uses)
    {
        Some(HookUseOutcome::MaxUsesExceeded)
    } else if rate_limited(record, now) {
        Some(HookUseOutcome::RateLimited)
    } else {
        None
    }
}

fn rate_limited(record: &HookRecord, now: i64) -> bool {
    let Some(rate_limit) = record.policy.rate_limit.as_ref() else {
        return false;
    };
    let Some(window_started_at) = rate_limit.window_started_at else {
        return false;
    };
    window_started_at + rate_limit.window_seconds > now
        && rate_limit.count >= rate_limit.max_requests
}

pub fn hook_record_from_row(row: &sqlx::any::AnyRow) -> Result<HookRecord, sqlx::Error> {
    let data_json: String = row.get("data_json");
    let data = serde_json::from_str(&data_json).map_err(|err| {
        eprintln!(
            "event=hook_row_decode_failure field=data_json error={}",
            sanitize_log_value(&err.to_string())
        );
        sqlx::Error::Decode(Box::new(err))
    })?;
    let kind: Option<String> = row.get("kind");

    Ok(HookRecord {
        hook_id: HookId(row.get("hook_id")),
        recipient_id: RecipientId(row.get("recipient_id")),
        installation_id: DeviceInstallationId(row.get("installation_id")),
        label: row.get("label"),
        notification: HookNotificationRecord {
            kind: kind.map(NotificationKind),
            title: row.get("title"),
            body: row.get("body"),
            privacy: HookPrivacy::from(row.get::<Option<String>, _>("privacy")),
        },
        open: HookOpenRecord {
            open_behavior: HookOpenBehavior::from(row.get::<Option<String>, _>("open_behavior")),
            workflow: row.get("workflow"),
            action: row.get("action"),
            deep_link: row.get("deep_link"),
        },
        data,
        created_at: row.get("created_at"),
        revoked_at: row.get("revoked_at"),
        policy: HookPolicyRecord {
            expires_at: row.get("expires_at"),
            max_uses: row.get("max_uses"),
            rate_limit: match (
                row.get::<Option<i64>, _>("rate_limit_window_seconds"),
                row.get::<Option<i64>, _>("rate_limit_max_requests"),
            ) {
                (Some(window_seconds), Some(max_requests)) => Some(HookRateLimitRecord {
                    window_seconds,
                    max_requests,
                    window_started_at: row.get("rate_limit_window_started_at"),
                    count: row.get("rate_limit_count"),
                }),
                _ => None,
            },
        },
        use_count: row.get("use_count"),
        last_used_at: row.get("last_used_at"),
    })
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = left.len() ^ right.len();
    let max = left.len().max(right.len());
    for index in 0..max {
        let lhs = left.get(index).copied().unwrap_or(0);
        let rhs = right.get(index).copied().unwrap_or(0);
        diff |= usize::from(lhs ^ rhs);
    }
    diff == 0
}
