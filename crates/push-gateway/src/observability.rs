use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderName, HeaderValue, Response},
    middleware::Next,
    response::IntoResponse,
};

use fedi_decentralized_push_gateway_types::random_url_token;

/// HTTP header carrying the per-request id.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Shared operational counters and worker state.
#[derive(Clone, Debug, Default)]
pub struct Observability {
    inner: Arc<ObservabilityInner>,
}

/// Atomic observability state.
#[derive(Debug, Default)]
struct ObservabilityInner {
    /// Total HTTP requests observed by the router middleware.
    http_requests_total: AtomicU64,
    /// HTTP responses with status code 2xx.
    http_responses_2xx_total: AtomicU64,
    /// HTTP responses with status code 4xx.
    http_responses_4xx_total: AtomicU64,
    /// HTTP responses with status code 5xx.
    http_responses_5xx_total: AtomicU64,
    /// Delivery worker claim attempts that found a row.
    outbox_claims_total: AtomicU64,
    /// Delivery worker database claim queries, including idle scans that found no row.
    outbox_claim_queries_total: AtomicU64,
    /// Delivery worker entries into an idle notification/deadline wait.
    outbox_idle_waits_total: AtomicU64,
    /// Delivery worker successful provider deliveries.
    outbox_delivery_success_total: AtomicU64,
    /// Delivery worker failed provider deliveries.
    outbox_delivery_failure_total: AtomicU64,
    /// Failed provider deliveries by sanitized low-cardinality reason class.
    outbox_delivery_failure_auth_total: AtomicU64,
    outbox_delivery_failure_quota_total: AtomicU64,
    outbox_delivery_failure_network_total: AtomicU64,
    outbox_delivery_failure_invalid_token_total: AtomicU64,
    outbox_delivery_failure_invalid_payload_total: AtomicU64,
    outbox_delivery_failure_transient_total: AtomicU64,
    /// Claimed rows transitioned to dead-letter by this process.
    outbox_dead_letter_total: AtomicU64,
    /// Invalid-token cleanup database failures.
    invalid_token_cleanup_failures_total: AtomicU64,
    /// Hook invocations rejected by fixed-window rate limiting.
    rate_limit_rejections_total: AtomicU64,
    /// Worker loop is running.
    worker_running: AtomicBool,
    /// Worker shutdown has been requested.
    worker_shutdown_requested: AtomicBool,
}

/// Snapshot of current in-process operational counters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservabilitySnapshot {
    /// Total HTTP requests observed by the router middleware.
    pub http_requests_total: u64,
    /// HTTP responses with status code 2xx.
    pub http_responses_2xx_total: u64,
    /// HTTP responses with status code 4xx.
    pub http_responses_4xx_total: u64,
    /// HTTP responses with status code 5xx.
    pub http_responses_5xx_total: u64,
    /// Delivery worker claim attempts that found a row.
    pub outbox_claims_total: u64,
    /// Delivery worker database claim queries, including idle scans that found no row.
    pub outbox_claim_queries_total: u64,
    /// Delivery worker entries into an idle notification/deadline wait.
    pub outbox_idle_waits_total: u64,
    /// Delivery worker successful provider deliveries.
    pub outbox_delivery_success_total: u64,
    /// Delivery worker failed provider deliveries.
    pub outbox_delivery_failure_total: u64,
    /// Failed provider deliveries by sanitized low-cardinality reason class.
    pub outbox_delivery_failure_auth_total: u64,
    /// Failed provider deliveries classified as provider quota failures.
    pub outbox_delivery_failure_quota_total: u64,
    /// Failed provider deliveries classified as provider network failures.
    pub outbox_delivery_failure_network_total: u64,
    /// Failed provider deliveries classified as permanent invalid-token failures.
    pub outbox_delivery_failure_invalid_token_total: u64,
    /// Failed provider deliveries classified as permanent invalid-payload failures.
    pub outbox_delivery_failure_invalid_payload_total: u64,
    /// Failed provider deliveries classified as generic transient failures.
    pub outbox_delivery_failure_transient_total: u64,
    /// Claimed rows transitioned to dead-letter by this process.
    pub outbox_dead_letter_total: u64,
    /// Invalid-token cleanup database failures.
    pub invalid_token_cleanup_failures_total: u64,
    /// Hook invocations rejected by fixed-window rate limiting.
    pub rate_limit_rejections_total: u64,
    /// Worker loop is running.
    pub worker_running: bool,
    /// Worker shutdown has been requested.
    pub worker_shutdown_requested: bool,
}

impl Observability {
    /// Returns a point-in-time snapshot of counters and worker state.
    #[must_use]
    pub fn snapshot(&self) -> ObservabilitySnapshot {
        ObservabilitySnapshot {
            http_requests_total: self.inner.http_requests_total.load(Ordering::Relaxed),
            http_responses_2xx_total: self.inner.http_responses_2xx_total.load(Ordering::Relaxed),
            http_responses_4xx_total: self.inner.http_responses_4xx_total.load(Ordering::Relaxed),
            http_responses_5xx_total: self.inner.http_responses_5xx_total.load(Ordering::Relaxed),
            outbox_claims_total: self.inner.outbox_claims_total.load(Ordering::Relaxed),
            outbox_claim_queries_total: self
                .inner
                .outbox_claim_queries_total
                .load(Ordering::Relaxed),
            outbox_idle_waits_total: self.inner.outbox_idle_waits_total.load(Ordering::Relaxed),
            outbox_delivery_success_total: self
                .inner
                .outbox_delivery_success_total
                .load(Ordering::Relaxed),
            outbox_delivery_failure_total: self
                .inner
                .outbox_delivery_failure_total
                .load(Ordering::Relaxed),
            outbox_delivery_failure_auth_total: self
                .inner
                .outbox_delivery_failure_auth_total
                .load(Ordering::Relaxed),
            outbox_delivery_failure_quota_total: self
                .inner
                .outbox_delivery_failure_quota_total
                .load(Ordering::Relaxed),
            outbox_delivery_failure_network_total: self
                .inner
                .outbox_delivery_failure_network_total
                .load(Ordering::Relaxed),
            outbox_delivery_failure_invalid_token_total: self
                .inner
                .outbox_delivery_failure_invalid_token_total
                .load(Ordering::Relaxed),
            outbox_delivery_failure_invalid_payload_total: self
                .inner
                .outbox_delivery_failure_invalid_payload_total
                .load(Ordering::Relaxed),
            outbox_delivery_failure_transient_total: self
                .inner
                .outbox_delivery_failure_transient_total
                .load(Ordering::Relaxed),
            outbox_dead_letter_total: self.inner.outbox_dead_letter_total.load(Ordering::Relaxed),
            invalid_token_cleanup_failures_total: self
                .inner
                .invalid_token_cleanup_failures_total
                .load(Ordering::Relaxed),
            rate_limit_rejections_total: self
                .inner
                .rate_limit_rejections_total
                .load(Ordering::Relaxed),
            worker_running: self.inner.worker_running.load(Ordering::Relaxed),
            worker_shutdown_requested: self.inner.worker_shutdown_requested.load(Ordering::Relaxed),
        }
    }

    /// Marks the delivery worker as running or stopped.
    pub fn set_worker_running(&self, running: bool) {
        self.inner.worker_running.store(running, Ordering::Relaxed);
    }

    /// Marks delivery worker shutdown as requested.
    pub fn set_worker_shutdown_requested(&self) {
        self.inner
            .worker_shutdown_requested
            .store(true, Ordering::Relaxed);
    }

    /// Records that the worker claimed one outbox row.
    pub fn record_outbox_claim(&self) {
        self.inner
            .outbox_claims_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records that the worker queried the database for a due outbox row.
    pub fn record_outbox_claim_query(&self) {
        self.inner
            .outbox_claim_queries_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records entry into an idle notification/deadline wait.
    pub fn record_outbox_idle_wait(&self) {
        self.inner
            .outbox_idle_waits_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records a successful provider delivery.
    pub fn record_delivery_success(&self) {
        self.inner
            .outbox_delivery_success_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records a failed provider delivery.
    pub fn record_delivery_failure(&self) {
        self.inner
            .outbox_delivery_failure_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records a failed provider delivery by sanitized reason class.
    pub fn record_delivery_failure_reason(&self, reason: &str) {
        self.record_delivery_failure();
        let counter = match provider_reason_class(reason) {
            "auth" => &self.inner.outbox_delivery_failure_auth_total,
            "quota" => &self.inner.outbox_delivery_failure_quota_total,
            "network" => &self.inner.outbox_delivery_failure_network_total,
            "invalid_token" => &self.inner.outbox_delivery_failure_invalid_token_total,
            "invalid_payload" => &self.inner.outbox_delivery_failure_invalid_payload_total,
            _ => &self.inner.outbox_delivery_failure_transient_total,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a dead-letter transition performed by this process.
    pub fn record_dead_letter(&self) {
        self.record_dead_letters(1);
    }

    /// Records multiple dead-letter transitions performed by this process.
    pub fn record_dead_letters(&self, count: u64) {
        self.inner
            .outbox_dead_letter_total
            .fetch_add(count, Ordering::Relaxed);
    }

    /// Records failure to atomically mark invalid-token and cleanup registration state.
    pub fn record_invalid_token_cleanup_failure(&self) {
        self.inner
            .invalid_token_cleanup_failures_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records a hook invocation rejected by rate limiting.
    pub fn record_rate_limit_rejection(&self) {
        self.inner
            .rate_limit_rejections_total
            .fetch_add(1, Ordering::Relaxed);
    }
}

fn provider_reason_class(reason: &str) -> &'static str {
    // Keep this list synchronized with `/metrics` exposition, docs, and tests
    // whenever provider error reasons change.
    match reason {
        "provider_auth" => "auth",
        "provider_quota" => "quota",
        "provider_network" | "provider_timeout" => "network",
        "invalid_token" => "invalid_token",
        "invalid_payload" => "invalid_payload",
        _ => "transient",
    }
}

/// Adds sanitized request-id logging and status counters around HTTP requests.
pub async fn observe_requests(
    State(observability): State<Observability>,
    mut request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let request_id = request_id();
    if let Ok(header_value) = HeaderValue::from_str(&request_id) {
        request.headers_mut().insert(
            HeaderName::from_static(REQUEST_ID_HEADER),
            header_value.clone(),
        );
    }
    let method = request.method().clone();
    let path = route_label(request.uri().path());
    let start = Instant::now();
    observability
        .inner
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);

    let mut response = next.run(request).await;
    let status = response.status();
    match status.as_u16() {
        200..=226 => {
            observability
                .inner
                .http_responses_2xx_total
                .fetch_add(1, Ordering::Relaxed);
        }
        400..=451 => {
            observability
                .inner
                .http_responses_4xx_total
                .fetch_add(1, Ordering::Relaxed);
        }
        500..=511 => {
            observability
                .inner
                .http_responses_5xx_total
                .fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }

    if let Ok(header_value) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), header_value);
    }

    eprintln!(
        "event=http_request request_id={} method={} route={} status={} latency_ms={}",
        request_id,
        method,
        path,
        status.as_u16(),
        start.elapsed().as_millis()
    );

    response.into_response()
}

fn request_id() -> String {
    random_url_token(12)
}

fn route_label(path: &str) -> &'static str {
    match path {
        "/health" => "/health",
        "/live" => "/live",
        "/ready" => "/ready",
        "/metrics" => "/metrics",
        "/hooks/notification" => "/hooks/notification",
        "/registrations" => "/registrations",
        "/v1/hooks" => "/v1/hooks",
        "/v1/telemetry/registrations" => "/v1/telemetry/registrations",
        _ if path.starts_with("/v1/telemetry/fmans/") && path.ends_with("/metrics") => {
            "/v1/telemetry/fmans/{fman_pubkey}/seats/{seat_id}/metrics"
        }
        _ if path.starts_with("/v1/telemetry/fmans/") => "/v1/telemetry/fmans/{fman_pubkey}/seats",
        _ if path.starts_with("/hooks/") => "/hooks/{hook_id}/{hook_secret}",
        _ if path.starts_with("/registrations/") && path.ends_with("/disable") => {
            "/registrations/{installation_id}/disable"
        }
        _ if path.starts_with("/registrations/") => "/registrations/{installation_id}",
        _ if path.starts_with("/v1/hooks/") => "/v1/hooks/{hook_id}",
        _ => "<unmatched>",
    }
}

#[cfg(test)]
mod tests;
