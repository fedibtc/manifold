use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use defe_api::{ApiError, ApiErrorKind, FlipInfo, ResourceDescriptor};
use secp256k1::{Keypair, Secp256k1, SecretKey, XOnlyPublicKey};

use crate::resource_manager::{
    ManagedResource, ResourceAllocation, ResourceDriver, ResourceKind, ResourceSlotId,
};
use crate::resource_process::{ResourceProcess, ResourceProcessConfig, log_tail};

const READY_TIMEOUT: Duration = Duration::from_secs(30);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const ADMIN_TOKEN: &str = "flip-local-admin-token";

/// Starts and supervises exclusive local FLIP daemon slots.
pub struct FlipDriver {
    daemon_bin: OsString,
    resource_root: PathBuf,
    log_dir: PathBuf,
    stable: Mutex<HashMap<ResourceSlotId, StableFlipAllocation>>,
}

impl FlipDriver {
    #[must_use]
    pub fn new(
        daemon_bin: impl Into<OsString>,
        resource_root: impl Into<PathBuf>,
        log_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            daemon_bin: daemon_bin.into(),
            resource_root: resource_root.into(),
            log_dir: log_dir.into(),
            stable: Mutex::new(HashMap::new()),
        }
    }

    fn stable_allocation(
        &self,
        allocation: &ResourceAllocation,
        request: &defe_api::FlipRequest,
    ) -> Result<StableFlipAllocation, ApiError> {
        let mut stable = self
            .stable
            .lock()
            .map_err(|_| internal_error("FLIP allocation mutex poisoned"))?;
        if let Some(existing) = stable.get(&allocation.slot_id) {
            return Ok(existing.clone());
        }

        let ports = defe_portalloc::port_alloc(2).map_err(|error| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!("allocate FLIP ports: {error}"),
            )
        })?;
        let public_port = ports.checked_add(1).ok_or_else(|| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                "allocated FLIP port range overflowed",
            )
        })?;
        let data_dir = self
            .resource_root
            .join("flip")
            .join(format!("slot-{}", allocation.slot_id.0));
        let mut provider_secret = [0_u8; 32];
        provider_secret[31] =
            u8::try_from((allocation.slot_id.0 % 254) + 1).expect("modulo result fits in u8");
        let provider_secret = SecretKey::from_byte_array(&provider_secret)
            .expect("non-zero deterministic Defe FLIP provider key");
        let secp = Secp256k1::new();
        let provider_keypair = Keypair::from_secret_key(&secp, &provider_secret);
        let (provider_pubkey, _) = XOnlyPublicKey::from_keypair(&provider_keypair);
        let created = StableFlipAllocation {
            data_dir: data_dir.clone(),
            trust_fixtures_dir: data_dir.join("trust-fixtures"),
            admin_port: ports,
            public_port,
            provider_secret_hex: provider_secret.display_secret().to_string(),
            provider_pubkey_hex: provider_pubkey.to_string(),
            iroh_connect_overrides: request.iroh_connect_overrides.clone(),
            holder_authorization_relay_url: request.holder_authorization_relay_url.clone(),
        };
        stable.insert(allocation.slot_id, created.clone());
        Ok(created)
    }

    fn start_flip(
        &self,
        allocation: &ResourceAllocation,
        request: &defe_api::FlipRequest,
    ) -> Result<Box<dyn ManagedResource>, ApiError> {
        let stable = self.stable_allocation(allocation, request)?;
        fs::create_dir_all(&stable.trust_fixtures_dir).map_err(|error| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "create FLIP trust-fixtures directory {}: {error}",
                    stable.trust_fixtures_dir.display()
                ),
            )
        })?;
        let log_path = self.log_dir.join(format!(
            "flip-slot-{}-generation-{}.log",
            allocation.slot_id.0, allocation.generation
        ));
        let mut config =
            ResourceProcessConfig::new(self.daemon_bin.clone(), log_path.clone(), log_path)
                .args([
                    "run".into(),
                    "daemon".into(),
                    "--manifold-environment".into(),
                    "development".into(),
                    "--data-dir".into(),
                    stable.data_dir.clone().into_os_string(),
                    "--admin-bind-address".into(),
                    format!("127.0.0.1:{}", stable.admin_port).into(),
                    "--public-bind-address".into(),
                    format!("127.0.0.1:{}", stable.public_port).into(),
                    "--bootstrap-admin-token".into(),
                    ADMIN_TOKEN.into(),
                    "--trust-fixtures".into(),
                    stable.trust_fixtures_dir.clone().into_os_string(),
                ])
                .env(
                    "FLIP_PROVIDER_NOSTR_SECRET_KEY",
                    &stable.provider_secret_hex,
                )
                .env("FM_IN_DEVIMINT", "1");
        if let Some(overrides) = &stable.iroh_connect_overrides {
            config = config.env("FM_IROH_CONNECT_OVERRIDES", overrides);
        }
        if let Some(relay_url) = &stable.holder_authorization_relay_url {
            config = config.env("MANIFOLD_DEV_NOSTR_RELAYS", relay_url);
        }
        let process = Arc::new(ResourceProcess::spawn(config).map_err(|error| {
            ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "failed to spawn FLIP daemon {}: {error}",
                    self.daemon_bin.to_string_lossy()
                ),
            )
        })?);
        if let Err(error) = wait_for_tcp(&process, stable.admin_port) {
            let _ = process.stop();
            return Err(error);
        }
        Ok(Box::new(FlipResource {
            descriptor: FlipInfo {
                admin_url: format!("http://127.0.0.1:{}", stable.admin_port),
                admin_token: ADMIN_TOKEN.to_owned(),
                data_dir: stable.data_dir,
                trust_fixtures_dir: stable.trust_fixtures_dir,
                provider_pubkey_hex: stable.provider_pubkey_hex,
            },
            process,
        }))
    }
}

impl ResourceDriver for FlipDriver {
    fn start(&self, allocation: &ResourceAllocation) -> Result<Box<dyn ManagedResource>, ApiError> {
        let ResourceKind::Flip(request) = &allocation.kind else {
            return Err(ApiError::new(
                ApiErrorKind::ResourceKindUnavailable,
                format!("FLIP driver cannot start {:?}", allocation.kind),
            ));
        };
        self.start_flip(allocation, request)
    }
}

#[derive(Clone)]
struct StableFlipAllocation {
    data_dir: PathBuf,
    trust_fixtures_dir: PathBuf,
    admin_port: u16,
    public_port: u16,
    provider_secret_hex: String,
    provider_pubkey_hex: String,
    iroh_connect_overrides: Option<String>,
    holder_authorization_relay_url: Option<String>,
}

struct FlipResource {
    descriptor: FlipInfo,
    process: Arc<ResourceProcess>,
}

impl ManagedResource for FlipResource {
    fn descriptor(&self) -> ResourceDescriptor {
        ResourceDescriptor::Flip(self.descriptor.clone())
    }

    fn is_running(&self) -> bool {
        self.process.is_running()
    }

    fn stop(&mut self) {
        let _ = self.process.stop();
    }
}

fn wait_for_tcp(process: &ResourceProcess, port: u16) -> Result<(), ApiError> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(100),
        )
        .is_ok()
        {
            return Ok(());
        }
        if !process.is_running() {
            return Err(ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "FLIP daemon exited before accepting connections; {}",
                    log_tail(process.stdout_log())
                ),
            ));
        }
        if Instant::now() >= deadline {
            return Err(ApiError::new(
                ApiErrorKind::ResourceStartFailed,
                format!(
                    "FLIP daemon did not accept connections before timeout; {}",
                    log_tail(process.stdout_log())
                ),
            ));
        }
        std::thread::sleep(READY_POLL_INTERVAL);
    }
}

fn internal_error(message: impl Into<String>) -> ApiError {
    ApiError::new(ApiErrorKind::InternalServerError, message)
}
