#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use tokio::time::{Duration, sleep};

use super::bitcoin::BitcoinFixture;
use super::{
    ManagedProcess, POLL_INTERVAL, locate_binary, process_log_path, run_command,
    use_devimint_if_under_defe,
};

pub const GATEWAY_PASSWORD: &str = "testpassword";

pub struct GatewayFixture {
    pub api_url: String,
    pub password: String,
    gateway_cli: PathBuf,
    process: ManagedProcess,
}

impl GatewayFixture {
    pub async fn start(
        test_id: &str,
        bitcoin: &BitcoinFixture,
        data_root: &Path,
    ) -> anyhow::Result<Self> {
        let base_port = defe_portalloc::port_alloc(4).context("allocate gatewayd ports")?;
        let metrics_port = base_port
            .checked_add(1)
            .context("gatewayd metrics port overflow")?;
        let ldk_port = base_port
            .checked_add(2)
            .context("gatewayd LDK port overflow")?;
        let iroh_port = base_port
            .checked_add(3)
            .context("gatewayd Iroh port overflow")?;
        let api_url = format!("http://127.0.0.1:{base_port}");
        let data_dir = data_root.join("gatewayd");
        fs::create_dir_all(&data_dir)
            .with_context(|| format!("create gatewayd data dir {}", data_dir.display()))?;

        let gatewayd = locate_binary("FLIP_E2E_GATEWAYD_BIN", "gatewayd")?;
        let gateway_cli = locate_binary("FLIP_E2E_GATEWAY_CLI_BIN", "gateway-cli")?;
        let ldk_alias = format!("flip-live-ldk-{test_id}");
        let mut command = Command::new(gatewayd);
        command
            .arg("--data-dir")
            .arg(&data_dir)
            .arg("--listen")
            .arg(format!("127.0.0.1:{base_port}"))
            .arg("--api-addr")
            .arg(&api_url)
            .arg("--network")
            .arg("regtest")
            .arg("--iroh-listen")
            .arg(format!("127.0.0.1:{iroh_port}"))
            .arg("--num-route-hints")
            .arg("0")
            .arg("ldk")
            .arg("--ldk-lightning-port")
            .arg(ldk_port.to_string())
            .arg("--ldk-alias")
            .arg(&ldk_alias)
            .env("RUST_LOG", "info")
            .env("FM_GATEWAY_LIGHTNING_MODULE_MODE", "LNv1")
            .env(
                "FM_GATEWAY_METRICS_LISTEN_ADDR",
                format!("127.0.0.1:{metrics_port}"),
            )
            .env(
                "FM_GATEWAY_BCRYPT_PASSWORD_HASH",
                "$2b$12$Etlumnzi/VJ0Ky0Dqoe55eCbvDXItj94thfhvu2o423ox7os.6XfC",
            )
            .env("FM_GATEWAY_SKIP_SETUP", "true")
            .env("FM_BITCOIND_URL", bitcoin.host_rpc_url())
            .env("FM_BITCOIND_USERNAME", bitcoin.rpc_username())
            .env("FM_BITCOIND_PASSWORD", bitcoin.rpc_password())
            .env("FM_DEFAULT_ROUTING_FEES", "2000,5000")
            .env("FM_DEFAULT_TRANSACTION_FEES", "2000,5000")
            .env("FM_PORT_LDK", ldk_port.to_string())
            .env("FM_LDK_ALIAS", &ldk_alias)
            .env(
                "FM_GATEWAY_MNEMONIC",
                "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            );
        use_devimint_if_under_defe(&mut command);
        let process = ManagedProcess::spawn(
            "gatewayd",
            &mut command,
            process_log_path(data_root, "gatewayd"),
        )?;

        let mut this = Self {
            api_url,
            password: GATEWAY_PASSWORD.to_owned(),
            gateway_cli,
            process,
        };
        this.wait_until_ready().await?;
        Ok(this)
    }

    async fn wait_until_ready(&mut self) -> anyhow::Result<()> {
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        loop {
            self.process.ensure_running()?;
            let mut command = Command::new(&self.gateway_cli);
            command
                .arg("-a")
                .arg(&self.api_url)
                .arg(format!("--rpcpassword={GATEWAY_PASSWORD}"))
                .arg("info");
            let output = run_command(&mut command);
            if output.is_ok() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return output.context("gatewayd did not become ready").map(drop);
            }
            sleep(POLL_INTERVAL).await;
        }
    }
}
