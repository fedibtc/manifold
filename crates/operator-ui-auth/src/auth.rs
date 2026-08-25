use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::CookieJar;

use crate::UiState;

/// Tower middleware protecting an API router with the authenticated session.
pub async fn require_auth<Api>(
    State(state): State<UiState<Api>>,
    jar: CookieJar,
    request: Request,
    next: Next,
) -> Result<Response, axum::http::StatusCode>
where
    Api: Clone + Send + Sync + 'static,
{
    if !state.requires_auth
        || jar
            .get(&state.auth_cookie_name)
            .is_some_and(|cookie| cookie.value() == state.auth_cookie_value)
    {
        return Ok(next.run(request).await);
    }

    Err(axum::http::StatusCode::UNAUTHORIZED)
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::header::{COOKIE, SET_COOKIE};
    use axum::http::{Request, StatusCode};
    use axum::middleware;
    use axum::response::IntoResponse as _;
    use axum::routing::get;
    use tower::ServiceExt as _;

    use super::*;

    async fn protected() -> StatusCode {
        StatusCode::OK
    }

    fn protected_router(requires_auth: bool) -> Router {
        let state = UiState::new((), requires_auth);
        Router::new()
            .route("/", get(protected))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                require_auth::<()>,
            ))
            .with_state(state)
    }

    #[tokio::test]
    async fn password_mode_rejects_requests_without_session_cookie() {
        let response = protected_router(true)
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn valid_password_issues_cookie_accepted_by_layer() {
        let state = UiState::new((), true);
        let jar = crate::authenticate_password(
            &fedimint_core::module::ApiAuth::new("generated-password".to_owned()),
            state.auth_cookie_name.clone(),
            state.auth_cookie_value.clone(),
            CookieJar::new(),
            &crate::LoginInput {
                password: "generated-password".to_owned(),
            },
        )
        .expect("valid password");
        let response = (jar, StatusCode::NO_CONTENT).into_response();
        let cookie = response.headers()[SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap();

        let response = Router::new()
            .route("/", get(protected))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                require_auth::<()>,
            ))
            .with_state(state)
            .oneshot(
                Request::get("/")
                    .header(COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn trusted_proxy_mode_skips_local_login() {
        let response = protected_router(false)
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
