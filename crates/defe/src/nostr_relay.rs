use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use defe_api::{ApiError, ApiErrorKind, NostrRelayInfo, ResourceDescriptor};

use crate::resource_manager::{
    ManagedResource, ResourceAllocation, ResourceDriver, ResourceKind, ResourceSlotId,
};
use crate::resource_process::{ResourceProcess, ResourceProcessConfig, log_tail};

const NOSTR_RELAY_HOST: &str = "127.0.0.1";
/// `nostr-rs-relay` binds its listener only after it builds three SQLite
/// connection pools and migrates the database to schema v18, so readiness is
/// disk-bound in the same way the push gateway's is. This matches that
/// resource's budget rather than the ten seconds it used to allow, which was
/// the shortest budget of any resource here. A relay that fails for the usual
/// reason -- its port is taken -- panics and exits in milliseconds and is
/// caught by the liveness arm below, so the longer budget does not slow the
/// common failure down.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(100);

/// Starts and supervises local `nostr-rs-relay` resource slots.
pub struct NostrRelayDriver {
    /// Executable path or program name for `nostr-rs-relay`.
    relay_bin: OsString,
    /// Root directory used for relay data directories and generated configs.
    resource_root: PathBuf,
    /// Directory where relay process logs are written.
    log_dir: PathBuf,
    /// Stable per-slot allocation state reused across resource restarts.
    stable: Mutex<HashMap<ResourceSlotId, StableNostrRelayAllocation>>,
    /// Test-only access to launched processes so restart behavior can be exercised.
    #[cfg(test)]
    processes: Mutex<HashMap<ResourceSlotId, Arc<ResourceProcess>>>,
}

impl NostrRelayDriver {
    /// Create a Nostr relay driver using the given binary and storage directories.
    #[must_use]
    pub fn new(
        relay_bin: impl Into<OsString>,
        resource_root: impl Into<PathBuf>,
        log_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            relay_bin: relay_bin.into(),
            resource_root: resource_root.into(),
            log_dir: log_dir.into(),
            stable: Mutex::new(HashMap::new()),
            #[cfg(test)]
            processes: Mutex::new(HashMap::new()),
        }
    }

    fn stable_allocation(
        &self,
        allocation: &ResourceAllocation,
    ) -> Result<StableNostrRelayAllocation, ApiError> {
        let mut stable = self
            .stable
            .lock()
            .map_err(|_| internal_error("nostr relay allocation mutex poisoned"))?;
        if let Some(existing) = stable.get(&allocation.slot_id) {
            return Ok(existing.clone());
        }

        let resource_id = format!("slot-{}", allocation.slot_id.0);
        let slot_dir = self.resource_root.join("nostr-relay").join(&resource_id);
        let data_dir = slot_dir.join("db");
        let config_path = slot_dir.join("config.toml");
        let port = defe_portalloc::port_alloc(1).map_err(|err| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!("failed to allocate port for nostr relay: {err}"),
            )
        })?;

        let created = StableNostrRelayAllocation {
            port,
            data_dir,
            config_path,
        };
        stable.insert(allocation.slot_id, created.clone());
        Ok(created)
    }

    fn start_nostr_relay(
        &self,
        allocation: &ResourceAllocation,
    ) -> Result<Box<dyn ManagedResource>, ApiError> {
        if allocation.kind != ResourceKind::NostrRelay {
            return Err(ApiError::new(
                ApiErrorKind::ResourceKindUnavailable,
                format!("nostr relay driver cannot start {:?}", allocation.kind),
            ));
        }

        let stable = self.stable_allocation(allocation)?;
        let log_path =
            nostr_relay_log_path(&self.log_dir, allocation.slot_id, allocation.generation);
        stable.prepare_files(&log_path)?;

        let process = Arc::new(
            ResourceProcess::spawn(
                ResourceProcessConfig::new(
                    self.relay_bin.clone(),
                    log_path.clone(),
                    log_path.clone(),
                )
                .arg("--config")
                .arg(stable.config_path.clone())
                .env("RUST_LOG", relay_log_filter(std::env::var_os("RUST_LOG"))),
            )
            .map_err(|err| {
                ApiError::new(
                    ApiErrorKind::ResourceStartFailed,
                    format!(
                        "failed to spawn nostr relay {}: {err}",
                        self.relay_bin.to_string_lossy()
                    ),
                )
            })?,
        );

        if let Err(err) = wait_until_ready(&process, stable.port, &log_path) {
            let _ = process.stop();
            return Err(err);
        }

        #[cfg(test)]
        self.processes
            .lock()
            .map_err(|_| internal_error("nostr relay process mutex poisoned"))?
            .insert(allocation.slot_id, Arc::clone(&process));

        Ok(Box::new(NostrRelayResource {
            process,
            descriptor: stable.descriptor(),
        }))
    }

    #[cfg(test)]
    fn stop_only_running_process_for_test(&self) {
        let running_processes = {
            let processes = self.processes.lock().expect("nostr relay process mutex");
            processes
                .values()
                .filter(|process| process.is_running())
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            running_processes.len(),
            1,
            "test expected exactly one running relay process"
        );
        let _ = running_processes[0].stop();
    }
}

impl ResourceDriver for NostrRelayDriver {
    /// Start a `nostr-rs-relay` process for the requested allocation.
    fn start(&self, allocation: &ResourceAllocation) -> Result<Box<dyn ManagedResource>, ApiError> {
        self.start_nostr_relay(allocation)
    }
}

#[derive(Debug, Clone)]
struct StableNostrRelayAllocation {
    /// TCP port reserved for the lifetime of this relay slot.
    port: u16,
    /// Directory containing the relay database for this slot.
    data_dir: PathBuf,
    /// Path to the generated `nostr-rs-relay` TOML configuration.
    config_path: PathBuf,
}

impl StableNostrRelayAllocation {
    fn prepare_files(&self, log_path: &Path) -> Result<(), ApiError> {
        validate_utf8_path(&self.data_dir, "nostr relay data directory")?;
        validate_utf8_path(&self.config_path, "nostr relay config path")?;
        validate_utf8_path(log_path, "nostr relay log path")?;
        fs::create_dir_all(&self.data_dir).map_err(|err| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "failed to create nostr relay data directory {}: {err}",
                    self.data_dir.display()
                ),
            )
        })?;
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                ApiError::new(
                    ApiErrorKind::ResourceStartFailed,
                    format!(
                        "failed to create nostr relay config directory {}: {err}",
                        parent.display()
                    ),
                )
            })?;
        }
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                ApiError::new(
                    ApiErrorKind::ResourceStartFailed,
                    format!(
                        "failed to create nostr relay log directory {}: {err}",
                        parent.display()
                    ),
                )
            })?;
        }

        fs::write(&self.config_path, self.config_contents()).map_err(|err| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "failed to write nostr relay config {}: {err}",
                    self.config_path.display()
                ),
            )
        })
    }

    fn config_contents(&self) -> String {
        format!(
            "[database]\ndata_directory = \"{}\"\n\n[network]\naddress = \"{NOSTR_RELAY_HOST}\"\nport = {}\n",
            toml_basic_string_path(&self.data_dir),
            self.port
        )
    }

    fn descriptor(&self) -> ResourceDescriptor {
        ResourceDescriptor::NostrRelay(NostrRelayInfo {
            url: format!("ws://{NOSTR_RELAY_HOST}:{}", self.port),
            host: NOSTR_RELAY_HOST.to_owned(),
            port: self.port,
            data_dir: self.data_dir.clone(),
        })
    }
}

struct NostrRelayResource {
    /// Supervised relay process for this resource generation.
    process: Arc<ResourceProcess>,
    /// Client-visible descriptor returned for this resource generation.
    descriptor: ResourceDescriptor,
}

impl ManagedResource for NostrRelayResource {
    fn descriptor(&self) -> ResourceDescriptor {
        self.descriptor.clone()
    }

    fn is_running(&self) -> bool {
        self.process.is_running()
    }

    fn stop(&mut self) {
        let _ = self.process.stop();
    }
}

fn nostr_relay_log_path(log_dir: &Path, slot_id: ResourceSlotId, generation: u64) -> PathBuf {
    log_dir.join(format!(
        "nostr-relay-slot-{}-generation-{generation}.log",
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
                        format!("failed to build relay readiness runtime: {err}"),
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
                format!("relay readiness worker did not report a result: {err}"),
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
                    "nostr relay exited before becoming ready; {}",
                    log_tail(log_path)
                ),
            ));
        }

        if deadline <= tokio::time::Instant::now() {
            return Err(ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "nostr relay did not become ready on {NOSTR_RELAY_HOST}:{port} before timeout; {}",
                    log_tail(log_path)
                ),
            ));
        }

        interval.tick().await;
    }
}

/// Log filter handed to `nostr-rs-relay`.
///
/// The relay initialises `tracing` from `RUST_LOG` and prints nothing at all
/// when it is unset, so without this its log file is empty on every run and a
/// readiness failure quotes nothing. An inherited value wins, so a developer
/// can still raise the level for one run.
fn relay_log_filter(inherited: Option<OsString>) -> OsString {
    inherited.unwrap_or_else(|| OsString::from("info"))
}

fn toml_basic_string_path(path: &Path) -> String {
    toml_basic_string(path.to_str().expect("validated UTF-8 path"))
}

fn validate_utf8_path(path: &Path, label: &str) -> Result<(), ApiError> {
    if path.to_str().is_some() {
        return Ok(());
    }

    Err(ApiError::new(
        ApiErrorKind::ResourceStartFailed,
        format!(
            "{label} must be valid UTF-8 for defe's wire protocol: {}",
            path.display()
        ),
    ))
}

fn toml_basic_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn internal_error(message: impl Into<String>) -> ApiError {
    ApiError::new(ApiErrorKind::InternalServerError, message)
}

#[cfg(test)]
mod tests;
