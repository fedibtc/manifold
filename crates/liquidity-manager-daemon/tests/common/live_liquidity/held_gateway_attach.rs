//! A test-only transport fault: hold one target's attach before it reaches gatewayd.
//! Other requests use the real gateway, including all balance reads and funding.
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Context as _;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse as _, Response};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const BODY_LIMIT: usize = 1024 * 1024;

struct Forwarder {
    upstream: OnceLock<String>,
    held_invite: OnceLock<String>,
    client: reqwest::Client,
    entered: watch::Sender<bool>,
    release: CancellationToken,
    shutdown: CancellationToken,
    requests: Semaphore,
}

pub struct HeldGatewayAttach {
    pub api_url: String,
    entered: watch::Receiver<bool>,
    release: CancellationToken,
    shutdown: CancellationToken,
    server: JoinHandle<std::io::Result<()>>,
    state: Arc<Forwarder>,
}

impl HeldGatewayAttach {
    pub async fn start() -> anyhow::Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let api_url = format!("http://{}", listener.local_addr()?);
        let (entered_tx, entered) = watch::channel(false);
        let release = CancellationToken::new();
        let shutdown = CancellationToken::new();
        let state = Arc::new(Forwarder {
            upstream: OnceLock::new(),
            held_invite: OnceLock::new(),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            entered: entered_tx,
            release: release.clone(),
            shutdown: shutdown.clone(),
            requests: Semaphore::new(16),
        });
        let app = Router::new()
            .fallback(forward)
            .with_state(Arc::clone(&state));
        let stop = shutdown.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
        });
        Ok(Self {
            api_url,
            entered,
            release,
            shutdown,
            server,
            state,
        })
    }

    pub fn set_upstream(&mut self, upstream: &str) -> anyhow::Result<()> {
        self.state
            .upstream
            .set(upstream.trim_end_matches('/').to_owned())
            .map_err(|_| anyhow::anyhow!("gateway forwarder upstream is already set"))
    }

    pub fn hold_invite(&mut self, invite: &str) -> anyhow::Result<()> {
        self.state
            .held_invite
            .set(invite.to_owned())
            .map_err(|_| anyhow::anyhow!("gateway forwarder target is already set"))
    }

    pub async fn wait_until_entered(&mut self) -> anyhow::Result<()> {
        // Bound fixture entry observation without extending any service deadline.
        tokio::time::timeout(
            Duration::from_secs(6),
            self.entered.wait_for(|entered| *entered),
        )
        .await
        .context("allocation did not reach the held gateway attach")??;
        Ok(())
    }

    /// Return an error to the held call, never forward it (even after release).
    pub fn release_with_error(&self) {
        self.release.cancel();
    }

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), &mut self.server)
            .await
            .context("gateway forwarder did not shut down")???;
        Ok(())
    }
}

impl Drop for HeldGatewayAttach {
    fn drop(&mut self) {
        // All handlers select this token, including body reads and upstream IO.
        // No held request has a code path that can later mutate the real gateway.
        self.shutdown.cancel();
        self.server.abort();
    }
}

async fn forward(State(state): State<Arc<Forwarder>>, request: Request) -> Response {
    let Ok(_permit) = state.requests.try_acquire() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    tokio::select! {
        biased;
        () = state.shutdown.cancelled() => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        response = forward_inner(&state, request) => response,
    }
}

async fn forward_inner(state: &Forwarder, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Ok(body) = to_bytes(body, BODY_LIMIT).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    if parts.method == Method::POST && parts.uri.path() == "/connect_fed" {
        // Initial setup only needs read-only probes. Do not let an unexpected
        // attach slip through before the test has armed its target.
        let Some(invite) = state.held_invite.get() else {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };
        let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        if payload["invite_code"].as_str() == Some(invite.as_str()) {
            state.entered.send_replace(true);
            state.release.cancelled().await;
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    }
    let path = parts.uri.path_and_query().map_or("/", |path| path.as_str());
    let Some(upstream) = state.upstream.get() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let mut request = state
        .client
        .request(parts.method, format!("{upstream}{path}"))
        .body(body);
    for name in [header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT] {
        if let Some(value) = parts.headers.get(&name) {
            request = request.header(name, value);
        }
    }
    let Ok(mut upstream) = request.send().await else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let mut response = Response::builder().status(upstream.status());
    if let Some(content_type) = upstream.headers().get(header::CONTENT_TYPE) {
        response = response.header(header::CONTENT_TYPE, content_type);
    }
    let mut bytes = Vec::new();
    loop {
        match upstream.chunk().await {
            Ok(Some(chunk)) if bytes.len() + chunk.len() <= BODY_LIMIT => {
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            _ => return StatusCode::BAD_GATEWAY.into_response(),
        }
    }
    response
        .body(Body::from(bytes))
        .expect("valid gateway response")
}
