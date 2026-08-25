use std::collections::HashMap;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use defe_api::{ApiError, ApiErrorKind, PushGatewayInfo, ResourceDescriptor};

use crate::resource_manager::{
    ManagedResource, ResourceAllocation, ResourceDriver, ResourceKind, ResourceSlotId,
};
use crate::resource_process::{ResourceProcess, ResourceProcessConfig, log_tail};

const PUSH_GATEWAY_HOST: &str = "127.0.0.1";
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(100);

/// Starts and supervises local push gateway resource slots.
pub struct PushGatewayDriver {
    /// Push gateway binary to spawn for each resource process.
    gateway_bin: OsString,
    /// Root directory for per-slot resource data.
    resource_root: PathBuf,
    /// Directory for child stdout/stderr log files.
    log_dir: PathBuf,
    /// Stable per-slot allocation data reused across restarts.
    stable: Mutex<HashMap<ResourceSlotId, StablePushGatewayAllocation>>,
}

impl PushGatewayDriver {
    /// Create a push gateway driver using the given binary and storage directories.
    #[must_use]
    pub fn new(
        gateway_bin: impl Into<OsString>,
        resource_root: impl Into<PathBuf>,
        log_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            gateway_bin: gateway_bin.into(),
            resource_root: resource_root.into(),
            log_dir: log_dir.into(),
            stable: Mutex::new(HashMap::new()),
        }
    }

    fn stable_allocation(
        &self,
        allocation: &ResourceAllocation,
    ) -> Result<StablePushGatewayAllocation, ApiError> {
        let mut stable = self
            .stable
            .lock()
            .map_err(|_| internal_error("push gateway allocation mutex poisoned"))?;
        if let Some(existing) = stable.get(&allocation.slot_id) {
            return Ok(existing.clone());
        }

        let port = defe_portalloc::port_alloc(1).map_err(|err| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!("failed to allocate port for push gateway: {err}"),
            )
        })?;
        let resource_id = format!("slot-{}", allocation.slot_id.0);
        let slot_dir = self.resource_root.join("push-gateway").join(&resource_id);
        let database_path = slot_dir.join("push-gateway.sqlite");
        let app_id = format!("defe-push-gateway-slot-{}", allocation.slot_id.0);

        let created = StablePushGatewayAllocation {
            port,
            slot_dir,
            database_path,
            app_id,
        };
        stable.insert(allocation.slot_id, created.clone());
        Ok(created)
    }

    fn start_push_gateway(
        &self,
        allocation: &ResourceAllocation,
    ) -> Result<Box<dyn ManagedResource>, ApiError> {
        if allocation.kind != ResourceKind::PushGateway {
            return Err(ApiError::new(
                ApiErrorKind::ResourceKindUnavailable,
                format!("push gateway driver cannot start {:?}", allocation.kind),
            ));
        }

        let stable = self.stable_allocation(allocation)?;
        stable.prepare_files()?;

        let log_path =
            push_gateway_log_path(&self.log_dir, allocation.slot_id, allocation.generation);

        let process = Arc::new(
            ResourceProcess::spawn(push_gateway_process_config(
                self.gateway_bin.clone(),
                &log_path,
                &stable,
            ))
            .map_err(|err| {
                ApiError::new(
                    ApiErrorKind::ResourceStartFailed,
                    format!(
                        "failed to spawn push gateway {}: {err}",
                        self.gateway_bin.to_string_lossy()
                    ),
                )
            })?,
        );

        if let Err(err) = wait_until_ready(&process, stable.port, &log_path) {
            let _ = process.stop();
            return Err(err);
        }

        let descriptor = PushGatewayInfo {
            url: format!("http://{PUSH_GATEWAY_HOST}:{}", stable.port),
            host: PUSH_GATEWAY_HOST.to_owned(),
            port: stable.port,
            app_id: stable.app_id,
            database_path: stable.database_path,
        };

        Ok(Box::new(PushGatewayResource {
            descriptor,
            process,
        }))
    }
}

fn push_gateway_process_config(
    gateway_bin: OsString,
    log_path: &Path,
    stable: &StablePushGatewayAllocation,
) -> ResourceProcessConfig {
    let bind = format!("{PUSH_GATEWAY_HOST}:{}", stable.port);
    let public_base_url = format!("http://{PUSH_GATEWAY_HOST}:{}", stable.port);
    let database_url = format!("sqlite://{}?mode=rwc", stable.database_path.display());
    ResourceProcessConfig::new(gateway_bin, log_path.to_owned(), log_path.to_owned())
        .env("PUSH_GATEWAY_BIND", bind)
        .env("PUSH_GATEWAY_APP_ID", stable.app_id.clone())
        .env("PUSH_GATEWAY_PUBLIC_BASE_URL", public_base_url)
        .env("PUSH_GATEWAY_ALLOW_INSECURE_PUBLIC_BASE_URL", "true")
        .env("PUSH_GATEWAY_DATABASE_URL", database_url)
}

impl ResourceDriver for PushGatewayDriver {
    fn start(&self, allocation: &ResourceAllocation) -> Result<Box<dyn ManagedResource>, ApiError> {
        self.start_push_gateway(allocation)
    }
}

#[derive(Clone)]
struct StablePushGatewayAllocation {
    /// Stable TCP port reserved for this resource slot.
    port: u16,
    /// Per-slot directory under the defe resources root.
    slot_dir: PathBuf,
    /// SQLite database path reused across restarts of this slot.
    database_path: PathBuf,
    /// App id accepted by this gateway instance.
    app_id: String,
}

impl StablePushGatewayAllocation {
    fn prepare_files(&self) -> Result<(), ApiError> {
        std::fs::create_dir_all(&self.slot_dir).map_err(|err| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "failed to create push gateway data dir {}: {err}",
                    self.slot_dir.display()
                ),
            )
        })
    }
}

fn internal_error(message: impl Into<String>) -> ApiError {
    ApiError::new(ApiErrorKind::InternalServerError, message)
}

struct PushGatewayResource {
    /// Descriptor returned to clients for this resource lease.
    descriptor: PushGatewayInfo,
    /// Supervised child process running the gateway.
    process: Arc<ResourceProcess>,
}

impl ManagedResource for PushGatewayResource {
    fn descriptor(&self) -> ResourceDescriptor {
        ResourceDescriptor::PushGateway(self.descriptor.clone())
    }

    fn is_running(&self) -> bool {
        self.process.is_running()
    }

    fn stop(&mut self) {
        let _ = self.process.stop();
    }
}

fn push_gateway_log_path(log_dir: &Path, slot_id: ResourceSlotId, generation: u64) -> PathBuf {
    log_dir.join(format!(
        "push-gateway-slot-{}-generation-{generation}.log",
        slot_id.0
    ))
}

fn wait_until_ready(process: &ResourceProcess, port: u16, log_path: &Path) -> Result<(), ApiError> {
    std::thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        scope.spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| {
                    ApiError::new(
                        ApiErrorKind::InternalServerError,
                        format!("failed to build push gateway readiness runtime: {err}"),
                    )
                })
                .and_then(|runtime| {
                    runtime.block_on(wait_until_ready_async(process, port, log_path))
                });
            let _ = sender.send(result);
        });
        receiver.recv().unwrap_or_else(|err| {
            Err(ApiError::new(
                ApiErrorKind::InternalServerError,
                format!("push gateway readiness worker did not report a result: {err}"),
            ))
        })
    })
}

async fn wait_until_ready_async(
    process: &ResourceProcess,
    port: u16,
    log_path: &Path,
) -> Result<(), ApiError> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
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
                    "push gateway exited before becoming ready; {}",
                    log_tail(log_path)
                ),
            ));
        }

        if deadline <= tokio::time::Instant::now() {
            return Err(ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "push gateway did not become ready on {PUSH_GATEWAY_HOST}:{port} before timeout; {}",
                    log_tail(log_path)
                ),
            ));
        }

        interval.tick().await;
    }
}

#[cfg(test)]
mod tests;
