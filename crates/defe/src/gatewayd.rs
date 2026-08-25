use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use defe_api::{ApiError, ApiErrorKind, GatewaydInfo, ResourceDescriptor};

use crate::resource_manager::{
    ManagedResource, ResourceAllocation, ResourceDriver, ResourceKind, ResourceSlotId,
};
use crate::resource_process::{ResourceProcess, ResourceProcessConfig, log_tail};

const PASSWORD: &str = "testpassword";
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Starts and supervises local Fedimint gateway daemon slots.
pub struct GatewaydDriver {
    gatewayd_bin: OsString,
    gateway_cli_bin: OsString,
    resource_root: PathBuf,
    log_dir: PathBuf,
    stable: Mutex<HashMap<ResourceSlotId, StableGatewaydAllocation>>,
}

impl GatewaydDriver {
    #[must_use]
    pub fn new(
        gatewayd_bin: impl Into<OsString>,
        gateway_cli_bin: impl Into<OsString>,
        resource_root: impl Into<PathBuf>,
        log_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            gatewayd_bin: gatewayd_bin.into(),
            gateway_cli_bin: gateway_cli_bin.into(),
            resource_root: resource_root.into(),
            log_dir: log_dir.into(),
            stable: Mutex::new(HashMap::new()),
        }
    }

    fn stable_allocation(
        &self,
        allocation: &ResourceAllocation,
        request: &defe_api::GatewaydRequest,
    ) -> Result<StableGatewaydAllocation, ApiError> {
        let mut stable = self
            .stable
            .lock()
            .map_err(|_| internal_error("gatewayd allocation mutex poisoned"))?;
        if let Some(existing) = stable.get(&allocation.slot_id) {
            return Ok(existing.clone());
        }
        let ports = defe_portalloc::port_alloc(3).map_err(|error| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!("allocate gatewayd ports: {error}"),
            )
        })?;
        let metrics_port = checked_port(ports, 1)?;
        let ldk_port = checked_port(ports, 2)?;
        let created = StableGatewaydAllocation {
            data_dir: self
                .resource_root
                .join("gatewayd")
                .join(format!("slot-{}", allocation.slot_id.0)),
            api_port: ports,
            metrics_port,
            ldk_port,
            bitcoind: request.bitcoind.clone(),
            iroh_connect_overrides: request.iroh_connect_overrides.clone(),
        };
        stable.insert(allocation.slot_id, created.clone());
        Ok(created)
    }

    fn start_gatewayd(
        &self,
        allocation: &ResourceAllocation,
        request: &defe_api::GatewaydRequest,
    ) -> Result<Box<dyn ManagedResource>, ApiError> {
        let stable = self.stable_allocation(allocation, request)?;
        fs::create_dir_all(&stable.data_dir).map_err(|error| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "create gatewayd data directory {}: {error}",
                    stable.data_dir.display()
                ),
            )
        })?;
        let api_url = format!("http://127.0.0.1:{}", stable.api_port);
        let log_path = self.log_dir.join(format!(
            "gatewayd-slot-{}-generation-{}.log",
            allocation.slot_id.0, allocation.generation
        ));
        let mut config = ResourceProcessConfig::new(self.gatewayd_bin.clone(), log_path.clone(), log_path)
            .args([
                "--data-dir".into(),
                stable.data_dir.clone().into_os_string(),
                "--listen".into(),
                format!("127.0.0.1:{}", stable.api_port).into(),
                "--api-addr".into(),
                api_url.clone().into(),
                "--network".into(),
                "regtest".into(),
                "--num-route-hints".into(),
                "0".into(),
                "ldk".into(),
                "--ldk-lightning-port".into(),
                stable.ldk_port.to_string().into(),
                "--ldk-alias".into(),
                format!("defe-gatewayd-{}", allocation.slot_id.0).into(),
            ])
            .env("RUST_LOG", "info")
            .env("FM_GATEWAY_LIGHTNING_MODULE_MODE", "LNv1")
            .env("FM_GATEWAY_METRICS_LISTEN_ADDR", format!("127.0.0.1:{}", stable.metrics_port))
            .env("FM_GATEWAY_BCRYPT_PASSWORD_HASH", "$2b$12$Etlumnzi/VJ0Ky0Dqoe55eCbvDXItj94thfhvu2o423ox7os.6XfC")
            .env("FM_GATEWAY_SKIP_SETUP", "true")
            .env("FM_BITCOIND_URL", &stable.bitcoind.rpc_url)
            .env("FM_BITCOIND_USERNAME", &stable.bitcoind.rpc_username)
            .env("FM_BITCOIND_PASSWORD", &stable.bitcoind.rpc_password)
            .env("FM_DEFAULT_ROUTING_FEES", "2000,5000")
            .env("FM_DEFAULT_TRANSACTION_FEES", "2000,5000")
            .env("FM_PORT_LDK", stable.ldk_port.to_string())
            .env("FM_LDK_ALIAS", format!("defe-gatewayd-{}", allocation.slot_id.0))
            .env("FM_GATEWAY_MNEMONIC", "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about")
            .env("FM_IN_DEVIMINT", "1");
        if let Some(overrides) = &stable.iroh_connect_overrides {
            config = config.env("FM_IROH_CONNECT_OVERRIDES", overrides);
        }
        let process = Arc::new(ResourceProcess::spawn(config).map_err(|error| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "failed to spawn gatewayd {}: {error}",
                    self.gatewayd_bin.to_string_lossy()
                ),
            )
        })?);
        if let Err(error) = self.wait_until_ready(&process, &api_url) {
            let _ = process.stop();
            return Err(error);
        }
        Ok(Box::new(GatewaydResource {
            descriptor: GatewaydInfo {
                api_url,
                password: PASSWORD.to_owned(),
            },
            process,
        }))
    }

    fn wait_until_ready(&self, process: &ResourceProcess, api_url: &str) -> Result<(), ApiError> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if !process.is_running() {
                return Err(ApiError::new(
                    ApiErrorKind::ResourceStartFailed,
                    format!(
                        "gatewayd exited before becoming ready; {}",
                        log_tail(process.stdout_log())
                    ),
                ));
            }
            if Command::new(&self.gateway_cli_bin)
                .args(["-a", api_url, &format!("--rpcpassword={PASSWORD}"), "info"])
                .output()
                .is_ok_and(|output| output.status.success())
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(ApiError::new(
                    ApiErrorKind::ResourceStartFailed,
                    format!(
                        "gatewayd did not become ready before timeout; {}",
                        log_tail(process.stdout_log())
                    ),
                ));
            }
            std::thread::sleep(READY_POLL_INTERVAL);
        }
    }
}

impl ResourceDriver for GatewaydDriver {
    fn start(&self, allocation: &ResourceAllocation) -> Result<Box<dyn ManagedResource>, ApiError> {
        let ResourceKind::Gatewayd(request) = &allocation.kind else {
            return Err(ApiError::new(
                ApiErrorKind::ResourceKindUnavailable,
                format!("gatewayd driver cannot start {:?}", allocation.kind),
            ));
        };
        self.start_gatewayd(allocation, request)
    }
}

#[derive(Clone)]
struct StableGatewaydAllocation {
    data_dir: PathBuf,
    api_port: u16,
    metrics_port: u16,
    ldk_port: u16,
    bitcoind: defe_api::BitcoindInfo,
    iroh_connect_overrides: Option<String>,
}

struct GatewaydResource {
    descriptor: GatewaydInfo,
    process: Arc<ResourceProcess>,
}

impl ManagedResource for GatewaydResource {
    fn descriptor(&self) -> ResourceDescriptor {
        ResourceDescriptor::Gatewayd(self.descriptor.clone())
    }
    fn is_running(&self) -> bool {
        self.process.is_running()
    }
    fn stop(&mut self) {
        let _ = self.process.stop();
    }
}

fn checked_port(base: u16, offset: u16) -> Result<u16, ApiError> {
    base.checked_add(offset).ok_or_else(|| {
        ApiError::new(
            ApiErrorKind::ResourceStartFailed,
            "allocated gatewayd port range overflowed",
        )
    })
}

fn internal_error(message: impl Into<String>) -> ApiError {
    ApiError::new(ApiErrorKind::InternalServerError, message)
}
