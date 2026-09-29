//! The actual bundled guardian must reach Hello without probing Bitcoin backends.
//!
//! These process tests remain opt-in under GATE-selfci-local-development-cost:
//! `cargo test -p fman --features bitcoin-fallback-startup-tests --test
//! bitcoin_fallback_startup -- --ignored`

#![cfg(unix)]

use std::os::fd::OwnedFd;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn};
use axum::routing::{get, post};
use fedimint_server::config::driven::{ChildState, DrivenDkgClient};
use serde_json::{Value, json};
use tokio::process::Command;

/// Abort the loopback server even if a regression assertion panics.
struct Server {
    /// URL passed to the real guardian process.
    url: String,
    /// Server lifetime, independent of the spawned guardian.
    task: tokio::task::JoinHandle<()>,
    /// Requests observed before the guardian is stopped after its Hello.
    requests: Arc<AtomicUsize>,
}

impl Server {
    async fn start(router: axum::Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&requests);
        let router = router.layer(from_fn(move |request, next: Next| {
            observed.fetch_add(1, Ordering::SeqCst);
            async move { next.run(request).await }
        }));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            url,
            task,
            requests,
        }
    }

    /// Construction must not contact either backend before driven-DKG setup.
    fn assert_unused(&self) {
        assert_eq!(
            self.requests.load(Ordering::SeqCst),
            0,
            "guardian must not probe {} before driven-DKG setup",
            self.url
        );
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A responsive Core endpoint, which startup must no longer probe.
async fn core(Json(request): Json<Value>) -> Json<Value> {
    assert_eq!(request["method"], "getblockhash");
    assert_eq!(request["params"], json!([1]));
    Json(json!({
        "result": "00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048",
        "error": null,
        "id": request["id"],
    }))
}

async fn assert_hello(primary: &Server, fallback_url: &str) {
    let data = tempfile::tempdir().unwrap();
    let (parent, child) = std::os::unix::net::UnixStream::pair().unwrap();
    parent.set_nonblocking(true).unwrap();
    let parent = tokio::net::UnixStream::from_std(parent).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_fleet-manager"));
    command
        .arg0(fman_core::bundled_fedimintd::ARGV0)
        .env_clear()
        .env("FM_DKG_CTRL", "1")
        .env("FMAN_E2E_LOCAL_IROH", "1")
        .args(["--bind-metrics", "127.0.0.1:0"])
        .args(["--data-dir", data.path().to_str().unwrap()])
        .args(["--bitcoind-url", &primary.url])
        .args(["--bitcoind-username", "test"])
        .args(["--bitcoind-password", "test"])
        .args(["--esplora-url", fallback_url])
        .stdin(Stdio::from(OwnedFd::from(child)))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    for key in ["LD_LIBRARY_PATH", "RUST_BACKTRACE"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let mut child = command.spawn().unwrap();
    drop(command);
    // FMan's production Hello budget is 30s. Leave a wide margin, without
    // waiting for the Esplora client's 60s network timeout on a regression.
    let hello =
        tokio::time::timeout(Duration::from_secs(10), DrivenDkgClient::connect(parent)).await;
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    let client = hello
        .expect("Bitcoin backends must not hold guardian Hello hostage")
        .expect("guardian must send a valid Hello");
    assert!(matches!(client.child_state(), ChildState::NeedsParams));
    primary.assert_unused();
}

#[tokio::test]
#[ignore = "opt-in bundled-process regression; see module documentation"]
async fn healthy_core_starts_guardian_when_fallback_stalls() {
    let primary = Server::start(axum::Router::new().route("/", post(core))).await;
    let fallback = Server::start(
        axum::Router::new().route("/block-height/1", get(std::future::pending::<StatusCode>)),
    )
    .await;
    assert_hello(&primary, &fallback.url).await;
    fallback.assert_unused();
}

#[tokio::test]
#[ignore = "opt-in bundled-process regression; see module documentation"]
async fn healthy_core_starts_guardian_when_fallback_returns_forbidden() {
    let primary = Server::start(axum::Router::new().route("/", post(core))).await;
    let fallback = Server::start(
        axum::Router::new().route("/block-height/1", get(|| async { StatusCode::FORBIDDEN })),
    )
    .await;
    assert_hello(&primary, &fallback.url).await;
    fallback.assert_unused();
}

#[tokio::test]
#[ignore = "opt-in bundled-process regression; see module documentation"]
async fn healthy_core_starts_guardian_when_fallback_refuses_connection() {
    let primary = Server::start(axum::Router::new().route("/", post(core))).await;
    // Keep the port bound, but deliberately never listen, so no unrelated
    // process can take the port between selecting it and spawning the child.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    assert_hello(
        &primary,
        &format!("http://{}", socket.local_addr().unwrap()),
    )
    .await;
}

#[tokio::test]
#[ignore = "opt-in bundled-process regression; see module documentation"]
async fn guardian_sends_hello_without_probing_either_stalled_backend() {
    let primary =
        Server::start(axum::Router::new().route("/", post(std::future::pending::<StatusCode>)))
            .await;
    let fallback = Server::start(
        axum::Router::new().route("/block-height/1", get(std::future::pending::<StatusCode>)),
    )
    .await;
    assert_hello(&primary, &fallback.url).await;
    fallback.assert_unused();
}
