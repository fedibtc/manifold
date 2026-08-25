//! Shared static-file and SPA routing for embedded operator dashboards.
//!
//! Authentication belongs to each daemon's API routes. Consumers merge this
//! router after applying their API middleware so the dashboard shell and its
//! assets remain reachable before the operator authenticates.

use std::borrow::Cow;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{ALLOW, CACHE_CONTROL, CONTENT_TYPE, HeaderValue, X_CONTENT_TYPE_OPTIONS};
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;
use tower_http::compression::CompressionLayer;

const INDEX_HTML: &str = "index.html";
const NO_STORE: &str = "no-store";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

#[derive(Clone, Copy)]
struct StaticUiState {
    reserved_prefixes: &'static [&'static str],
}

/// Build a public static dashboard router.
///
/// `reserved_prefixes` names daemon-owned route prefixes, without a leading
/// slash. Missing paths under them return 404 instead of the SPA shell.
///
/// Responses are gzipped when the client offers it. The embedded assets are
/// stored uncompressed, and a dashboard is mostly JavaScript, so the daemon
/// owns this rather than leaving it to a proxy that no longer sits in front of
/// it. The default predicate skips tiny bodies and already-compressed media.
pub fn router<E>(reserved_prefixes: &'static [&'static str]) -> Router
where
    E: RustEmbed + Send + Sync + 'static,
{
    Router::new()
        .fallback(serve::<E>)
        .layer(CompressionLayer::new().gzip(true))
        .with_state(StaticUiState { reserved_prefixes })
}

async fn serve<E>(State(state): State<StaticUiState>, method: Method, uri: Uri) -> Response
where
    E: RustEmbed,
{
    let path = uri.path().trim_start_matches('/');
    let asset_exists = path.is_empty() || path == INDEX_HTML || E::get(path).is_some();

    // Preserve the daemon API's 404 contract for missing reserved routes. A
    // non-GET typo must not be reclassified as a static UI method error merely
    // because the dashboard router is embedded.
    if !asset_exists
        && (is_reserved(path, state.reserved_prefixes)
            || is_reserved(path, &["assets", "__control"])
            || path.contains('.'))
    {
        return status_response(StatusCode::NOT_FOUND);
    }

    if method != Method::GET && method != Method::HEAD {
        let mut response = status_response(StatusCode::METHOD_NOT_ALLOWED);
        response
            .headers_mut()
            .insert(ALLOW, HeaderValue::from_static("GET, HEAD"));
        return response;
    }

    if path.is_empty() || path == INDEX_HTML {
        return asset_response::<E>(INDEX_HTML, method == Method::HEAD, NO_STORE);
    }

    if E::get(path).is_some() {
        let cache_control = if path.starts_with("assets/") {
            IMMUTABLE
        } else {
            NO_STORE
        };
        return asset_response::<E>(path, method == Method::HEAD, cache_control);
    }

    // API typos and missing static files must not become a successful SPA
    // shell. Extension-less browser navigation paths do need the shell because
    // both dashboards use createBrowserRouter with real paths.
    asset_response::<E>(INDEX_HTML, method == Method::HEAD, NO_STORE)
}

fn is_reserved(path: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| {
        path == *prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn asset_response<E>(path: &str, head: bool, cache_control: &'static str) -> Response
where
    E: RustEmbed,
{
    let Some(file) = E::get(path) else {
        return status_response(StatusCode::NOT_FOUND);
    };
    let content_type = HeaderValue::from_str(file.metadata.mimetype())
        .expect("rust-embed produces a valid MIME type");
    let body = if head {
        Cow::Borrowed(&[][..])
    } else {
        file.data
    };
    let mut response = Body::from(body).into_response();
    response.headers_mut().insert(CONTENT_TYPE, content_type);
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
    response
        .headers_mut()
        .insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

fn status_response(status: StatusCode) -> Response {
    let mut response = status.into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(NO_STORE));
    response
        .headers_mut()
        .insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use axum::http::header::{ACCEPT_ENCODING, CONTENT_ENCODING};
    use axum::middleware::{self, Next};
    use axum::routing::post;
    use tower::ServiceExt as _;

    use super::*;

    #[derive(rust_embed::Embed)]
    #[folder = "test-ui"]
    struct TestUi;

    #[tokio::test]
    async fn assets_are_public_while_api_routes_remain_protected() {
        async fn require_auth(request: Request<Body>, next: Next) -> Response {
            if request.headers().contains_key("x-test-auth") {
                next.run(request).await
            } else {
                StatusCode::UNAUTHORIZED.into_response()
            }
        }

        let protected = Router::new()
            .route("/api/present", post(|| async { StatusCode::NO_CONTENT }))
            .route_layer(middleware::from_fn(require_auth));
        // Static routes are merged outside the protected API route layer.
        let app = protected.merge(router::<TestUi>(&["api", "admin", "health"]));

        for path in ["/", "/assets/app.js"] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }

        let unauthorized = app
            .clone()
            .oneshot(Request::post("/api/present").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let authorized = app
            .clone()
            .oneshot(
                Request::post("/api/present")
                    .header("x-test-auth", "yes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authorized.status(), StatusCode::NO_CONTENT);

        let index = app
            .clone()
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(index.headers()[CACHE_CONTROL], NO_STORE);
        assert_eq!(index.headers()[X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert!(
            index.headers()[CONTENT_TYPE]
                .to_str()
                .unwrap()
                .contains("text/html")
        );
        assert!(
            !to_bytes(index.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );

        let deep_link = app
            .clone()
            .oneshot(
                Request::get("/allocations/example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deep_link.status(), StatusCode::OK);
        assert_eq!(deep_link.headers()[CACHE_CONTROL], NO_STORE);

        for path in [
            "/api/missing",
            "/admin/missing",
            "/health/missing",
            "/assets/missing.js",
            "/__control",
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }

        let missing_api_post = app
            .clone()
            .oneshot(Request::post("/api/missing").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(missing_api_post.status(), StatusCode::NOT_FOUND);

        let head = app
            .clone()
            .oneshot(Request::head("/assets/app.js").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()[CACHE_CONTROL], IMMUTABLE);
        assert!(
            to_bytes(head.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );

        let method = app
            .oneshot(Request::post("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(method.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(method.headers()[ALLOW], "GET, HEAD");
    }

    #[tokio::test]
    async fn assets_compress_for_clients_that_accept_gzip() {
        let app = router::<TestUi>(&["api"]);
        let raw = TestUi::get("assets/app.js").expect("fixture asset").data;

        let compressed = app
            .clone()
            .oneshot(
                Request::get("/assets/app.js")
                    .header(ACCEPT_ENCODING, "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(compressed.status(), StatusCode::OK);
        assert_eq!(compressed.headers()[CONTENT_ENCODING], "gzip");
        // Compression must not cost the caching and sniffing guarantees.
        assert_eq!(compressed.headers()[CACHE_CONTROL], IMMUTABLE);
        assert_eq!(compressed.headers()[X_CONTENT_TYPE_OPTIONS], "nosniff");
        let encoded = to_bytes(compressed.into_body(), usize::MAX).await.unwrap();
        assert!(encoded.len() < raw.len(), "gzip did not shrink the asset");

        // A client that does not offer gzip still gets the exact file.
        let plain = app
            .oneshot(Request::get("/assets/app.js").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(!plain.headers().contains_key(CONTENT_ENCODING));
        assert_eq!(
            to_bytes(plain.into_body(), usize::MAX).await.unwrap(),
            raw.as_ref()
        );
    }
}
