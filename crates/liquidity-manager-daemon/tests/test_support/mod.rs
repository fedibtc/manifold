#![allow(dead_code)]

use std::ffi::OsString;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};
use reqwest::{Client, StatusCode};

/// Bootstrap Admin API bearer shared by the daemon-process integration tests.
pub const ADMIN_TOKEN: &str = "flip-local-admin-token";

/// Resolve at execution time so cached Nextest artifacts remain relocatable.
/// Ordinary `cargo test` runs retain Cargo's compile-time binary path fallback.
pub(crate) fn liquidity_manager_daemon_bin() -> OsString {
    std::env::var_os("FLIP_E2E_LIQUIDITY_MANAGER_DAEMON_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_liquidity-manager-daemon").into())
}

pub struct TestPorts {
    pub admin_bind_address: SocketAddr,
    pub public_bind_address: SocketAddr,
}

impl TestPorts {
    pub fn allocate() -> anyhow::Result<Self> {
        let base_port = defe_portalloc::port_alloc(2).context("allocate daemon test ports")?;
        let public_port = base_port
            .checked_add(1)
            .context("allocated daemon test port range overflowed")?;
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
            .expect("system clock is after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir()
            .join("fedi-flip-tests")
            .join(format!("{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path)
            .with_context(|| format!("create test data dir {}", path.display()))?;
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
    pub child: Child,
}

impl DaemonProcess {
    pub fn start(data_dir: &Path, ports: &TestPorts) -> anyhow::Result<Self> {
        Self::start_with_mode(data_dir, ports, false, None)
    }

    /// Start with the development relay routing pointed at a test relay.
    ///
    /// Holder-authorization enrollment reads the environment-pinned relays, so
    /// a test that publishes to its own fixture relay has to say so here rather
    /// than through the operator's advertisement relay config.
    pub fn start_with_relay(
        data_dir: &Path,
        ports: &TestPorts,
        relay_url: &str,
    ) -> anyhow::Result<Self> {
        Self::start_with_mode(data_dir, ports, false, Some(relay_url))
    }

    pub fn start_restore_mode(data_dir: &Path, ports: &TestPorts) -> anyhow::Result<Self> {
        Self::start_with_mode(data_dir, ports, true, None)
    }

    fn start_with_mode(
        data_dir: &Path,
        ports: &TestPorts,
        restore_mode: bool,
        dev_relay_url: Option<&str>,
    ) -> anyhow::Result<Self> {
        let mut command = Command::new(liquidity_manager_daemon_bin());
        command
            .arg("run")
            .arg("daemon")
            .arg("--manifold-environment")
            .arg(if restore_mode {
                "production"
            } else {
                "development"
            })
            .arg("--data-dir")
            .arg(data_dir)
            .arg("--admin-bind-address")
            .arg(ports.admin_bind_address.to_string())
            .arg("--public-bind-address")
            .arg(ports.public_bind_address.to_string())
            .arg("--bootstrap-admin-token")
            .arg(ADMIN_TOKEN);
        if restore_mode {
            command.arg("--restore-mode");
        }
        if let Some(relay_url) = dev_relay_url {
            command.env("MANIFOLD_DEV_NOSTR_RELAYS", relay_url);
        }

        let child = command
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .context("spawn liquidity-manager-daemon")?;

        Ok(Self { child })
    }

    pub fn stop(&mut self) -> anyhow::Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }

        terminate_child(&mut self.child)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
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

/// Wait until the authenticated Admin API can serve requests.
///
/// The unauthenticated `/health` route intentionally reports shell liveness
/// before the normal-mode runtime generation opens SQLite. The smoke tests use
/// runtime-backed Admin API calls immediately afterwards, so they must wait for
/// `get_health` instead.
pub async fn wait_for_admin_ready(
    client: &Client,
    admin_url: &str,
    daemon: &mut DaemonProcess,
) -> anyhow::Result<()> {
    for _ in 0..30 {
        if let Some(status) = daemon.child.try_wait()? {
            anyhow::bail!("daemon exited before health check passed: {status}");
        }

        if let Ok(response) = client
            .post(format!("{admin_url}/admin/v1/get_health"))
            .bearer_auth(ADMIN_TOKEN)
            .send()
            .await
            && response.status() == StatusCode::OK
        {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    anyhow::bail!("authenticated Admin API did not become ready at {admin_url}")
}
