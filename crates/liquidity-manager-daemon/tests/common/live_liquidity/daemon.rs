#![allow(dead_code)]

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::test_support::liquidity_manager_daemon_bin;
use anyhow::{Context, ensure};
use fedi_iroh_rpc::iroh::EndpointAddr;
use reqwest::{Client, StatusCode};
use serde_json::Value;

use super::POLL_INTERVAL;

pub const ADMIN_TOKEN: &str = "flip-local-admin-token";

/// Boot inputs shared by every spawn of one daemon instance: the imported
/// provider Schnorr signing key and the `--trust-fixtures` directory
/// substituting the federation preview and FMan trust-material inputs.
/// Restarts must reuse the same launch so the provider identity and fixture
/// content stay stable across the daemon lifetime.
pub struct DaemonLaunch {
    pub trust_fixtures_dir: PathBuf,
    pub provider_secret_hex: String,
    /// Esplora HTTP base URL forced onto the daemon's target-federation
    /// Fedimint client (via `FM_FORCE_BITCOIN_RPC_*`) so its wallet client can
    /// watch and claim peg-ins. Only the stability-pool path needs it.
    pub esplora_http_url: Option<String>,
    /// Relay this daemon should read Holder authorizations from. Enrollment
    /// reads the environment-pinned relays, so the harness points development
    /// routing at its own leased relay rather than the built-in default.
    pub holder_authorization_relay_url: Option<String>,
}

impl DaemonLaunch {
    pub fn new(data_dir: &Path) -> anyhow::Result<Self> {
        let trust_fixtures_dir = data_dir.join("trust-fixtures");
        fs::create_dir_all(&trust_fixtures_dir)
            .with_context(|| format!("create {}", trust_fixtures_dir.display()))?;
        Ok(Self {
            trust_fixtures_dir,
            provider_secret_hex: nostr_sdk::Keys::generate().secret_key().to_secret_hex(),
            esplora_http_url: None,
            holder_authorization_relay_url: None,
        })
    }

    pub fn provider_keys(&self) -> anyhow::Result<nostr_sdk::Keys> {
        nostr_sdk::Keys::parse(&self.provider_secret_hex).context("parse launch provider key")
    }
}

pub struct TestPorts {
    pub admin_bind_address: SocketAddr,
    pub public_bind_address: SocketAddr,
}

impl TestPorts {
    pub fn allocate() -> anyhow::Result<Self> {
        let base_port = defe_portalloc::port_alloc(2).context("allocate daemon ports")?;
        let public_port = base_port
            .checked_add(1)
            .context("allocated daemon port range overflowed")?;
        Ok(Self {
            admin_bind_address: SocketAddr::from(([127, 0, 0, 1], base_port)),
            public_bind_address: SocketAddr::from(([127, 0, 0, 1], public_port)),
        })
    }
}

pub struct TestDataDir {
    path: PathBuf,
}

impl TestDataDir {
    pub fn new(name: &str) -> anyhow::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_nanos();
        let path = std::env::temp_dir()
            .join("fedi-flip-tests")
            .join(format!("{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDataDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub struct DaemonProcess {
    child: Child,
}

impl DaemonProcess {
    pub fn start(
        data_dir: &Path,
        ports: &TestPorts,
        launch: &DaemonLaunch,
    ) -> anyhow::Result<Self> {
        let mut command = Command::new(liquidity_manager_daemon_bin());
        command
            .arg("run")
            .arg("daemon")
            .arg("--manifold-environment")
            .arg("development")
            .arg("--data-dir")
            .arg(data_dir)
            .arg("--admin-bind-address")
            .arg(ports.admin_bind_address.to_string())
            .arg("--public-bind-address")
            .arg(ports.public_bind_address.to_string())
            .arg("--bootstrap-admin-token")
            .arg(ADMIN_TOKEN)
            // Single production flow: Schnorr auth with the imported provider
            // key; only the federation preview and FMan trust-material inputs
            // are substituted with the harness-written fixture files.
            .arg("--trust-fixtures")
            .arg(&launch.trust_fixtures_dir)
            // The harness runs its federation on loopback, and the target-client
            // join applies the address policy before the slot, the open, and the
            // dial. Without this the join refuses `ws://127.0.0.1:...` and the
            // allocation never reaches a wallet operation. This is the flag's
            // stated purpose - it is off by default and refused on mainnet, and
            // this daemon runs `--manifold-environment development`.
            .arg("--allow-private-federation-endpoints")
            .env(
                "FLIP_PROVIDER_NOSTR_SECRET_KEY",
                &launch.provider_secret_hex,
            )
            // The daemon hosts the target-federation Fedimint client, whose
            // background peg-in monitor uses `is_running_in_test_env()` to pick
            // aggressive (100ms) vs production (minutes) polling. Set it
            // explicitly (as aqueduct's harness does) so it does not depend on
            // nextest's marker propagating across the spawn.
            .env("FM_IN_DEVIMINT", "1");
        if let Some(relay_url) = &launch.holder_authorization_relay_url {
            command.env("MANIFOLD_DEV_NOSTR_RELAYS", relay_url);
        }
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());

        // Force the target client's wallet RPC to a real esplora when one is
        // provided (stability-pool path): the 0.12 wallet client watches the
        // chain only through esplora, so a bitcoind-only backend can never see
        // or claim the peg-in.
        if let Some(esplora_http_url) = &launch.esplora_http_url {
            command
                .env("FM_FORCE_BITCOIN_RPC_KIND", "esplora")
                .env("FM_FORCE_BITCOIN_RPC_URL", esplora_http_url);
        }

        let child = command.spawn().context("spawn liquidity-manager-daemon")?;
        Ok(Self { child })
    }

    pub fn stop(&mut self) -> anyhow::Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }

        terminate_child(&mut self.child)?;
        // Derived from what the daemon is allowed to take, not from what it
        // usually takes. Runtime teardown calls `TargetFedimintClients::
        // shutdown_all`, which shuts each target client down in turn, and
        // fedimint's `ClientHandle::shutdown` awaits
        // `shutdown_join_all(Some(Duration::from_secs(30)))`. So one target
        // client alone may legitimately hold teardown for 30 s.
        //
        // The previous 10 s bound was therefore under-provisioned rather than
        // generous, and it failed on CI at 9.97 s while passing locally - a
        // test that depends on machine load. This bound still catches a real
        // hang, because the join it waits for is itself bounded at 30 s.
        let deadline = std::time::Instant::now() + Duration::from_secs(45);
        while std::time::Instant::now() < deadline {
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }

        self.child
            .kill()
            .context("kill daemon after SIGTERM timeout")?;
        let _ = self.child.wait();
        anyhow::bail!("daemon did not stop after SIGTERM")
    }

    pub fn ensure_running(&mut self) -> anyhow::Result<()> {
        if let Some(status) = self.child.try_wait()? {
            anyhow::bail!("daemon exited: {status}");
        }
        Ok(())
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(unix)]
fn terminate_child(child: &mut Child) -> anyhow::Result<()> {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    const SIGTERM: i32 = 15;
    let result = unsafe { kill(child.id() as i32, SIGTERM) };
    ensure!(result == 0, "failed to send SIGTERM to daemon");
    Ok(())
}

#[cfg(not(unix))]
fn terminate_child(child: &mut Child) -> anyhow::Result<()> {
    child.kill().context("terminate daemon")
}

/// Waits until the daemon can actually serve an Admin API request.
///
/// Deliberately not a poll of unauthenticated `/health`. That answers 200 while
/// the process has no runtime generation — it is how an operator watches a live
/// restore land — but every `/admin/v1/*` route answers 503 until the
/// generation is installed, and the Admin listener binds before it. Waiting on
/// `/health` therefore races daemon startup: the caller's first admin call can
/// arrive inside that window and fail with 503, which on a loaded machine
/// running several test partitions at once is wide enough to lose.
///
/// Polling an authenticated route instead tests the precondition every caller
/// actually has.
pub async fn wait_for_health(
    client: &Client,
    admin_url: &str,
    daemon: &mut DaemonProcess,
) -> anyhow::Result<()> {
    for _ in 0..60 {
        daemon.ensure_running()?;
        if let Ok(response) = client
            .post(format!("{admin_url}/admin/v1/get_health"))
            .bearer_auth(ADMIN_TOKEN)
            .json(&serde_json::json!({}))
            .send()
            .await
            && response.status() == StatusCode::OK
        {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("daemon did not begin serving Admin API requests at {admin_url}")
}

/// Posts an admin request, treating `503 Service Unavailable` as "not yet".
///
/// The daemon answers `Unavailable` while a dependency it must reach is still
/// coming up, and under the parallel suite gatewayd, electrs, the relay and a
/// target federation are all warming at once on one machine. That makes a 503
/// during setup a normal transient rather than a failure — but
/// `error_for_status` cannot tell the two apart, so a single one would end the
/// test. Bounded, so a dependency that never arrives still fails.
pub async fn admin_post_when_available(
    http: &Client,
    admin_url: &str,
    method: &str,
    body: &Value,
) -> anyhow::Result<Value> {
    let mut last_status = None;
    for _ in 0..600 {
        let response = http
            .post(format!("{admin_url}/admin/v1/{method}"))
            .bearer_auth(ADMIN_TOKEN)
            .json(body)
            .send()
            .await
            .with_context(|| format!("send admin request {method}"))?;
        if response.status() != StatusCode::SERVICE_UNAVAILABLE {
            return response
                .error_for_status()
                .with_context(|| format!("admin request {method} failed"))?
                .json()
                .await
                .with_context(|| format!("decode admin response {method}"));
        }
        last_status = Some(response.status());
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("admin request {method} stayed unavailable ({last_status:?})")
}

pub async fn admin_post(
    http: &Client,
    admin_url: &str,
    method: &str,
    body: &Value,
) -> anyhow::Result<Value> {
    http.post(format!("{admin_url}/admin/v1/{method}"))
        .bearer_auth(ADMIN_TOKEN)
        .json(body)
        .send()
        .await
        .with_context(|| format!("send admin request {method}"))?
        .error_for_status()
        .with_context(|| format!("admin request {method} failed"))?
        .json()
        .await
        .with_context(|| format!("decode admin response {method}"))
}

pub async fn wait_for_endpoint_addr(
    data_dir: &Path,
    daemon: &mut DaemonProcess,
) -> anyhow::Result<EndpointAddr> {
    let path = data_dir.join("public-iroh-endpoint-addr.json");
    for _ in 0..60 {
        daemon.ensure_running()?;
        if let Ok(bytes) = tokio::fs::read(&path).await {
            return serde_json::from_slice(&bytes)
                .with_context(|| format!("parse {}", path.display()));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("timed out waiting for {}", path.display())
}
