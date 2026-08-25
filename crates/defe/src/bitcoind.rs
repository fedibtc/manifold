use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use defe_api::{ApiError, ApiErrorKind, BitcoindInfo, ResourceDescriptor};

use crate::resource_manager::{
    ManagedResource, ResourceAllocation, ResourceDriver, ResourceKind, ResourceSlotId,
};
use crate::resource_process::{ResourceProcess, ResourceProcessConfig, log_tail};

const BITCOIND_HOST: &str = "127.0.0.1";
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(100);

/// Starts and supervises local Bitcoin Core regtest resource slots.
pub struct BitcoindDriver {
    bitcoind_bin: OsString,
    resource_root: PathBuf,
    log_dir: PathBuf,
    stable: Mutex<HashMap<ResourceSlotId, StableBitcoindAllocation>>,
}

impl BitcoindDriver {
    /// Create a bitcoind driver using the given binary and storage directories.
    #[must_use]
    pub fn new(
        bitcoind_bin: impl Into<OsString>,
        resource_root: impl Into<PathBuf>,
        log_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            bitcoind_bin: bitcoind_bin.into(),
            resource_root: resource_root.into(),
            log_dir: log_dir.into(),
            stable: Mutex::new(HashMap::new()),
        }
    }

    fn stable_allocation(
        &self,
        allocation: &ResourceAllocation,
    ) -> Result<StableBitcoindAllocation, ApiError> {
        let mut stable = self
            .stable
            .lock()
            .map_err(|_| internal_error("bitcoind allocation mutex poisoned"))?;
        if let Some(existing) = stable.get(&allocation.slot_id) {
            return Ok(existing.clone());
        }

        let ports = defe_portalloc::port_alloc(4).map_err(|err| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!("failed to allocate ports for bitcoind: {err}"),
            )
        })?;
        let p2p_port = ports;
        let rpc_port = ports.checked_add(1).ok_or_else(|| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                "allocated bitcoind port range overflowed u16",
            )
        })?;
        let zmq_pub_raw_block_port = ports.checked_add(2).ok_or_else(|| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                "allocated bitcoind port range overflowed u16",
            )
        })?;
        let zmq_pub_raw_tx_port = ports.checked_add(3).ok_or_else(|| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                "allocated bitcoind port range overflowed u16",
            )
        })?;
        let resource_id = format!("slot-{}", allocation.slot_id.0);
        let data_dir = self.resource_root.join("bitcoind").join(&resource_id);
        let rpc_username = "bitcoin".to_owned();
        let rpc_password = "bitcoin".to_owned();

        let created = StableBitcoindAllocation {
            p2p_port,
            rpc_port,
            zmq_pub_raw_block_port,
            zmq_pub_raw_tx_port,
            data_dir,
            rpc_username,
            rpc_password,
        };
        stable.insert(allocation.slot_id, created.clone());
        Ok(created)
    }

    fn start_bitcoind(
        &self,
        allocation: &ResourceAllocation,
    ) -> Result<Box<dyn ManagedResource>, ApiError> {
        if allocation.kind != ResourceKind::Bitcoind {
            return Err(ApiError::new(
                ApiErrorKind::ResourceKindUnavailable,
                format!("bitcoind driver cannot start {:?}", allocation.kind),
            ));
        }

        let stable = self.stable_allocation(allocation)?;
        stable.prepare_files()?;
        let log_path = bitcoind_log_path(&self.log_dir, allocation.slot_id, allocation.generation);

        let process = Arc::new(
            ResourceProcess::spawn(
                ResourceProcessConfig::new(
                    self.bitcoind_bin.clone(),
                    log_path.clone(),
                    log_path.clone(),
                )
                .arg(format!("-datadir={}", stable.data_dir.display())),
            )
            .map_err(|err| {
                ApiError::new(
                    ApiErrorKind::ResourceStartFailed,
                    format!(
                        "failed to spawn bitcoind {}: {err}",
                        self.bitcoind_bin.to_string_lossy()
                    ),
                )
            })?,
        );

        if let Err(err) = wait_until_ready(&process, stable.rpc_port, &log_path) {
            let _ = process.stop();
            return Err(err);
        }
        if let Err(err) = self.initialize_wallet_and_blocks(&stable, &log_path) {
            let _ = process.stop();
            return Err(err);
        }

        Ok(Box::new(BitcoindResource {
            descriptor: stable.descriptor(),
            process,
        }))
    }
}

impl ResourceDriver for BitcoindDriver {
    fn start(&self, allocation: &ResourceAllocation) -> Result<Box<dyn ManagedResource>, ApiError> {
        self.start_bitcoind(allocation)
    }
}

impl BitcoindDriver {
    fn initialize_wallet_and_blocks(
        &self,
        stable: &StableBitcoindAllocation,
        log_path: &Path,
    ) -> Result<(), ApiError> {
        self.poll_bitcoin_cli(stable, log_path, &["getblockchaininfo"])?;
        match self.run_bitcoin_cli(stable, &["createwallet", "default"]) {
            Ok(_) => {}
            Err(err) if err.message.contains("Database already exists") => {}
            Err(err) => return Err(err),
        }
        let address = self.run_bitcoin_cli(stable, &["-rpcwallet=default", "getnewaddress"])?;
        self.run_bitcoin_cli(
            stable,
            &[
                "-rpcwallet=default",
                "generatetoaddress",
                "101",
                address.trim(),
            ],
        )?;
        self.poll_bitcoin_cli(stable, log_path, &["getblockchaininfo"])?;
        Ok(())
    }

    fn run_bitcoin_cli(
        &self,
        stable: &StableBitcoindAllocation,
        args: &[&str],
    ) -> Result<String, ApiError> {
        let output = Command::new(self.bitcoin_cli_bin())
            .arg("-regtest")
            .arg(format!("-datadir={}", stable.data_dir.display()))
            .args(args)
            .output()
            .map_err(|err| {
                ApiError::new(
                    ApiErrorKind::ResourceStartFailed,
                    format!("failed to run bitcoin-cli: {err}"),
                )
            })?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(ApiError::new(
            ApiErrorKind::ResourceStartFailed,
            format!("bitcoin-cli {} failed: {stderr}", args.join(" ")),
        ))
    }

    fn poll_bitcoin_cli(
        &self,
        stable: &StableBitcoindAllocation,
        log_path: &Path,
        args: &[&str],
    ) -> Result<(), ApiError> {
        let deadline = std::time::Instant::now() + READY_TIMEOUT;
        loop {
            match self.run_bitcoin_cli(stable, args) {
                Ok(_) => return Ok(()),
                Err(err) if std::time::Instant::now() < deadline => {
                    let _ = err;
                    std::thread::sleep(READY_POLL_INTERVAL);
                }
                Err(err) => {
                    return Err(ApiError::new(
                        ApiErrorKind::ResourceStartFailed,
                        format!(
                            "bitcoind RPC did not become ready before timeout: {}; {}",
                            err.message,
                            log_tail(log_path)
                        ),
                    ));
                }
            }
        }
    }

    fn bitcoin_cli_bin(&self) -> OsString {
        let bitcoind = PathBuf::from(&self.bitcoind_bin);
        bitcoind
            .parent()
            .map(|parent| parent.join("bitcoin-cli").into_os_string())
            .unwrap_or_else(|| OsString::from("bitcoin-cli"))
    }
}

#[derive(Clone)]
struct StableBitcoindAllocation {
    p2p_port: u16,
    rpc_port: u16,
    zmq_pub_raw_block_port: u16,
    zmq_pub_raw_tx_port: u16,
    data_dir: PathBuf,
    rpc_username: String,
    rpc_password: String,
}

impl StableBitcoindAllocation {
    fn prepare_files(&self) -> Result<(), ApiError> {
        fs::create_dir_all(&self.data_dir)
            .map_err(|err| {
                ApiError::new(
                    ApiErrorKind::ResourceStartFailed,
                    format!(
                        "failed to create bitcoind data dir {}: {err}",
                        self.data_dir.display()
                    ),
                )
            })
            .and_then(|()| {
                fs::write(self.data_dir.join("bitcoin.conf"), self.bitcoin_conf()).map_err(|err| {
                    ApiError::new(
                        ApiErrorKind::ResourceStartFailed,
                        format!(
                            "failed to write bitcoind config in {}: {err}",
                            self.data_dir.display()
                        ),
                    )
                })
            })
    }

    fn descriptor(&self) -> BitcoindInfo {
        BitcoindInfo {
            rpc_url: format!("http://{BITCOIND_HOST}:{}", self.rpc_port),
            rpc_host: BITCOIND_HOST.to_owned(),
            rpc_port: self.rpc_port,
            p2p_port: self.p2p_port,
            rpc_username: self.rpc_username.clone(),
            rpc_password: self.rpc_password.clone(),
            data_dir: self.data_dir.clone(),
        }
    }

    fn bitcoin_conf(&self) -> String {
        format!(
            "\
regtest=1
fallbackfee=0.0004
txindex=1
server=1
rpcuser={rpc_user}
rpcpassword={rpc_password}
zmqpubrawblock=tcp://127.0.0.1:{zmq_pub_raw_block}
zmqpubrawtx=tcp://127.0.0.1:{zmq_pub_raw_tx}
rpcworkqueue=1024
rpcthreads=64
deprecatedrpc=warnings
dnsseed=0
[regtest]
port={p2p_port}
bind=127.0.0.1:{p2p_port}
rpcport={rpc_port}
rpcbind=127.0.0.1:{rpc_port}
rpcallowip=127.0.0.1
",
            rpc_user = self.rpc_username,
            rpc_password = self.rpc_password,
            zmq_pub_raw_block = self.zmq_pub_raw_block_port,
            zmq_pub_raw_tx = self.zmq_pub_raw_tx_port,
            p2p_port = self.p2p_port,
            rpc_port = self.rpc_port,
        )
    }
}

struct BitcoindResource {
    descriptor: BitcoindInfo,
    process: Arc<ResourceProcess>,
}

impl ManagedResource for BitcoindResource {
    fn descriptor(&self) -> ResourceDescriptor {
        ResourceDescriptor::Bitcoind(self.descriptor.clone())
    }

    fn is_running(&self) -> bool {
        self.process.is_running()
    }

    fn stop(&mut self) {
        let _ = self.process.stop();
    }
}

fn bitcoind_log_path(log_dir: &Path, slot_id: ResourceSlotId, generation: u64) -> PathBuf {
    log_dir.join(format!(
        "bitcoind-slot-{}-generation-{generation}.log",
        slot_id.0
    ))
}

fn wait_until_ready(
    process: &ResourceProcess,
    rpc_port: u16,
    log_path: &Path,
) -> Result<(), ApiError> {
    std::thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        scope.spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| {
                    ApiError::new(
                        ApiErrorKind::InternalServerError,
                        format!("failed to build bitcoind readiness runtime: {err}"),
                    )
                })
                .and_then(|runtime| {
                    runtime.block_on(wait_until_ready_async(process, rpc_port, log_path))
                });
            let _ = sender.send(result);
        });
        receiver.recv().unwrap_or_else(|err| {
            Err(ApiError::new(
                ApiErrorKind::InternalServerError,
                format!("bitcoind readiness worker did not report a result: {err}"),
            ))
        })
    })
}

async fn wait_until_ready_async(
    process: &ResourceProcess,
    rpc_port: u16,
    log_path: &Path,
) -> Result<(), ApiError> {
    let addr = SocketAddr::from(([127, 0, 0, 1], rpc_port));
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    let mut interval = tokio::time::interval(READY_POLL_INTERVAL);

    loop {
        if tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr))
            .await
            .is_ok_and(|result| result.is_ok())
        {
            return Ok(());
        }

        if !process.is_running() {
            return Err(ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "bitcoind exited before becoming ready; {}",
                    log_tail(log_path)
                ),
            ));
        }

        if deadline <= tokio::time::Instant::now() {
            return Err(ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "bitcoind did not become ready on {BITCOIND_HOST}:{rpc_port} before timeout; {}",
                    log_tail(log_path)
                ),
            ));
        }

        interval.tick().await;
    }
}

fn internal_error(message: impl Into<String>) -> ApiError {
    ApiError::new(ApiErrorKind::InternalServerError, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stable_allocation() -> StableBitcoindAllocation {
        StableBitcoindAllocation {
            p2p_port: 18_444,
            rpc_port: 18_443,
            zmq_pub_raw_block_port: 28_332,
            zmq_pub_raw_tx_port: 28_333,
            data_dir: PathBuf::from("/tmp/defe-bitcoind-test"),
            rpc_username: "bitcoin".to_owned(),
            rpc_password: "bitcoin".to_owned(),
        }
    }

    #[test]
    fn bitcoind_logs_are_generation_specific() {
        assert_eq!(
            bitcoind_log_path(Path::new("/tmp/logs"), ResourceSlotId(7), 3),
            PathBuf::from("/tmp/logs/bitcoind-slot-7-generation-3.log")
        );
    }

    #[test]
    fn bitcoind_enables_full_transaction_index() {
        assert!(stable_allocation().bitcoin_conf().contains("txindex=1\n"));
    }
}
