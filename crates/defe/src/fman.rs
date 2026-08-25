use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use defe_api::{ApiError, ApiErrorKind, FmanInfo, FmanRequest, ResourceDescriptor};

use crate::resource_manager::{
    ManagedResource, ResourceAllocation, ResourceDriver, ResourceKind, ResourceSlotId,
};
use crate::resource_process::{ResourceProcess, ResourceProcessConfig, log_tail};

const LOCATOR_LOG_PREFIX: &str = "Fleet Manager locator: ";
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Setup-payment trust root every spawned Fleet Manager accepts: a test that
/// wants its published federation list admitted must sign it with this
/// well-known secret key.
const SETUP_PAYMENT_PUBLISHER_SECRET: &str =
    "0000000000000000000000000000000000000000000000000000000000000001";

fn setup_payment_publisher() -> String {
    nostr::Keys::parse(SETUP_PAYMENT_PUBLISHER_SECRET)
        .expect("well-known test secret key is valid")
        .public_key()
        .to_hex()
}

/// Starts and supervises exclusive local Fleet Manager resource slots.
pub struct FmanDriver {
    fleet_manager_bin: OsString,
    fman_cli_bin: OsString,
    resource_root: PathBuf,
    log_dir: PathBuf,
    stable: Mutex<HashMap<ResourceSlotId, StableFmanAllocation>>,
}

impl FmanDriver {
    /// Create a Fleet Manager driver using the given binaries and storage directories.
    #[must_use]
    pub fn new(
        fleet_manager_bin: impl Into<OsString>,
        fman_cli_bin: impl Into<OsString>,
        resource_root: impl Into<PathBuf>,
        log_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            fleet_manager_bin: fleet_manager_bin.into(),
            fman_cli_bin: fman_cli_bin.into(),
            resource_root: resource_root.into(),
            log_dir: log_dir.into(),
            stable: Mutex::new(HashMap::new()),
        }
    }

    fn stable_allocation(
        &self,
        allocation: &ResourceAllocation,
        request: &FmanRequest,
    ) -> Result<StableFmanAllocation, ApiError> {
        let mut stable = self
            .stable
            .lock()
            .map_err(|_| internal_error("Fleet Manager allocation mutex poisoned"))?;
        if let Some(existing) = stable.get(&allocation.slot_id) {
            return Ok(existing.clone());
        }

        let resource_id = format!("slot-{}", allocation.slot_id.0);
        // The operator HTTP listener needs a port that stays fixed across
        // restarts of this slot, so allocate it with the rest of the slot's
        // stable identity rather than per start.
        let admin_http_port = defe_portalloc::port_alloc(1).map_err(|error| {
            internal_error(format!(
                "failed to allocate Fleet Manager operator HTTP port: {error}"
            ))
        })?;
        let created = StableFmanAllocation {
            data_dir: self.resource_root.join("fman").join(resource_id),
            request: request.clone(),
            admin_http_port,
            admin_password: format!("defe-fman-operator-{}", allocation.slot_id.0),
        };
        stable.insert(allocation.slot_id, created.clone());
        Ok(created)
    }

    /// Onboard a freshly started Fleet Manager as a new one.
    ///
    /// Retried until the daemon is listening: a slot restarted on its stable
    /// data dir still has the previous run's socket file on disk for a moment.
    /// An already-onboarded daemon answers that it is, which is success here —
    /// the driver's contract is that the resource is up and onboarded, and a
    /// restart of the same slot keeps its identity.
    fn onboard(
        &self,
        data_dir: &Path,
        relay_url: &str,
        process: &ResourceProcess,
    ) -> Result<(), ApiError> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if !process.is_running() {
                return Err(ApiError::new(
                    ApiErrorKind::ResourceStartFailed,
                    "Fleet Manager exited before it could be onboarded".to_owned(),
                ));
            }
            let output = std::process::Command::new(&self.fman_cli_bin)
                .arg("--data-dir")
                .arg(data_dir)
                .arg("onboard")
                .arg("new")
                // The daemon answers whether onboarding was needed; the driver
                // does not read its refusal message to find out.
                .arg("--if-needed")
                .output()
                .map_err(|err| {
                    ApiError::new(
                        ApiErrorKind::ResourceStartFailed,
                        format!("failed to run the Fleet Manager onboarding verb: {err}"),
                    )
                })?;
            if output.status.success() {
                break;
            }
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            if Instant::now() >= deadline {
                return Err(ApiError::new(
                    ApiErrorKind::ResourceStartFailed,
                    format!("failed to onboard the Fleet Manager: {}", stderr.trim()),
                ));
            }
            std::thread::sleep(READY_POLL_INTERVAL);
        }

        let onboarding = self.admin(data_dir, &["onboarding"])?;
        let stage = onboarding
            .get("stage")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| internal_error("onboarding response has no stage"))?;
        if stage == "complete" {
            return Ok(());
        }
        if stage == "holder_authorization" {
            let subject = onboarding
                .get("service_nostr_pubkey")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| internal_error("onboarding response has no service Nostr pubkey"))?;
            let request = serde_json::json!({ "subject_pubkey": subject }).to_string();
            let issuer =
                PathBuf::from(&self.fleet_manager_bin).with_file_name("manifold-test-issuer");
            let issued = std::process::Command::new(&issuer)
                .args([
                    "--environment",
                    "development",
                    "--relay",
                    relay_url,
                    "--authorization-request",
                    &request,
                    "--publish-fman-authorization",
                ])
                .output()
                .map_err(|err| internal_error(format!("run {}: {err}", issuer.display())))?;
            if !issued.status.success() {
                return Err(internal_error(format!(
                    "test issuer failed: {}",
                    String::from_utf8_lossy(&issued.stderr).trim()
                )));
            }
            self.admin(data_dir, &["refresh-holder-authorizations"])?;
        }
        self.admin(data_dir, &["onboard", "offer", "--max-seats", "1"])?;
        Ok(())
    }

    fn admin(&self, data_dir: &Path, args: &[&str]) -> Result<serde_json::Value, ApiError> {
        let output = std::process::Command::new(&self.fman_cli_bin)
            .arg("--data-dir")
            .arg(data_dir)
            .args(args)
            .output()
            .map_err(|err| internal_error(format!("run Fleet Manager admin command: {err}")))?;
        if !output.status.success() {
            return Err(internal_error(format!(
                "Fleet Manager admin {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|err| internal_error(format!("parse Fleet Manager admin response: {err}")))
    }

    fn start_fman(
        &self,
        allocation: &ResourceAllocation,
        request: &FmanRequest,
    ) -> Result<Box<dyn ManagedResource>, ApiError> {
        let stable = self.stable_allocation(allocation, request)?;
        fs::create_dir_all(&stable.data_dir).map_err(|err| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "failed to create Fleet Manager data dir {}: {err}",
                    stable.data_dir.display()
                ),
            )
        })?;
        // The daemon refuses a password file that is group- or world-readable.
        let password_path = stable.data_dir.join("operator-password");
        write_operator_password(&password_path, &stable.admin_password)?;
        let admin_http_bind = format!("127.0.0.1:{}", stable.admin_http_port);
        let log_path = self.log_dir.join(format!(
            "fman-slot-{}-generation-{}.log",
            allocation.slot_id.0, allocation.generation
        ));
        let process = Arc::new(
            ResourceProcess::spawn(
                ResourceProcessConfig::new(
                    self.fleet_manager_bin.clone(),
                    log_path.clone(),
                    log_path.clone(),
                )
                .args([
                    "serve".into(),
                    "--data-dir".into(),
                    stable.data_dir.clone().into_os_string(),
                    "--bitcoind-url".into(),
                    stable.request.bitcoind.rpc_url.clone().into(),
                    "--bitcoind-username".into(),
                    stable.request.bitcoind.rpc_username.clone().into(),
                    "--bitcoind-password".into(),
                    stable.request.bitcoind.rpc_password.clone().into(),
                    "--first-port-base".into(),
                    stable.request.first_port_base.to_string().into(),
                    "--manifold-environment".into(),
                    "development".into(),
                    "--admin-http-bind".into(),
                    admin_http_bind.clone().into(),
                    "--admin-http-auth".into(),
                    "password".into(),
                    "--admin-http-password-file".into(),
                    password_path.clone().into_os_string(),
                ])
                .env(
                    fedi_decentralized_manifold_environment::DEV_NOSTR_RELAYS_ENV,
                    stable.request.nostr_relay_url.clone(),
                )
                .env(
                    fedi_decentralized_manifold_environment::DEV_SETUP_PAYMENT_PUBLISHER_ENV,
                    setup_payment_publisher(),
                )
                .env("FMAN_E2E_LOCAL_IROH", "1")
                .env(
                    "FM_IROH_CONNECT_OVERRIDES",
                    stable.request.iroh_connect_overrides.clone(),
                ),
            )
            .map_err(|err| {
                ApiError::new(
                    ApiErrorKind::ResourceStartFailed,
                    format!(
                        "failed to spawn Fleet Manager {}: {err}",
                        self.fleet_manager_bin.to_string_lossy()
                    ),
                )
            })?,
        );

        // A Fleet Manager acquires its identity by being onboarded, so a
        // freshly started one is waiting on its admin socket rather than
        // serving RPC. The harness makes the same call an operator would.
        if let Err(err) = self.onboard(&stable.data_dir, &stable.request.nostr_relay_url, &process)
        {
            let _ = process.stop();
            return Err(err);
        }

        let locator = match wait_for_locator(&process, &log_path) {
            Ok(locator) => locator,
            Err(err) => {
                let _ = process.stop();
                return Err(err);
            }
        };

        Ok(Box::new(FmanResource {
            descriptor: FmanInfo {
                locator,
                data_dir: stable.data_dir,
                iroh_connect_overrides: stable.request.iroh_connect_overrides,
                admin_url: format!("http://{admin_http_bind}"),
                admin_password: stable.admin_password,
            },
            process,
        }))
    }
}

impl ResourceDriver for FmanDriver {
    fn start(&self, allocation: &ResourceAllocation) -> Result<Box<dyn ManagedResource>, ApiError> {
        let ResourceKind::Fman(request) = &allocation.kind else {
            return Err(ApiError::new(
                ApiErrorKind::ResourceKindUnavailable,
                format!("Fleet Manager driver cannot start {:?}", allocation.kind),
            ));
        };
        self.start_fman(allocation, request)
    }
}

#[derive(Clone)]
struct StableFmanAllocation {
    data_dir: PathBuf,
    request: FmanRequest,
    admin_http_port: u16,
    admin_password: String,
}

struct FmanResource {
    descriptor: FmanInfo,
    process: Arc<ResourceProcess>,
}

impl ManagedResource for FmanResource {
    fn descriptor(&self) -> ResourceDescriptor {
        ResourceDescriptor::Fman(self.descriptor.clone())
    }

    fn is_running(&self) -> bool {
        self.process.is_running()
    }

    fn stop(&mut self) {
        let _ = self.process.stop();
    }
}

fn wait_for_locator(process: &ResourceProcess, log_path: &Path) -> Result<String, ApiError> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if let Ok(log) = fs::read_to_string(log_path)
            && let Some(locator) = log
                .lines()
                .find_map(|line| line.strip_prefix(LOCATOR_LOG_PREFIX))
        {
            return Ok(locator.to_owned());
        }
        if !process.is_running() {
            return Err(ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "Fleet Manager exited before printing a locator; {}",
                    log_tail(log_path)
                ),
            ));
        }
        if Instant::now() >= deadline {
            return Err(ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "Fleet Manager did not print a locator before timeout; {}",
                    log_tail(log_path)
                ),
            ));
        }
        std::thread::sleep(READY_POLL_INTERVAL);
    }
}

/// Write the operator password owner-only, as the daemon requires.
fn write_operator_password(path: &Path, password: &str) -> Result<(), ApiError> {
    let write = || -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)?;
            file.write_all(password.as_bytes())
        }
        #[cfg(not(unix))]
        fs::write(path, password)
    };
    write().map_err(|err| {
        ApiError::new(
            ApiErrorKind::ResourceStartFailed,
            format!(
                "failed to write Fleet Manager operator password {}: {err}",
                path.display()
            ),
        )
    })
}

fn internal_error(message: impl Into<String>) -> ApiError {
    ApiError::new(ApiErrorKind::InternalServerError, message)
}
