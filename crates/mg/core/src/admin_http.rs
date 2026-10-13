//! Authenticated browser-facing operator API.
//!
//! This adapter invokes the same in-process admin operations as `admin.sock`.
//! Authentication session mechanics come from the locally adapted
//! `operator-ui-auth` crate; operation ownership remains in this crate.
//!
//! The listener is bound once and serves two phases
//! ([SPEC-operator-http](../../specs/SPEC-operator-http.md)). Before an
//! identity exists it answers the onboarding verbs and refuses the rest with
//! `not_onboarded`; the dashboard merged into this same router is what an
//! operator uses to answer. When the fleet opens, [`admin::OperatorPhase::open_fleet`]
//! switches the dispatcher in place — the port an operator loaded the wizard on
//! is the port that then serves it.

use std::net::SocketAddr;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::http::header::CACHE_CONTROL;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use fedimint_core::module::ApiAuth;
use operator_ui_auth::auth::require_auth;
use operator_ui_auth::{LoginInput, UiState, authenticate_password};
use serde_json::Value;

use crate::admin::{self, AdminRequest, OperatorPhase};

const AUTH_API_ROUTE: &str = "/api/auth";
const ADMIN_API_ROUTE: &str = "/api/admin";

#[derive(Clone)]
struct AdminHttpState {
    phase: OperatorPhase,
    password: Option<ApiAuth>,
}

/// Authentication boundary selected by the deployment package.
pub enum AdminHttpAuth {
    /// The listener is reachable only through an authenticating platform proxy.
    TrustedProxy,
    /// Verify this password through the authentication API.
    Password(String),
}

/// Build the operator API router.
///
/// The authentication boundary is fixed here and does not move with the phase.
/// The password comes from a file the deployment wrote, not from the identity,
/// so password mode protects the onboarding phase exactly as it protects the
/// fleet — an operator signs in, then sets the host up.
pub fn router(phase: &OperatorPhase, auth: AdminHttpAuth) -> Router {
    let password = match auth {
        AdminHttpAuth::TrustedProxy => None,
        AdminHttpAuth::Password(password) => Some(ApiAuth::new(password)),
    };
    let requires_auth = password.is_some();
    let state = UiState::new(
        AdminHttpState {
            phase: phase.clone(),
            password,
        },
        requires_auth,
    );

    let protected = Router::new()
        .route(ADMIN_API_ROUTE, post(dispatch))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth::<AdminHttpState>,
        ));
    let public = if requires_auth {
        Router::new().route(AUTH_API_ROUTE, post(authenticate))
    } else {
        Router::new()
    };

    public
        .merge(protected)
        .layer(middleware::from_fn(no_store))
        .with_state(state)
}

/// Bind and serve a composed browser-facing operator router.
///
/// Returns the address actually bound, which is not always the one asked for:
/// a deployment may ask for port 0.
pub async fn serve(
    router: Router,
    bind: SocketAddr,
) -> anyhow::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let bound = listener.local_addr()?;
    let server = axum::serve(listener, router.into_make_service());
    Ok((
        bound,
        tokio::spawn(async move {
            if let Err(error) = server.await {
                tracing::warn!(%error, "operator HTTP server stopped");
            }
        }),
    ))
}

async fn authenticate(
    State(state): State<UiState<AdminHttpState>>,
    jar: CookieJar,
    Json(input): Json<LoginInput>,
) -> Response {
    let auth = state
        .api
        .password
        .expect("authentication route is mounted only in password mode");
    match authenticate_password(
        &auth,
        state.auth_cookie_name,
        state.auth_cookie_value,
        jar,
        &input,
    ) {
        Some(jar) => (jar, StatusCode::NO_CONTENT).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

#[cfg(test)]
#[path = "../tests/admin_http.rs"]
mod tests;

async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, "no-store".parse().expect("static header"));
    response
}

async fn dispatch(
    State(state): State<UiState<AdminHttpState>>,
    Json(request): Json<AdminRequest>,
) -> Response {
    let answered = state.api.phase.answer(request).await;
    let response: Result<Value, admin::AdminError> =
        answered.map_err(|error| admin::AdminError::from_error(&error));
    Json(response).into_response()
}
