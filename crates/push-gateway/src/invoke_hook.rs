use axum::{
    Extension, Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
};
use serde_json::Value;
use std::{net::SocketAddr, time::Duration};

use fedi_decentralized_push_gateway_storage::hook_record_from_row;
use fedi_decentralized_push_gateway_types::random_url_token;

use crate::{
    ApiError, AppState, HookId, HookIdempotencyRepository, HookPrivacy, HookRecord, HookRepository,
    HookToken, HookUseOutcome, InvokeHookRequest, InvokeHookResponse, JsonPayload, Notification,
    NotificationId, NotificationKind, PushRegistrationRepository, RecipientId,
    RegistrationEligibility,
    hook_management::internal_error,
    time::unix_timestamp,
    validation::{
        validate_data, validate_fcm_outbound_notification, validate_optional_string,
        validate_required_string,
    },
};

/// Handles public invocation of a generated hook capability URL.
pub(crate) async fn invoke_hook(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Path((hook_id, hook_secret)): Path<(String, String)>,
    JsonPayload(request): JsonPayload<InvokeHookRequest>,
) -> Result<Json<InvokeHookResponse>, ApiError> {
    validate_required_string("hook_id", &hook_id, 128)?;
    validate_required_string("hook_secret", &hook_secret, 512)?;
    validate_invoke_request(&request)?;
    let source_prefix = invocation_source_prefix(
        &state,
        &headers,
        peer.map(|Extension(ConnectInfo(peer))| peer),
    );
    let limits = state.config().rate_limits().clone();

    let invocation_guard = state
        .acquire_database_write_lock()
        .await
        .map_err(crate::app_state::database_write_admission_error)?;

    let hook_id = HookId(hook_id);
    let hook_secret = HookToken::from_path_segment(hook_secret);
    let context = InvocationContext {
        pool: state.database().pool(),
        observability: state.observability(),
        rate_limiters: state.rate_limiters(),
        source_prefix,
        policy: InvocationPolicy {
            hook_invocations_per_source_prefix: limits.hook_invocations_per_source_prefix,
            hook_invocations_per_hook: limits.hook_invocations_per_hook,
            hook_invocation_window_seconds: limits.hook_invocation_window_seconds,
            max_global_outbox_backlog: limits.max_global_outbox_backlog,
            max_recipient_outbox_backlog: limits.max_recipient_outbox_backlog,
            max_idempotency_rows_global: limits.max_hook_rows_global,
            idempotency_reclamation_batch_size: limits.admission_gc_batch_size,
            production_mode: state.config().production_mode(),
            registration_ttl: Duration::from_secs(state.config().registration_ttl_seconds()),
        },
    };
    let outcome = accept_invocation(&context, &hook_id, &hook_secret, request).await?;
    drop(invocation_guard);
    if outcome.enqueued_rows {
        // The transaction and process-local mutation lock are both released before
        // retaining this notification permit, so the worker can promptly claim the
        // newly committed row without racing SQLite writer upgrades.
        state.notify_delivery_worker();
    }

    Ok(Json(InvokeHookResponse::accepted(outcome.target_count)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InvocationOutcome {
    target_count: usize,
    enqueued_rows: bool,
}

struct InvocationContext<'a> {
    pool: &'a sqlx::AnyPool,
    observability: crate::Observability,
    rate_limiters: crate::rate_limits::RateLimiters,
    source_prefix: String,
    policy: InvocationPolicy,
}

#[derive(Clone, Copy, Debug)]
struct InvocationPolicy {
    hook_invocations_per_source_prefix: u64,
    hook_invocations_per_hook: u64,
    hook_invocation_window_seconds: u64,
    max_global_outbox_backlog: u64,
    max_recipient_outbox_backlog: u64,
    max_idempotency_rows_global: u64,
    idempotency_reclamation_batch_size: u64,
    production_mode: bool,
    registration_ttl: Duration,
}

async fn accept_invocation(
    context: &InvocationContext<'_>,
    hook_id: &HookId,
    hook_secret: &HookToken,
    request: InvokeHookRequest,
) -> Result<InvocationOutcome, ApiError> {
    let now = unix_timestamp();
    let registration_eligibility = RegistrationEligibility {
        cutoff_timestamp: now.saturating_sub(
            i64::try_from(context.policy.registration_ttl.as_secs()).unwrap_or(i64::MAX),
        ),
    };
    let hook_secret_hash = hook_secret.hash_hex();
    let caller_idempotency_key = request.idempotency_key.clone();
    let mut transaction = context.pool.begin().await.map_err(internal_error)?;

    let hook_row = sqlx::query(
        "SELECT hook_secret_hash, hook_id, recipient_id, installation_id, label, kind, workflow,
                action, deep_link, open_behavior, privacy, title, body, data_json, created_at,
                expires_at, revoked_at, max_uses, rate_limit_window_seconds,
                rate_limit_max_requests, rate_limit_window_started_at, rate_limit_count,
                use_count, last_used_at
         FROM notification_hooks WHERE hook_id = $1",
    )
    .bind(&hook_id.0)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(internal_error)?;
    let Some(hook_row) = hook_row else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "hook_not_found",
            "hook not found",
        ));
    };
    let stored_secret_hash: String = sqlx::Row::get(&hook_row, "hook_secret_hash");
    if !constant_time_eq(stored_secret_hash.as_bytes(), hook_secret_hash.as_bytes()) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "hook_not_found",
            "hook not found",
        ));
    }

    let mut hook = hook_record_from_row(&hook_row).map_err(internal_error)?;
    if let Some(rejection) = classify_hook_capability_terminated(&hook, now) {
        transaction.rollback().await.map_err(internal_error)?;
        return rejection_to_error(rejection, &context.observability);
    }

    if let Some(idempotency_key) = caller_idempotency_key.as_deref()
        && let Some(target_count) = HookIdempotencyRepository::retained_target_count_in_transaction(
            &mut transaction,
            hook_id,
            idempotency_key,
        )
        .await
        .map_err(internal_error)?
    {
        transaction.commit().await.map_err(internal_error)?;
        return Ok(InvocationOutcome {
            target_count,
            enqueued_rows: false,
        });
    }

    if let Some(rejection) = classify_hook_before_target(&hook, now) {
        transaction.rollback().await.map_err(internal_error)?;
        return rejection_to_error(rejection, &context.observability);
    }

    let registrations = PushRegistrationRepository::eligible_installation_in_transaction(
        &mut transaction,
        &hook.recipient_id,
        &hook.installation_id,
        registration_eligibility,
    )
    .await
    .map_err(internal_error)?
    .into_iter()
    .collect::<Vec<_>>();
    if registrations.is_empty() {
        transaction.rollback().await.map_err(internal_error)?;
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "target_installation_unavailable",
            "target installation is temporarily unavailable",
        ));
    }

    if caller_idempotency_key.is_some()
        && !HookIdempotencyRepository::ensure_capacity_in_transaction(
            &mut transaction,
            now,
            context.policy.max_idempotency_rows_global,
            context.policy.idempotency_reclamation_batch_size,
        )
        .await
        .map_err(internal_error)?
    {
        transaction.rollback().await.map_err(internal_error)?;
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "idempotency_capacity_exceeded",
            "hook idempotency capacity is temporarily unavailable",
        ));
    }

    enforce_invocation_rate_limits(
        &context.rate_limiters,
        &context.source_prefix,
        hook_id,
        context.policy.hook_invocations_per_source_prefix,
        context.policy.hook_invocations_per_hook,
        context.policy.hook_invocation_window_seconds,
    )
    .await?;

    let update_result = sqlx::query(
        "UPDATE notification_hooks
         SET use_count = use_count + 1,
             last_used_at = $2,
             rate_limit_window_started_at =
                 CASE
                     WHEN NOT $3
                           AND (rate_limit_window_seconds IS NULL
                                OR rate_limit_max_requests IS NULL)
                           OR rate_limit_window_started_at IS NULL
                           OR rate_limit_window_started_at
                                + CASE
                                      WHEN $3
                                           AND (rate_limit_window_seconds IS NULL
                                                OR rate_limit_window_seconds < $4)
                                      THEN $4
                                      ELSE rate_limit_window_seconds
                                  END <= $2
                     THEN $2
                     ELSE rate_limit_window_started_at
                 END,
             rate_limit_count =
                 CASE
                     WHEN NOT $3
                           AND (rate_limit_window_seconds IS NULL
                                OR rate_limit_max_requests IS NULL)
                     THEN 0
                     WHEN rate_limit_window_started_at IS NULL
                           OR rate_limit_window_started_at
                                + CASE
                                      WHEN $3
                                           AND (rate_limit_window_seconds IS NULL
                                                OR rate_limit_window_seconds < $4)
                                      THEN $4
                                      ELSE rate_limit_window_seconds
                                  END <= $2
                     THEN 1
                     ELSE rate_limit_count + 1
                 END
         WHERE hook_id = $1
            AND revoked_at IS NULL
             AND (expires_at IS NULL OR expires_at > $2)
             AND (max_uses IS NULL OR use_count < max_uses)
             AND (
                 (NOT $3 AND (
                     rate_limit_window_seconds IS NULL
                     OR rate_limit_max_requests IS NULL
                 ))
                 OR rate_limit_window_started_at IS NULL
                 OR rate_limit_window_started_at
                      + CASE
                            WHEN $3
                                 AND (rate_limit_window_seconds IS NULL
                                      OR rate_limit_window_seconds < $4)
                            THEN $4
                            ELSE rate_limit_window_seconds
                        END <= $2
                 OR rate_limit_count
                      < CASE
                            WHEN $3
                                 AND (rate_limit_max_requests IS NULL
                                      OR rate_limit_max_requests > $5)
                            THEN $5
                            ELSE rate_limit_max_requests
                        END
             )",
    )
    .bind(&hook_id.0)
    .bind(now)
    .bind(context.policy.production_mode)
    .bind(crate::DEFAULT_RATE_LIMIT_WINDOW_SECONDS)
    .bind(crate::DEFAULT_RATE_LIMIT_MAX_REQUESTS)
    .execute(&mut *transaction)
    .await
    .map_err(internal_error)?;

    if update_result.rows_affected() != 1 {
        transaction.rollback().await.map_err(internal_error)?;
        let outcome = HookRepository::new(context.pool.clone())
            .verify_without_marking_used(hook_id, hook_secret)
            .await
            .map_err(internal_error)?;
        if context.policy.production_mode && matches!(outcome, HookUseOutcome::Accepted(_)) {
            context.observability.record_rate_limit_rejection();
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "hook_rate_limited",
                "hook rate limited",
            ));
        }
        return rejection_to_error(outcome, &context.observability);
    }

    hook.use_count = hook.use_count.saturating_add(1);
    hook.last_used_at = Some(now);

    let hook_expires_at = hook.policy.expires_at;
    let notification = notification_from_hook_and_request(hook, request);
    enforce_outbox_backlog_caps(
        &mut transaction,
        &notification.recipient_id,
        registrations.len(),
        context.policy.max_global_outbox_backlog,
        context.policy.max_recipient_outbox_backlog,
    )
    .await?;
    validate_fcm_outbound_notification(&notification)?;
    let notification_json =
        serde_json::to_string(&notification).map_err(|_| ApiError::internal_server_error())?;
    let event_id = random_url_token(18);
    sqlx::query(
        "INSERT INTO notification_events (
             event_id, hook_id, caller_idempotency_key, recipient_id, notification_json,
             target_count, created_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&event_id)
    .bind(&hook_id.0)
    .bind(caller_idempotency_key.as_deref())
    .bind(&notification.recipient_id.0)
    .bind(&notification_json)
    .bind(i64::try_from(registrations.len()).unwrap_or(i64::MAX))
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(internal_error)?;

    if let Some(idempotency_key) = caller_idempotency_key.as_deref() {
        HookIdempotencyRepository::record_in_transaction(
            &mut transaction,
            hook_id,
            idempotency_key,
            registrations.len(),
            now,
            hook_expires_at,
        )
        .await
        .map_err(internal_error)?;
    }

    for registration in &registrations {
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
        .await
        .map_err(internal_error)?;
    }

    transaction.commit().await.map_err(internal_error)?;
    Ok(InvocationOutcome {
        target_count: registrations.len(),
        enqueued_rows: !registrations.is_empty(),
    })
}

fn classify_hook_before_target(hook: &HookRecord, now: i64) -> Option<HookUseOutcome> {
    classify_hook_capability_terminated(hook, now).or_else(|| {
        if hook
            .policy
            .max_uses
            .is_some_and(|max_uses| max_uses <= hook.use_count)
        {
            Some(HookUseOutcome::MaxUsesExceeded)
        } else if hook.policy.rate_limit.as_ref().is_some_and(|rate_limit| {
            rate_limit.window_started_at.is_some_and(|started_at| {
                now < started_at.saturating_add(rate_limit.window_seconds)
                    && rate_limit.max_requests <= rate_limit.count
            })
        }) {
            Some(HookUseOutcome::RateLimited)
        } else {
            None
        }
    })
}

fn classify_hook_capability_terminated(hook: &HookRecord, now: i64) -> Option<HookUseOutcome> {
    if hook.revoked_at.is_some() {
        Some(HookUseOutcome::Revoked)
    } else if hook
        .policy
        .expires_at
        .is_some_and(|expires_at| expires_at <= now)
    {
        Some(HookUseOutcome::Expired)
    } else {
        None
    }
}

fn invocation_source_prefix(
    state: &AppState,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
) -> String {
    let limits = state.config().rate_limits();
    crate::rate_limits::effective_source_prefix(peer, headers, limits.trusted_proxy_cidrs.as_ref())
}

async fn enforce_invocation_rate_limits(
    rate_limiters: &crate::rate_limits::RateLimiters,
    source: &str,
    hook_id: &HookId,
    hook_invocations_per_source_prefix: u64,
    hook_invocations_per_hook: u64,
    hook_invocation_window_seconds: u64,
) -> Result<(), ApiError> {
    if !rate_limiters
        .check(
            crate::rate_limits::RateLimitFamily::HookInvocationSource,
            format!("hook-invoke-source:{source}"),
            hook_invocations_per_source_prefix,
            hook_invocation_window_seconds,
        )
        .await
    {
        return Err(rate_limit_error(
            "source_rate_limited",
            "source rate limited",
        ));
    }
    if !rate_limiters
        .check(
            crate::rate_limits::RateLimitFamily::HookInvocationHook,
            format!("hook-invoke-hook:{}", hook_id.0),
            hook_invocations_per_hook,
            hook_invocation_window_seconds,
        )
        .await
    {
        return Err(rate_limit_error("hook_rate_limited", "hook rate limited"));
    }
    Ok(())
}

async fn enforce_outbox_backlog_caps(
    transaction: &mut sqlx::Transaction<'_, sqlx::Any>,
    recipient_id: &RecipientId,
    new_rows: usize,
    max_global_outbox_backlog: u64,
    max_recipient_outbox_backlog: u64,
) -> Result<(), ApiError> {
    if new_rows == 0 {
        return Ok(());
    }
    let new_rows = u64::try_from(new_rows).unwrap_or(u64::MAX);
    if 0 < max_global_outbox_backlog {
        let global: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_outbox
             WHERE status IN ('pending', 'retrying', 'in_progress')",
        )
        .fetch_one(&mut **transaction)
        .await
        .map_err(internal_error)?;
        if max_global_outbox_backlog
            < u64::try_from(global)
                .unwrap_or(u64::MAX)
                .saturating_add(new_rows)
        {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "outbox_backlog_full",
                "outbox backlog full",
            ));
        }
    }
    if 0 < max_recipient_outbox_backlog {
        let recipient: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_outbox
             WHERE recipient_id = $1
               AND status IN ('pending', 'retrying', 'in_progress')",
        )
        .bind(&recipient_id.0)
        .fetch_one(&mut **transaction)
        .await
        .map_err(internal_error)?;
        if max_recipient_outbox_backlog
            < u64::try_from(recipient)
                .unwrap_or(u64::MAX)
                .saturating_add(new_rows)
        {
            return Err(rate_limit_error(
                "recipient_outbox_backlog_full",
                "recipient outbox backlog full",
            ));
        }
    }
    Ok(())
}

fn rate_limit_error(code: &'static str, message: &'static str) -> ApiError {
    ApiError::new(StatusCode::TOO_MANY_REQUESTS, code, message)
}

fn rejection_to_error<T>(
    outcome: HookUseOutcome,
    observability: &crate::Observability,
) -> Result<T, ApiError> {
    match outcome {
        HookUseOutcome::Accepted(_) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "hook_conflict",
            "hook conflict",
        )),
        HookUseOutcome::NotFound => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "hook_not_found",
            "hook not found",
        )),
        HookUseOutcome::Expired => Err(ApiError::new(
            StatusCode::GONE,
            "hook_expired",
            "hook expired",
        )),
        HookUseOutcome::Revoked => Err(ApiError::new(
            StatusCode::GONE,
            "hook_revoked",
            "hook revoked",
        )),
        HookUseOutcome::MaxUsesExceeded => Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "hook_max_uses_exceeded",
            "hook max uses exceeded",
        )),
        HookUseOutcome::RateLimited => {
            observability.record_rate_limit_rejection();
            Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "hook_rate_limited",
                "hook rate limited",
            ))
        }
    }
}

fn notification_from_hook_and_request(
    hook: HookRecord,
    request: InvokeHookRequest,
) -> Notification {
    let mut data = hook.data;
    data.insert(
        "pg.open_behavior".to_owned(),
        Value::String(hook.open.open_behavior.as_str().to_owned()),
    );
    data.insert(
        "pg.privacy".to_owned(),
        Value::String(hook.notification.privacy.as_str().to_owned()),
    );
    if let Some(workflow) = hook.open.workflow.clone() {
        data.insert("pg.workflow".to_owned(), Value::String(workflow));
    }
    if let Some(action) = hook.open.action.clone() {
        data.insert("pg.action".to_owned(), Value::String(action));
    }
    if let Some(deep_link) = hook.open.deep_link.clone() {
        data.insert("pg.deep_link".to_owned(), Value::String(deep_link));
    }
    for (key, value) in request.data {
        data.insert(key, value);
    }

    Notification {
        recipient_id: hook.recipient_id,
        notification_id: NotificationId(format!("hook:{}:{}", hook.hook_id.0, hook.use_count)),
        kind: hook
            .notification
            .kind
            .unwrap_or_else(|| NotificationKind("hook".to_owned())),
        title: if hook.notification.privacy == HookPrivacy::DataOnly {
            None
        } else {
            hook.notification.title
        },
        body: if hook.notification.privacy == HookPrivacy::DataOnly {
            None
        } else {
            hook.notification.body
        },
        data,
    }
}

fn validate_invoke_request(request: &InvokeHookRequest) -> Result<(), ApiError> {
    validate_optional_string("idempotency_key", request.idempotency_key.as_deref(), 128)?;
    validate_data(&request.data)?;
    crate::validation::validate_no_reserved_data_keys(&request.data)?;
    Ok(())
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
