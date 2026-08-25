use std::net::SocketAddr;

use axum::{
    Router,
    extract::{DefaultBodyLimit, connect_info::ConnectInfo},
    http::{Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    routing::post,
};

use crate::{
    AppState,
    health::health_compat,
    health::liveness,
    health::metrics,
    health::readiness,
    hook_management::create_hook,
    hook_management::list_hooks,
    hook_management::revoke_hook,
    invoke_hook::invoke_hook,
    notify::notification_hook,
    register::disable_installation,
    register::register_installation,
    register::unregister_installation,
    telemetry_collector::{list_guardian_telemetry_seats, scrape_guardian_telemetry},
    telemetry_receiver::register_guardian_telemetry,
};

/// Builds the public push gateway Axum router.
///
/// This is a compatibility alias for [`public_app`]. New server code should use
/// [`public_app`] for the public listener and [`operator_app`] for the optional
/// operator listener so the route split is explicit at call sites.
pub fn app(state: AppState) -> Router {
    public_app(state)
}

/// Builds the public push gateway Axum router.
///
/// The public router exposes hook/API routes plus `/health` and `/live`. It does
/// not expose `/ready` or `/metrics` by default. `/metrics` is mounted without
/// operator authentication only when `PUSH_GATEWAY_PUBLIC_METRICS_ENABLED=true`.
/// If `PUSH_GATEWAY_OPERATOR_TOKEN` is configured without a separate
/// `PUSH_GATEWAY_OPERATOR_BIND`, `/ready`, the guardian telemetry target route,
/// and, unless public metrics opt-in is enabled, `/metrics` are mounted here as
/// a token-protected fallback operator surface. When public metrics opt-in is
/// enabled, `/metrics` remains unauthenticated while the other two routes stay
/// token protected.
pub fn public_app(state: AppState) -> Router {
    let observability = state.observability();
    let mut router = Router::new()
        .route("/health", get(health_compat))
        .route("/live", get(liveness))
        .route(
            "/hooks/{hook_id}/{hook_secret}",
            post(invoke_hook).route_layer(middleware::from_fn_with_state(
                state.clone(),
                rate_limit_hook_attempt,
            )),
        )
        .route("/registrations", post(register_installation))
        .route(
            "/v1/telemetry/registrations",
            post(register_guardian_telemetry),
        )
        .route(
            "/registrations/{installation_id}",
            axum::routing::delete(unregister_installation),
        )
        .route(
            "/registrations/{installation_id}/disable",
            post(disable_installation),
        )
        .route("/v1/hooks", post(create_hook).get(list_hooks))
        .route("/v1/hooks/{hook_id}", axum::routing::delete(revoke_hook));
    if state.config().legacy_notification_hook_enabled() {
        router = router.route("/hooks/notification", post(notification_hook));
    }

    if state.config().public_metrics_enabled() {
        router = router.route("/metrics", get(metrics));
    }

    if state.config().operator_token().is_some() && state.config().operator_bind().is_none() {
        let mut operator_routes = Router::new()
            .route("/ready", get(readiness))
            .route(
                "/v1/telemetry/fmans/{fman_pubkey}/seats",
                get(list_guardian_telemetry_seats),
            )
            .route(
                "/v1/telemetry/fmans/{fman_pubkey}/seats/{seat_id}/metrics",
                get(scrape_guardian_telemetry),
            );
        if !state.config().public_metrics_enabled() {
            operator_routes = operator_routes.route("/metrics", get(metrics));
        }
        router = router.merge(operator_routes.route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_operator_token,
        )));
    }

    router
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn(ensure_connect_info))
        .layer(middleware::from_fn_with_state(
            observability,
            crate::observability::observe_requests,
        ))
        .with_state(state)
}

/// Builds the operator push gateway Axum router.
///
/// The operator router exposes `/ready` and `/metrics` in addition to `/health`
/// and `/live`. It is intended to be served on `PUSH_GATEWAY_OPERATOR_BIND`,
/// normally loopback or a trusted monitoring network. When
/// `PUSH_GATEWAY_OPERATOR_TOKEN` is configured, every route on this router
/// requires `Authorization: Bearer $PUSH_GATEWAY_OPERATOR_TOKEN`.
pub fn operator_app(state: AppState) -> Router {
    let observability = state.observability();
    let mut router = Router::new()
        .route("/health", get(health_compat))
        .route("/live", get(liveness))
        .route("/ready", get(readiness))
        .route("/metrics", get(metrics));
    router = router
        .route(
            "/v1/telemetry/fmans/{fman_pubkey}/seats",
            get(list_guardian_telemetry_seats),
        )
        .route(
            "/v1/telemetry/fmans/{fman_pubkey}/seats/{seat_id}/metrics",
            get(scrape_guardian_telemetry),
        );

    if state.config().operator_token().is_some() {
        router = router.route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_operator_token,
        ));
    }

    router
        .layer(middleware::from_fn(ensure_connect_info))
        .layer(middleware::from_fn_with_state(
            observability,
            crate::observability::observe_requests,
        ))
        .with_state(state)
}

async fn ensure_connect_info(mut request: axum::extract::Request, next: Next) -> Response {
    if request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_none()
    {
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:0"
                .parse::<SocketAddr>()
                .expect("static socket addr"),
        ));
    }
    next.run(request).await
}

async fn require_operator_token(
    state: axum::extract::State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(expected_token) = state.config().operator_token() else {
        return Ok(next.run(request).await);
    };
    let Some(authorization) = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let Some(token) = authorization.strip_prefix("Bearer ") else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    if !constant_time_eq(token.as_bytes(), expected_token.as_str().as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

async fn rate_limit_hook_attempt(
    state: axum::extract::State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let limits = state.config().rate_limits();
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(peer)| *peer);
    let source = crate::rate_limits::effective_source_prefix(
        peer,
        request.headers(),
        limits.trusted_proxy_cidrs.as_ref(),
    );
    if limits.hook_invocations_per_source_prefix > 0
        && !state
            .rate_limiters()
            .check(
                crate::rate_limits::RateLimitFamily::HookInvocationSource,
                format!("hook-attempt:{source}"),
                limits.hook_invocations_per_source_prefix,
                limits.hook_invocation_window_seconds,
            )
            .await
    {
        state.observability().record_rate_limit_rejection();
        return Ok(crate::hook_management::rate_limited(
            "source_rate_limited",
            "source rate limited",
        )
        .into_response());
    }
    Ok(next.run(request).await)
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
