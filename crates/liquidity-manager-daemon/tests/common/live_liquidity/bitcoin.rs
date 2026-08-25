#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

use anyhow::Context;
use defe_client::BitcoindInfo;
use tokio::time::{Duration, sleep};

use super::{POLL_INTERVAL, locate_binary, run_command};

pub struct BitcoinFixture {
    info: BitcoindInfo,
    bitcoin_cli: PathBuf,
}

impl BitcoinFixture {
    pub fn new(info: BitcoindInfo) -> anyhow::Result<Self> {
        Ok(Self {
            info,
            bitcoin_cli: locate_binary("FLIP_E2E_BITCOIN_CLI_BIN", "bitcoin-cli")?,
        })
    }

    pub fn host_rpc_url(&self) -> &str {
        &self.info.rpc_url
    }

    pub fn host_rpc_url_with_auth(&self) -> String {
        format!(
            "http://{}:{}@{}:{}",
            self.info.rpc_username, self.info.rpc_password, self.info.rpc_host, self.info.rpc_port
        )
    }

    pub fn rpc_username(&self) -> &str {
        &self.info.rpc_username
    }

    pub fn rpc_password(&self) -> &str {
        &self.info.rpc_password
    }

    /// Full leased bitcoind descriptor, for fixtures (e.g. esplora) that need
    /// the data dir and RPC host/port directly.
    pub fn info(&self) -> &BitcoindInfo {
        &self.info
    }

    pub async fn mine_blocks(&self, count: u32) -> anyhow::Result<()> {
        let address = self.new_address()?;
        self.run(&["generatetoaddress", &count.to_string(), &address])?;
        Ok(())
    }

    pub async fn send_to_address(&self, address: &str, amount_btc: f64) -> anyhow::Result<String> {
        self.run(&["sendtoaddress", address, &amount_btc.to_string()])
            .context("send regtest Bitcoin to address")
    }

    pub fn new_address(&self) -> anyhow::Result<String> {
        self.run(&["getnewaddress"])
            .context("get defe Bitcoin wallet address")
    }

    pub async fn get_block_height(&self) -> anyhow::Result<u64> {
        self.run(&["getblockcount"])
            .context("query defe Bitcoin Core block height")?
            .parse::<u64>()
            .context("parse Bitcoin Core block height")
    }

    pub async fn wait_for_block_height(&self, expected: u64) -> anyhow::Result<()> {
        let deadline = std::time::Instant::now() + Duration::from_secs(90);
        loop {
            match self.get_block_height().await {
                Ok(height) if height >= expected => return Ok(()),
                Ok(_) | Err(_) if std::time::Instant::now() < deadline => {
                    sleep(POLL_INTERVAL).await;
                }
                Ok(height) => {
                    anyhow::bail!("Bitcoin Core reached height {height}, expected {expected}");
                }
                Err(error) => return Err(error).context("Bitcoin Core did not expose tip height"),
            }
        }
    }

    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        let mut command = Command::new(&self.bitcoin_cli);
        command
            .arg("-regtest")
            .arg(format!("-rpcconnect={}", self.info.rpc_host))
            .arg(format!("-rpcport={}", self.info.rpc_port))
            .arg(format!("-rpcuser={}", self.info.rpc_username))
            .arg(format!("-rpcpassword={}", self.info.rpc_password))
            .arg("-rpcwallet=default")
            .args(args);
        run_command(&mut command)
    }
}
