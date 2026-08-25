#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

use anyhow::Context;
use tokio::time::Duration;

use super::bitcoin::BitcoinFixture;
use super::{ManagedProcess, locate_binary, process_log_path};

/// An esplora (electrs) indexer over the leased regtest bitcoind.
///
/// The Fedimint wallet **client** watches the chain only through an esplora
/// HTTP API (its non-user RPC path is `create_esplora_rpc`), so the FLIP
/// target-federation client cannot claim a peg-in against a bitcoind-only
/// backend. This mirrors the `tests-e2e` fleet-manager harness, which spawns
/// the same indexer for the FI wallet's peg-in.
pub struct EsploraFixture {
    /// Base URL to give the client via `FM_FORCE_BITCOIN_RPC_URL`.
    pub http_url: String,
    process: ManagedProcess,
}

impl EsploraFixture {
    pub async fn start(bitcoin: &BitcoinFixture, data_root: &Path) -> anyhow::Result<Self> {
        let http_port = defe_portalloc::port_alloc(3).context("allocate esplora ports")?;
        let monitoring_port = http_port
            .checked_add(1)
            .context("esplora monitoring port overflow")?;
        // electrs also binds an Electrum RPC port; unset, it falls back to the
        // fixed regtest default (60401) and collides across concurrent or
        // leftover instances. Pin it to an allocated port even though the test
        // reads esplora only over HTTP.
        let electrum_port = http_port
            .checked_add(2)
            .context("esplora electrum port overflow")?;
        let db_dir = data_root.join("esplora-db");
        std::fs::create_dir_all(&db_dir)
            .with_context(|| format!("create esplora db dir {}", db_dir.display()))?;

        let esplora_bin = locate_binary("FLIP_E2E_ESPLORA_BIN", "esplora")?;
        let info = bitcoin.info();
        let mut command = Command::new(esplora_bin);
        command
            .arg(format!("--daemon-dir={}", info.data_dir.display()))
            .arg(format!("--db-dir={}", db_dir.display()))
            .arg(format!(
                "--cookie={}:{}",
                info.rpc_username, info.rpc_password
            ))
            .arg("--network=regtest")
            .arg(format!(
                "--daemon-rpc-addr={}:{}",
                info.rpc_host, info.rpc_port
            ))
            .arg(format!("--http-addr=127.0.0.1:{http_port}"))
            .arg(format!("--monitoring-addr=127.0.0.1:{monitoring_port}"))
            .arg(format!("--electrum-rpc-addr=127.0.0.1:{electrum_port}"))
            // Index over RPC instead of reading bitcoind's on-disk block files,
            // which a defe-managed node may keep in an incompatible format.
            .arg("--jsonrpc-import");
        let process = ManagedProcess::spawn(
            "esplora",
            &mut command,
            process_log_path(data_root, "esplora"),
        )?;

        let mut this = Self {
            // Trailing slash: matches the URL shape fedimint's esplora RPC expects.
            http_url: format!("http://127.0.0.1:{http_port}/"),
            process,
        };
        this.wait_for_http(http_port, Duration::from_secs(60))
            .await?;
        Ok(this)
    }

    async fn wait_for_http(&mut self, port: u16, timeout: Duration) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            self.process.ensure_running()?;
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("timed out waiting for esplora HTTP on 127.0.0.1:{port}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
