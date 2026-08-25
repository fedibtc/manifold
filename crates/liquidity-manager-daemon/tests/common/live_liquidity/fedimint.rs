#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use tokio::time::Duration;

use super::bitcoin::BitcoinFixture;
use super::{
    ManagedProcess, locate_binary, process_log_path, run_command, use_devimint_if_under_defe,
};

pub struct FedimintFixture {
    pub api_port: u16,
    pub invite_code: String,
    fedimint_cli: PathBuf,
    process: ManagedProcess,
    data_dir: PathBuf,
}

impl FedimintFixture {
    /// Target federation without the stability-pool module (gateway/LN tests).
    ///
    /// `label` names this federation's subdirectory under `data_root`. It is a
    /// parameter rather than a constant because a test may stand up more than
    /// one target federation, and two of them sharing a data directory is a
    /// corrupt federation rather than a failed one.
    pub async fn start(
        label: &str,
        bitcoin: &BitcoinFixture,
        data_root: &Path,
    ) -> anyhow::Result<Self> {
        Self::start_inner(label, bitcoin, data_root, false).await
    }

    /// Target federation running the stability-pool v2 module. Uses the
    /// SP-enabled `fedimintd` (`FLIP_E2E_SP_FEDIMINTD_BIN`) and enables the
    /// module with test parameters (mock oracle, short cycle), so the live
    /// stability-pool allocation path can be exercised end to end.
    pub async fn start_with_stability_pool(
        label: &str,
        bitcoin: &BitcoinFixture,
        data_root: &Path,
    ) -> anyhow::Result<Self> {
        Self::start_inner(label, bitcoin, data_root, true).await
    }

    async fn start_inner(
        label: &str,
        bitcoin: &BitcoinFixture,
        data_root: &Path,
        stability_pool: bool,
    ) -> anyhow::Result<Self> {
        let api_port = defe_portalloc::port_alloc(4).context("allocate target Fedimint ports")?;
        let p2p_port = api_port
            .checked_add(1)
            .context("target Fedimint P2P port overflow")?;
        let ui_port = api_port
            .checked_add(2)
            .context("target Fedimint UI port overflow")?;
        let metrics_port = api_port
            .checked_add(3)
            .context("target Fedimint metrics port overflow")?;
        let api_url = format!("ws://127.0.0.1:{api_port}/");
        let p2p_url = format!("fedimint://127.0.0.1:{p2p_port}");
        let data_dir = data_root.join(label);
        fs::create_dir_all(&data_dir)
            .with_context(|| format!("create Fedimint data dir {}", data_dir.display()))?;

        let fedimintd = if stability_pool {
            locate_binary("FLIP_E2E_SP_FEDIMINTD_BIN", "fedimintd")?
        } else {
            locate_binary("FLIP_E2E_FEDIMINTD_BIN", "fedimintd")?
        };
        let fedimint_cli = locate_binary("FLIP_E2E_FEDIMINT_CLI_BIN", "fedimint-cli")?;
        let mut command = Command::new(fedimintd);
        command
            .arg("--api-url")
            .arg(&api_url)
            .arg("--p2p-url")
            .arg(&p2p_url)
            .env("RUST_LOG", "info")
            .env("FM_REL_NOTES_ACK", "0_4_xyz")
            .env("FM_DEFAULT_BITCOIN_RPC_KIND", "bitcoind")
            .env(
                "FM_DEFAULT_BITCOIN_RPC_URL",
                bitcoin.host_rpc_url_with_auth(),
            )
            .env("FM_BITCOIND_URL", bitcoin.host_rpc_url())
            .env("FM_BITCOIND_USERNAME", bitcoin.rpc_username())
            .env("FM_BITCOIND_PASSWORD", bitcoin.rpc_password())
            .env("FM_BITCOIN_NETWORK", "regtest")
            .env("FM_BIND_P2P", format!("127.0.0.1:{p2p_port}"))
            .env("FM_BIND_API", format!("127.0.0.1:{api_port}"))
            .env("FM_BIND_UI", format!("127.0.0.1:{ui_port}"))
            .env("FM_BIND_METRICS", format!("127.0.0.1:{metrics_port}"))
            .env("FM_DATA_DIR", &data_dir)
            // Aggressive test timing for the federation's wallet block sync /
            // peg-in path, matching the client side. Aqueduct's harness relies
            // on this for regtest peg-ins to finalize promptly.
            .env("FM_IN_DEVIMINT", "1");
        if stability_pool {
            // Bundle the stability-pool v2 server module into DKG with test
            // parameters (mock oracle, short cycle, 1:1 collateral), so the
            // federation needs no external price oracle.
            command
                .env("FEDI_STABILITY_POOL_V2_MODULE_ENABLE", "1")
                .env("FEDI_STABILITY_POOL_MODULE_TEST_PARAMS", "1");
        }
        use_devimint_if_under_defe(&mut command);
        // Labelled, like the data directory: a second federation writing into
        // the first one's log would let `wait_for_log` below match a line the
        // *other* federation printed, and the fixture would return before its
        // own consensus had started.
        let process = ManagedProcess::spawn(
            format!("{label} fedimintd"),
            &mut command,
            process_log_path(data_root, label),
        )?;
        let mut this = Self {
            api_port,
            invite_code: String::new(),
            fedimint_cli,
            process,
            data_dir,
        };
        this.process
            .wait_for_log("Setup UI running at", Duration::from_secs(300))
            .await?;
        this.set_local_params("target-federation", "guardian1", "targetpassword")?;
        this.start_dkg("targetpassword")?;
        this.process
            .wait_for_log("Starting Consensus Engine", Duration::from_secs(300))
            .await?;
        this.invite_code = fs::read_to_string(this.data_dir.join("invite-code"))
            .context("read target Fedimint invite code")?
            .trim()
            .to_owned();
        Ok(this)
    }

    /// Stops the federation, leaving its invite code and its trust material
    /// perfectly valid and its API unreachable.
    ///
    /// A target federation that has gone away is a state FLIP has to keep
    /// working in. Admission never touches the federation, so a request naming
    /// it is still accepted; only the funding that follows cannot happen.
    pub fn stop(&mut self) -> anyhow::Result<()> {
        self.process.stop()
    }

    fn set_local_params(
        &self,
        federation_name: &str,
        guardian_name: &str,
        admin_password: &str,
    ) -> anyhow::Result<()> {
        self.run_cli(&[
            "--password",
            admin_password,
            "admin",
            "setup",
            &format!("ws://127.0.0.1:{}", self.api_port),
            "set-local-params",
            "--federation-name",
            federation_name,
            "--federation-size",
            "1",
            guardian_name,
        ])
        .context("set target Fedimint local params")?;
        Ok(())
    }

    fn start_dkg(&self, admin_password: &str) -> anyhow::Result<()> {
        self.run_cli(&[
            "--password",
            admin_password,
            "admin",
            "setup",
            &format!("ws://127.0.0.1:{}", self.api_port),
            "start-dkg",
        ])
        .context("start target Fedimint DKG")?;
        Ok(())
    }

    fn run_cli(&self, args: &[&str]) -> anyhow::Result<String> {
        let mut command = Command::new(&self.fedimint_cli);
        command.args(args);
        run_command(&mut command)
    }
}
