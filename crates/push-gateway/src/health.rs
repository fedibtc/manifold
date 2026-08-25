use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
};

use crate::{
    AppState, DeliveryOutboxRepository, HealthResponse, HookRepository, PushRegistrationRepository,
    TelemetryRepository,
};

/// Compatibility health endpoint: the process is running and can serve HTTP.
pub(crate) async fn health_compat() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

/// Liveness endpoint: the process is running and can serve HTTP.
pub(crate) async fn liveness(State(state): State<AppState>) -> Json<HealthResponse> {
    let observability = state.observability().snapshot();
    Json(HealthResponse::ok(
        "liveness",
        state.config().provider_mode(),
        state.config().outbox_worker_concurrency(),
        &observability,
        None,
    ))
}

/// Readiness endpoint: required local dependencies are reachable and configured.
pub(crate) async fn readiness(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    let database_ready = state.database().is_ready().await;
    let outbox = if database_ready {
        DeliveryOutboxRepository::new(state.database().pool().clone(), state.database().backend())
            .status_counts()
            .await
            .ok()
    } else {
        None
    };
    let observability = state.observability().snapshot();
    let ready = database_ready && observability.worker_running;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let response = if ready {
        HealthResponse::ok(
            "readiness",
            state.config().provider_mode(),
            state.config().outbox_worker_concurrency(),
            &observability,
            outbox,
        )
    } else {
        HealthResponse::not_ready(
            "readiness",
            state.config().provider_mode(),
            database_ready,
            state.config().outbox_worker_concurrency(),
            &observability,
            outbox,
        )
    };
    (status, Json(response))
}

/// Prometheus-compatible text metrics for local/operator scraping.
pub(crate) async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let snapshot = state.observability().snapshot();
    let outbox = match DeliveryOutboxRepository::new(
        state.database().pool().clone(),
        state.database().backend(),
    )
    .operational_metrics()
    .await
    {
        Ok(outbox) => outbox,
        Err(err) => {
            eprintln!(
                "event=metrics_scrape_error operation=delivery_outbox_operational_metrics error={}",
                crate::log_sanitizer::sanitize_log_value(&err.to_string())
            );
            let body =
                "# HELP push_gateway_metrics_scrape_db_error Whether this metrics scrape failed to read database-backed gauges.\n\
                 # TYPE push_gateway_metrics_scrape_db_error gauge\n\
                 push_gateway_metrics_scrape_db_error 1\n"
                    .to_owned();
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
                body,
            );
        }
    };
    let counts = &outbox.status_counts;
    let hook_rows = match HookRepository::new(state.database().pool().clone())
        .row_metrics(crate::time::unix_timestamp())
        .await
    {
        Ok(metrics) => metrics,
        Err(err) => return metrics_database_error("hook_row_metrics", err),
    };
    let registration_rows = match PushRegistrationRepository::new(state.database().pool().clone())
        .row_metrics(state.registration_eligibility())
        .await
    {
        Ok(metrics) => metrics,
        Err(err) => return metrics_database_error("registration_row_metrics", err),
    };
    let telemetry = match TelemetryRepository::new(state.database().pool().clone())
        .metrics()
        .await
    {
        Ok(metrics) => metrics,
        Err(err) => return metrics_database_error("telemetry_metrics", err),
    };
    let mut body = format!(
        "# HELP push_gateway_http_requests_total HTTP requests observed by the push gateway.\n\
         # TYPE push_gateway_http_requests_total counter\n\
         push_gateway_http_requests_total {}\n\
         # HELP push_gateway_http_responses_total HTTP responses by status class.\n\
         # TYPE push_gateway_http_responses_total counter\n\
         push_gateway_http_responses_total{{class=\"2xx\"}} {}\n\
         push_gateway_http_responses_total{{class=\"4xx\"}} {}\n\
         push_gateway_http_responses_total{{class=\"5xx\"}} {}\n\
         # HELP push_gateway_outbox_claims_total Delivery outbox rows claimed by this process.\n\
         # TYPE push_gateway_outbox_claims_total counter\n\
         push_gateway_outbox_claims_total {}\n\
         # HELP push_gateway_outbox_claim_queries_total Delivery outbox claim database queries issued by this process.\n\
         # TYPE push_gateway_outbox_claim_queries_total counter\n\
         push_gateway_outbox_claim_queries_total {}\n\
         # HELP push_gateway_outbox_idle_waits_total Delivery worker entries into an idle notification/deadline wait.\n\
         # TYPE push_gateway_outbox_idle_waits_total counter\n\
         push_gateway_outbox_idle_waits_total {}\n\
         # HELP push_gateway_outbox_delivery_total Delivery outbox provider outcomes.\n\
         # TYPE push_gateway_outbox_delivery_total counter\n\
         push_gateway_outbox_delivery_total{{result=\"success\"}} {}\n\
         push_gateway_outbox_delivery_total{{result=\"failure\"}} {}\n\
         # HELP push_gateway_provider_outcomes_total Provider delivery failures by sanitized reason class.\n\
         # TYPE push_gateway_provider_outcomes_total counter\n\
         push_gateway_provider_outcomes_total{{reason_class=\"auth\"}} {}\n\
         push_gateway_provider_outcomes_total{{reason_class=\"quota\"}} {}\n\
         push_gateway_provider_outcomes_total{{reason_class=\"network\"}} {}\n\
         push_gateway_provider_outcomes_total{{reason_class=\"invalid_token\"}} {}\n\
         push_gateway_provider_outcomes_total{{reason_class=\"invalid_payload\"}} {}\n\
         push_gateway_provider_outcomes_total{{reason_class=\"transient\"}} {}\n\
         # HELP push_gateway_outbox_rows Delivery outbox rows by status.\n\
         # TYPE push_gateway_outbox_rows gauge\n\
         push_gateway_outbox_rows{{status=\"pending\"}} {}\n\
         push_gateway_outbox_rows{{status=\"in_progress\"}} {}\n\
         push_gateway_outbox_rows{{status=\"retrying\"}} {}\n\
         push_gateway_outbox_rows{{status=\"succeeded\"}} {}\n\
         push_gateway_outbox_rows{{status=\"invalid_token\"}} {}\n\
         push_gateway_outbox_rows{{status=\"dead_letter\"}} {}\n\
         # HELP push_gateway_outbox_oldest_due_age_seconds Age of the oldest currently due outbox row.\n\
         # TYPE push_gateway_outbox_oldest_due_age_seconds gauge\n\
         push_gateway_outbox_oldest_due_age_seconds {}\n\
         # HELP push_gateway_outbox_oldest_pending_age_seconds Age of the oldest pending outbox row.\n\
         # TYPE push_gateway_outbox_oldest_pending_age_seconds gauge\n\
         push_gateway_outbox_oldest_pending_age_seconds {}\n\
         # HELP push_gateway_outbox_retrying_oldest_age_seconds Age since the oldest retrying outbox row was last updated.\n\
         # TYPE push_gateway_outbox_retrying_oldest_age_seconds gauge\n\
         push_gateway_outbox_retrying_oldest_age_seconds {}\n\
         # HELP push_gateway_outbox_dead_letter_rows Current retained dead-letter rows.\n\
         # TYPE push_gateway_outbox_dead_letter_rows gauge\n\
         push_gateway_outbox_dead_letter_rows {}\n\
         # HELP push_gateway_outbox_dead_letter_total Dead-letter transitions observed by this process.\n\
         # TYPE push_gateway_outbox_dead_letter_total counter\n\
         push_gateway_outbox_dead_letter_total {}\n\
         # HELP push_gateway_outbox_dead_letter_retained_total Total retained dead-letter rows in the database.\n\
         # TYPE push_gateway_outbox_dead_letter_retained_total gauge\n\
         push_gateway_outbox_dead_letter_retained_total {}\n\
         # HELP push_gateway_invalid_token_cleanup_failures_total Invalid-token cleanup database failures.\n\
         # TYPE push_gateway_invalid_token_cleanup_failures_total counter\n\
         push_gateway_invalid_token_cleanup_failures_total {}\n\
         # HELP push_gateway_rate_limit_rejections_total Hook invocation rejections by fixed-window rate limiting.\n\
         # TYPE push_gateway_rate_limit_rejections_total counter\n\
         push_gateway_rate_limit_rejections_total {}\n\
         # HELP push_gateway_hook_rows Physical hook rows by eligibility state.\n\
         # TYPE push_gateway_hook_rows gauge\n\
         push_gateway_hook_rows{{state=\"total\"}} {}\n\
         push_gateway_hook_rows{{state=\"active\"}} {}\n\
         push_gateway_hook_rows{{state=\"terminal\"}} {}\n\
         # HELP push_gateway_registration_rows Physical registration rows by eligibility state.\n\
         # TYPE push_gateway_registration_rows gauge\n\
         push_gateway_registration_rows{{state=\"total\"}} {}\n\
         push_gateway_registration_rows{{state=\"registrations\"}} {}\n\
         push_gateway_registration_rows{{state=\"token_owners\"}} {}\n\
         push_gateway_registration_rows{{state=\"orphaned_token_owners\"}} {}\n\
         push_gateway_registration_rows{{state=\"active\"}} {}\n\
         push_gateway_registration_rows{{state=\"disabled\"}} {}\n\
         push_gateway_registration_rows{{state=\"stale\"}} {}\n\
         # HELP push_gateway_metrics_scrape_db_error Whether this metrics scrape failed to read database-backed gauges.\n\
         # TYPE push_gateway_metrics_scrape_db_error gauge\n\
         push_gateway_metrics_scrape_db_error 0\n\
         # HELP push_gateway_outbox_worker_running Whether the delivery worker is running in this process.\n\
         # TYPE push_gateway_outbox_worker_running gauge\n\
         push_gateway_outbox_worker_running {}\n\
         # HELP push_gateway_provider_mode_info Configured push provider mode.\n\
         # TYPE push_gateway_provider_mode_info gauge\n\
         push_gateway_provider_mode_info{{mode=\"{}\"}} 1\n",
        snapshot.http_requests_total,
        snapshot.http_responses_2xx_total,
        snapshot.http_responses_4xx_total,
        snapshot.http_responses_5xx_total,
        snapshot.outbox_claims_total,
        snapshot.outbox_claim_queries_total,
        snapshot.outbox_idle_waits_total,
        snapshot.outbox_delivery_success_total,
        snapshot.outbox_delivery_failure_total,
        snapshot.outbox_delivery_failure_auth_total,
        snapshot.outbox_delivery_failure_quota_total,
        snapshot.outbox_delivery_failure_network_total,
        snapshot.outbox_delivery_failure_invalid_token_total,
        snapshot.outbox_delivery_failure_invalid_payload_total,
        snapshot.outbox_delivery_failure_transient_total,
        counts.pending,
        counts.in_progress,
        counts.retrying,
        counts.succeeded,
        counts.invalid_token,
        counts.dead_letter,
        outbox.oldest_due_age_seconds,
        outbox.oldest_pending_age_seconds,
        outbox.oldest_retrying_age_seconds,
        outbox.dead_letter_current,
        snapshot.outbox_dead_letter_total,
        outbox.dead_letter_total,
        snapshot.invalid_token_cleanup_failures_total,
        snapshot.rate_limit_rejections_total,
        hook_rows.total,
        hook_rows.active,
        hook_rows.terminal,
        registration_rows.total,
        registration_rows.registrations,
        registration_rows.token_owners,
        registration_rows.orphaned_token_owners,
        registration_rows.active,
        registration_rows.disabled,
        registration_rows.stale,
        u8::from(snapshot.worker_running),
        state.config().provider_mode(),
    );
    body.push_str(&format!(
        "# HELP push_gateway_guardian_telemetry_targets Verified FMan telemetry targets.\n\
         # TYPE push_gateway_guardian_telemetry_targets gauge\n\
         push_gateway_guardian_telemetry_targets {}\n\
         # HELP push_gateway_guardian_telemetry_enabled Whether complete telemetry receiver configuration is active.\n\
         # TYPE push_gateway_guardian_telemetry_enabled gauge\n\
         push_gateway_guardian_telemetry_enabled {}\n",
        telemetry.targets,
        u8::from(state.telemetry_runtime().is_some()),
    ));
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
}

fn metrics_database_error(
    operation: &'static str,
    err: sqlx::Error,
) -> (
    StatusCode,
    [(axum::http::HeaderName, &'static str); 1],
    String,
) {
    eprintln!(
        "event=metrics_scrape_error operation={} error={}",
        operation,
        crate::log_sanitizer::sanitize_log_value(&err.to_string())
    );
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        "# HELP push_gateway_metrics_scrape_db_error Whether this metrics scrape failed to read database-backed gauges.\n\
         # TYPE push_gateway_metrics_scrape_db_error gauge\n\
         push_gateway_metrics_scrape_db_error 1\n"
            .to_owned(),
    )
}
