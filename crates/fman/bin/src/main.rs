//! L7 daemon: CLI args → open the fleet → serve the iroh RPC and the local
//! admin socket.
//!
//! This binary is also the `fedimintd` its seats run
//! ([`bundled_fedimintd`]).
//!
//! On exit (Ctrl-C or SIGTERM) the router shuts down and [`Fleet::shutdown`]
//! stops every seat process before the runtime exits; Linux children also
//! receive a parent-death signal if the FMan is hard-killed.

#[cfg(feature = "embedded-operator-ui")]
mod operator_ui;
mod push_callback;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, ValueEnum};
use fedi_decentralized_manifold_environment::{ManifoldEnvironment, ManifoldEnvironmentProfile};
use fedi_decentralized_service_fleet_manager::{
    FEDIMINTD_VENDOR_0_1, FLEET_MANAGER_ALPN, FleetManagerServiceServer, GUARDIAN_TELEMETRY_ALPN,
    GuardianTelemetryApiServer, Locator,
};
use fedi_iroh_rpc::IrohProtocol;
use fman_core::admin;
use fman_core::admin_http::{self, AdminHttpAuth};
use fman_core::bundled_fedimintd;

use fedimint_core::Amount;
use fedimint_core::util::SafeUrl;
use fedimint_server_core::ServerModuleInitRegistry;
use fman_core::facts::PortBase;
use fman_core::fleet::{
    Fleet, FleetConfig, FleetHolderAuthorizationStore, FleetNostrHost, FleetSetupPaymentPolicyStore,
};
use fman_core::push_callback::{
    DEFAULT_PUSH_CALLBACK_RETRY_INTERVAL, PushGatewayOrigin, PushGatewayOriginPolicy,
};
use fman_core::seat_process::{
    BitcoinBackend, BitcoindConfig, RespawnPolicy, SeatProcessConfig, SeatProcessSpawner,
};
use fman_core::service::FleetManagerRpc;
use fman_telemetry::GuardianTelemetryRpc;
use iroh::Endpoint;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use push_callback::PushGatewayCallbackInvoker;
use stability_pool_server::StabilityPoolInit;
use stability_pool_server::common::config::{CollateralRatio, OracleConfig};
use tracing_subscriber::prelude::*;

const MANIFOLD_SPV2_CYCLE_DURATION: Duration = Duration::from_secs(10 * 60);
const MANIFOLD_SPV2_MIN_SEEK: Amount = Amount::from_msats(10_000);
const MANIFOLD_SPV2_MIN_PROVIDE: Amount = Amount::from_msats(100_000);
const MANIFOLD_SPV2_MAX_PROVIDER_FEE_RATE_PPB: u64 = 22_062;
const MANIFOLD_SPV2_MIN_CANCELLATION_BPS: u32 = 100;
const N0_PKARR_RELAY: &str = "https://dns.iroh.link/pkarr";

#[cfg(test)]
mod tests;

#[cfg(not(any(target_env = "msvc", target_os = "ios", target_os = "android")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// No `Debug`: `Serve` carries the bitcoind password.
#[derive(Parser)]
#[command(name = "fleet-manager", about = "Fleet Manager daemon")]
enum Args {
    /// Run the daemon.
    Serve(Box<ServeArgs>),
}

// No `Debug`: carries the bitcoind password.
#[derive(Parser)]
struct ServeArgs {
    /// SQLite + per-seat data root.
    #[arg(long)]
    data_dir: PathBuf,
    /// Bitcoin Core RPC URL. When omitted, use the selected environment's
    /// public default Esplora backend; Development and Production have none.
    #[arg(long, requires_all = ["bitcoind_username", "bitcoind_password"])]
    bitcoind_url: Option<String>,
    /// Bitcoin Core RPC username.
    #[arg(long, requires = "bitcoind_url")]
    bitcoind_username: Option<String>,
    /// Bitcoin Core RPC password (plaintext CLI accepted under the
    /// single-tenant-host trust model; ARCH-fleet-manager *Trust
    /// boundaries*).
    #[arg(long, requires = "bitcoind_url")]
    bitcoind_password: Option<String>,
    /// First seat port block on the `base + 4k` grid. The grid is
    /// per-host: multiple FMans sharing a host (the E2E harness) must be
    /// given disjoint grids.
    #[arg(long, default_value_t = 30_000)]
    first_port_base: u16,
    /// Canonical Manifold environment profile: the single source of the
    /// active Nostr relays and the setup-payment publisher identity.
    /// Development deployments override those through the
    /// `MANIFOLD_DEV_*` environment variables; staging and production
    /// refuse the overrides.
    #[arg(long, env = "FLEET_MANAGER_MANIFOLD_ENVIRONMENT")]
    manifold_environment: ManifoldEnvironment,
    /// Pkarr HTTP relay passed to every seat's fedimintd alongside default n0
    /// DNS discovery.
    #[arg(
        long,
        env = "FLEET_MANAGER_IROH_DNS",
        default_value = N0_PKARR_RELAY
    )]
    iroh_dns: SafeUrl,
    /// Exact public push-gateway origin accepted for callback bearer URLs.
    #[arg(long, env = "FLEET_MANAGER_PUSH_GATEWAY_ORIGIN")]
    push_gateway_origin: Option<String>,
    /// Allow a loopback HTTP gateway in the development environment only.
    #[arg(
        long,
        env = "FLEET_MANAGER_ALLOW_INSECURE_PUSH_GATEWAY_ORIGIN",
        default_value_t = false
    )]
    allow_insecure_push_gateway_origin: bool,
    /// Bind the private operator web UI and HTTP admin API.
    #[arg(long)]
    admin_http_bind: Option<SocketAddr>,
    /// Authentication boundary for the operator HTTP listener.
    #[arg(long, value_enum, requires = "admin_http_bind")]
    admin_http_auth: Option<AdminHttpAuthArg>,
    /// File containing the generated operator password. Required with
    /// `--admin-http-auth password`; forbidden with `trusted-proxy`.
    #[arg(long, requires = "admin_http_bind")]
    admin_http_password_file: Option<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum AdminHttpAuthArg {
    /// Trust an authenticating platform proxy on an isolated private network.
    TrustedProxy,
    /// Show the vendored Fedimint login flow and verify a generated password.
    Password,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Before this CLI is parsed: fedimintd owns the argv it was spawned with.
    if is_bundled_fedimintd() {
        let safe_event_layer = std::env::var_os(fman_core::seat_process::SAFE_EVENT_DIR_ENV)
            .and_then(
                |directory| match safe_tracing::layer(PathBuf::from(directory)) {
                    Ok(layer) => Some(layer),
                    Err(error) => {
                        eprintln!("safe-event journal unavailable; continuing without it: {error}");
                        None
                    }
                },
            );
        let never = run_bundled_fedimintd(safe_event_layer).await?;
        match never {}
    }
    fedimint_core::rustls::install_crypto_provider().await;
    let args = Args::parse();
    match args {
        Args::Serve(args) => serve(*args).await,
    }
}

/// The child-process configuration, built once and used by both the restore
/// pre-pass and the fleet it hands over to: a restore that wrote seat
/// directories anywhere else would be recovering into a fleet that cannot find
/// them.
fn seat_process_config(
    args: &ServeArgs,
    manifold_environment: &ManifoldEnvironmentProfile,
) -> anyhow::Result<SeatProcessConfig> {
    let bitcoin_backend = match (
        &args.bitcoind_url,
        &args.bitcoind_username,
        &args.bitcoind_password,
    ) {
        (Some(url), Some(username), Some(password)) => {
            BitcoinBackend::Bitcoind(BitcoindConfig {
                url: url.clone(),
                username: username.clone(),
                password: password.clone(),
            })
        }
        (None, None, None) => BitcoinBackend::Esplora(
            manifold_environment
                .default_esplora_url()
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "the {} Manifold environment has no default Esplora backend; supply --bitcoind-url, --bitcoind-username, and --bitcoind-password",
                        manifold_environment.environment()
                    )
                })?,
        ),
        _ => unreachable!("clap requires complete Bitcoin Core configuration"),
    };
    Ok(SeatProcessConfig {
        data_root: args.data_dir.clone(),
        bitcoin_network: manifold_environment.bitcoin_network(),
        bitcoin_backend,
        iroh_dns: args.iroh_dns.clone(),
    })
}

/// The operator listener's bind address and authentication boundary, or `None`
/// when the deployment configured no listener.
///
/// Resolved before onboarding, and from the deployment's own files only: the
/// password file is written by the packaging, not derived from the fleet
/// identity, so nothing here waits on a fleet. That ordering is what lets the
/// dashboard be the thing that onboards the host
/// (`crates/fman/specs/SPEC-operator-http.md`, *The onboarding phase*).
fn admin_http_config(args: &ServeArgs) -> anyhow::Result<Option<(SocketAddr, AdminHttpAuth)>> {
    match (args.admin_http_bind, args.admin_http_auth) {
        (None, None) if args.admin_http_password_file.is_none() => Ok(None),
        (Some(bind), Some(AdminHttpAuthArg::TrustedProxy)) => {
            anyhow::ensure!(
                args.admin_http_password_file.is_none(),
                "--admin-http-password-file is forbidden with trusted-proxy auth"
            );
            Ok(Some((bind, AdminHttpAuth::TrustedProxy)))
        }
        (Some(bind), Some(AdminHttpAuthArg::Password)) => {
            let path = args.admin_http_password_file.as_ref().ok_or_else(|| {
                anyhow::anyhow!("--admin-http-password-file is required with password auth")
            })?;
            let metadata = std::fs::metadata(path)
                .with_context(|| format!("inspect operator password file {}", path.display()))?;
            anyhow::ensure!(metadata.is_file(), "operator password path is not a file");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                anyhow::ensure!(
                    metadata.permissions().mode() & 0o077 == 0,
                    "operator password file must not be accessible by group or others"
                );
            }
            let password = std::fs::read_to_string(path)
                .with_context(|| format!("read operator password from {}", path.display()))?;
            let password = password.trim_end_matches(['\r', '\n']);
            anyhow::ensure!(
                !password.is_empty() && password.len() <= 1024,
                "operator password must be between 1 and 1024 bytes"
            );
            Ok(Some((bind, AdminHttpAuth::Password(password.to_owned()))))
        }
        _ => anyhow::bail!("--admin-http-bind and --admin-http-auth must be provided together"),
    }
}

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let manifold_environment = args
        .manifold_environment
        .profile()
        .map_err(|err| anyhow::anyhow!("resolve Manifold environment profile: {err}"))?;
    anyhow::ensure!(
        !args.allow_insecure_push_gateway_origin
            || manifold_environment.environment() == ManifoldEnvironment::Development,
        "--allow-insecure-push-gateway-origin is development-only"
    );
    anyhow::ensure!(
        !args.allow_insecure_push_gateway_origin || args.push_gateway_origin.is_some(),
        "--allow-insecure-push-gateway-origin requires --push-gateway-origin"
    );
    let push_gateway_origin = args
        .push_gateway_origin
        .as_deref()
        .map(|origin| {
            let policy = if args.allow_insecure_push_gateway_origin {
                PushGatewayOriginPolicy::AllowInsecureLoopback
            } else {
                PushGatewayOriginPolicy::HttpsOnly
            };
            PushGatewayOrigin::parse(origin, policy)
        })
        .transpose()?;
    init_daemon_logging(args.data_dir.join("safe-events").join("fman"))?;
    tracing::info!(
        manifold_environment = %manifold_environment.environment(),
        manifold_profile_revision = manifold_environment.profile_revision(),
        "selected Manifold environment profile"
    );

    // Fail fast on a password the child environment cannot carry.
    if let Some(password) = &args.bitcoind_password {
        anyhow::ensure!(
            password.len() <= 1024 && !password.contains('\0'),
            "--bitcoind-password must be at most 1024 bytes with no NUL"
        );
    }

    let process = seat_process_config(&args, &manifold_environment)?;
    tracing::info!(
        bitcoin_network = %process.bitcoin_network,
        bitcoin_backend = match &process.bitcoin_backend {
            BitcoinBackend::Esplora(_) => "esplora",
            BitcoinBackend::Bitcoind(_) => "bitcoind",
        },
        "selected Bitcoin chain backend"
    );

    let wallet_dir = args.data_dir.join("wallet");
    // Advertisements and reads go to every profile relay through the Nostr
    // boundary's pool. Backups keep a single relay — the first canonical one
    // — so a restore knows exactly where its documents live; multi-relay
    // backup durability is a separate, planned change.
    let backup_relay_url = manifold_environment.nostr_relays().as_urls()[0].to_string();

    // Resolved before onboarding, because the operator answers the onboarding
    // question in this dashboard. The password comes from a file the
    // deployment wrote, not from the identity, so nothing here needs a fleet.
    let operator_http = admin_http_config(&args)?;

    // Phase one: complete operator onboarding before opening any fleet,
    // including restored guardian children.
    tokio::fs::create_dir_all(&args.data_dir).await?;
    let db = fman_core::db::Db::open(&args.data_dir).await?;
    db.bind_manifold_environment(manifold_environment.environment())
        .await?;
    // Operator surfaces bind once, before the fleet exists, and serve every
    // life of this daemon: the onboarding wizard until its final stage is
    // durable, then — switched in place, never rebound — the fleet.
    let onboarding = fman_core::onboarding::Onboarding::new(
        db.clone(),
        process.clone(),
        // A restore reads from the single relay this host backs up to.
        Arc::new(fman_nostr::backup::RelayBackupArchive::new(
            backup_relay_url.clone(),
        )),
        Arc::new(fman_nostr::NostrHolderAuthorizationFetcher::new(
            manifold_environment.nostr_relays().as_urls().to_vec(),
            Arc::new(FleetHolderAuthorizationStore::new(db.clone())),
        )),
        manifold_environment.setup_payment_publisher().is_some(),
    );
    let phase = admin::OperatorPhase::onboarding(onboarding.clone());
    let _admin = admin::serve(&phase, &admin::socket_path(&args.data_dir))?;
    let _admin_http = match operator_http {
        Some((bind, auth)) => {
            let (bound, task) =
                admin_http::serve(with_operator_ui(admin_http::router(&phase, auth)), bind).await?;
            // Two events on purpose. The lifecycle fact — this daemon is
            // serving the dashboard — carries no address and is safe to share.
            // The address is not: `--admin-http-bind` is the *private* operator
            // UI, and on a LAN deployment it renders the host's own interface,
            // which is host-specific configuration the sharing policy rejects.
            // It stays in the operator's local log.
            tracing::info!(safe_to_share = true, "serving the operator dashboard");
            tracing::info!(%bound, "operator dashboard listener bound");
            Some(task)
        }
        None => None,
    };
    if db.onboarding_stage().await? != fman_core::db::OnboardingStage::Complete {
        tracing::info!(
            safe_to_share = true,
            "this Fleet Manager has not completed onboarding; waiting for operator setup"
        );
    }
    onboarding.completed().await?;

    let wallet_origin = db.wallet_origin().await?;
    let fleet = Arc::new(
        Fleet::open_with_wallet(
            db,
            FleetConfig {
                manifold_environment: manifold_environment.environment(),
                first_port_base: PortBase::new(args.first_port_base).ok_or_else(|| {
                    anyhow::anyhow!("--first-port-base leaves no room for a seat's port block")
                })?,
                setup_payments_configured: manifold_environment.setup_payment_publisher().is_some(),
                respawn: RespawnPolicy::default(),
                backup_scan_interval: fman_core::backup_worker::DEFAULT_SCAN_INTERVAL,
                push_gateway_origin,
                push_callback_retry_interval: DEFAULT_PUSH_CALLBACK_RETRY_INTERVAL,
                completion_callback_invoker: Arc::new(PushGatewayCallbackInvoker::new()),
                process_spawner: SeatProcessSpawner::Bundled,
                process,
            },
            async |identity| {
                let wallet = fman_fedimint::Wallet::open_guarding(
                    wallet_dir,
                    &identity.derive_wallet_secret(),
                    &identity.derive_guardian_fee_secret(),
                    wallet_origin,
                )
                .await?;
                Ok(std::sync::Arc::new(wallet) as _)
            },
            async |identity| {
                Ok(
                    std::sync::Arc::new(fman_nostr::backup::RelayBackupSink::new(
                        fman_nostr::format::BackupIdentity::derive(identity),
                        backup_relay_url,
                    )) as _,
                )
            },
        )
        .await?,
    );

    let identity = fleet.identity();
    let service_pubkey = identity.derive_service_pubkey();
    let setup_payment_publisher = manifold_environment.setup_payment_publisher().copied();
    let policy_store = Arc::new(FleetSetupPaymentPolicyStore::new(fleet.clone()));
    // Revalidate retained authority before exposing the RPC router. A publisher
    // rotation must fail startup without a window serving the old policy.
    let retained_setup_payment_federations = match setup_payment_publisher {
        Some(publisher) => {
            fman_nostr::load_retained_setup_payment_policy(&policy_store, publisher).await?
        }
        None => None,
    };
    // The network-isolated formation harness supplies explicit loopback routes.
    let local_e2e = std::env::var_os("FMAN_E2E_LOCAL_IROH").is_some();
    let endpoint_builder = if local_e2e {
        Endpoint::builder(presets::N0DisableRelay)
    } else {
        Endpoint::builder(presets::N0)
    };
    let endpoint = endpoint_builder
        .secret_key(identity.derive_iroh_secret_key())
        .bind()
        .await?;
    let keys = identity.derive_service_nostr_keys();
    let telemetry_registration_keys = keys.clone();
    let holder_authorization_store = Arc::new(FleetHolderAuthorizationStore::for_fleet(&fleet));
    let retained_holder_authorizations = fman_nostr::load_retained_holder_authorizations(
        &holder_authorization_store,
        keys.public_key(),
    )
    .await?;
    let guardian_verification_fee_account = manifold_environment
        .guardian_verification_fee_account()
        .cloned();
    let nostr = fman_nostr::FleetManagerNostr::new(
        keys,
        setup_payment_publisher,
        retained_holder_authorizations,
        retained_setup_payment_federations,
        manifold_environment,
    );
    // Construct the RPC only after the Nostr policy watch exists, so policy is
    // ordinary constructor-owned state rather than a late-bound service mode.
    let rpc = FleetManagerRpc::new(
        fleet.clone(),
        guardian_verification_fee_account,
        nostr.subscribe_setup_payment_federations(),
    );
    let server = FleetManagerServiceServer::new(rpc.clone());
    let telemetry = GuardianTelemetryApiServer::new(GuardianTelemetryRpc::new(fleet.clone())?);
    let router = Router::builder(endpoint)
        .accept(FLEET_MANAGER_ALPN, IrohProtocol::new(server))
        .accept(
            GUARDIAN_TELEMETRY_ALPN,
            IrohProtocol::with_limits_and_request_read_timeout(
                telemetry,
                4 * 1024,
                8,
                std::time::Duration::from_secs(5),
            ),
        )
        .spawn();

    let host = Arc::new(FleetNostrHost::new(
        fleet.clone(),
        router.endpoint().id().to_string(),
        service_pubkey,
    ));
    rpc.bind_trust_material_source(Arc::new(NostrTrustMaterialSource::new(
        router.endpoint().id().to_string(),
        nostr.clone(),
    )));
    let _nostr = nostr.start(host, policy_store);
    let telemetry_registration = fman_telemetry::start_registration(
        fleet.clone(),
        nostr.clone(),
        router.endpoint().id().to_string(),
        telemetry_registration_keys,
        nostr.subscribe_setup_payment_federations(),
    );
    let join_reconciler = fman_fedimint::setup_payment_policy::spawn_setup_payment_join_reconciler(
        fleet.wallet().clone(),
        nostr.subscribe_setup_payment_federations(),
    );
    phase.open_fleet(fleet.clone(), nostr.presence());

    // The connection card an FI needs to reach this FMan: printed to stdout
    // behind the prefix the e2e harnesses read. They are the only consumers,
    // so production prints nothing and never waits for relay onlineness.
    if local_e2e {
        let locator = Locator::new(router.endpoint().addr(), service_pubkey);
        println!("{}{}", Locator::LOG_PREFIX, locator.to_json());
    }
    tracing::info!(
        %service_pubkey,
        "Fleet Manager serving FI and capability-scoped telemetry Iroh RPC; press Ctrl-C to stop"
    );

    shutdown_signal().await?;
    telemetry_registration.shutdown().await;
    router.shutdown().await?;
    // Stop and join every wallet-join task before shutting down the fleet.
    // Cancellation may leave dependency-owned same-partition work, but the
    // process-owned attempt registry prevents another join before process exit.
    let join_shutdown = join_reconciler.shutdown().await;
    fleet.shutdown().await;
    join_shutdown?;
    Ok(())
}

/// Supervisors stop services with SIGTERM; an interactive operator uses
/// Ctrl-C (SIGINT). Both must reach the graceful path below — seat processes,
/// the router, and bearer-holding workers are shut down in order — rather
/// than only the parent-death signal the children get on a hard kill.
async fn shutdown_signal() -> anyhow::Result<()> {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        _ = sigterm.recv() => {}
    }
    Ok(())
}

#[cfg(feature = "embedded-operator-ui")]
fn with_operator_ui(router: axum::Router) -> axum::Router {
    router.merge(operator_ui::router())
}

#[cfg(not(feature = "embedded-operator-ui"))]
fn with_operator_ui(router: axum::Router) -> axum::Router {
    router
}

/// Whether this process was spawned as the bundled `fedimintd`. Must be
/// consulted before the daemon parses its own CLI: fedimintd owns the argv.
fn is_bundled_fedimintd() -> bool {
    std::env::args_os().next().is_some_and(|argv0| {
        std::path::Path::new(&argv0).file_name()
            == Some(std::ffi::OsStr::new(bundled_fedimintd::ARGV0))
    })
}

fn manifold_stability_pool_init() -> StabilityPoolInit {
    StabilityPoolInit {
        // The local E2E environment is network-isolated, so its real
        // fedimintd processes use the module's deterministic oracle rather
        // than waiting forever on public price sources.
        oracle_config: if std::env::var_os("FMAN_E2E_LOCAL_IROH").is_some() {
            OracleConfig::Mock
        } else {
            OracleConfig::Aggregate
        },
        cycle_duration: MANIFOLD_SPV2_CYCLE_DURATION,
        collateral_ratio: CollateralRatio {
            provider: 1,
            seeker: 1,
        },
        min_allowed_seek: MANIFOLD_SPV2_MIN_SEEK,
        min_allowed_provide: MANIFOLD_SPV2_MIN_PROVIDE,
        max_allowed_provide_fee_rate_ppb: MANIFOLD_SPV2_MAX_PROVIDER_FEE_RATE_PPB,
        min_allowed_cancellation_bps: MANIFOLD_SPV2_MIN_CANCELLATION_BPS,
    }
}

fn manifold_modules() -> ServerModuleInitRegistry {
    let mut modules = fedimintd::default_modules();
    modules.attach(manifold_stability_pool_init());
    modules
}

/// Run the bundled `fedimintd`; only returns on failure. FMan's formation
/// client selects the exact Manifold v2 module set during DKG.
async fn run_bundled_fedimintd(
    safe_event_layer: Option<safe_tracing::BoxedSafeEventLayer>,
) -> anyhow::Result<std::convert::Infallible> {
    if let Some(layer) = safe_event_layer {
        fedimintd::run_with_extra_logging_layer(
            manifold_modules(),
            fedimint_core::fedimint_build_code_version_env!(),
            Some(FEDIMINTD_VENDOR_0_1),
            layer,
        )
        .await
    } else {
        fedimintd::run(
            manifold_modules(),
            fedimint_core::fedimint_build_code_version_env!(),
            Some(FEDIMINTD_VENDOR_0_1),
        )
        .await
    }
}

/// Initialize independent ordinary and explicitly shareable daemon log layers.
fn init_daemon_logging(directory: PathBuf) -> anyhow::Result<()> {
    let safe_events = match safe_tracing::layer(directory) {
        Ok(layer) => Some(layer),
        Err(error) => {
            eprintln!("safe-event journal unavailable; continuing without it: {error}");
            None
        }
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(filter);
    tracing_subscriber::registry()
        .with(safe_events)
        .with(stderr)
        .try_init()?;
    Ok(())
}

/// Serves the public trust-material verb from the running Nostr runtime.
///
/// The endpoint URL is the same `iroh://{endpoint_id}` the advertisement
/// publishes, so a verifier that resolved this FMan by its advertisement and
/// one that fetched its trust material see the same dial address.
pub struct NostrTrustMaterialSource {
    iroh_endpoint_id: String,
    nostr: fman_nostr::FleetManagerNostr,
}

impl NostrTrustMaterialSource {
    pub fn new(iroh_endpoint_id: String, nostr: fman_nostr::FleetManagerNostr) -> Self {
        Self {
            iroh_endpoint_id,
            nostr,
        }
    }
}

impl fman_core::service::TrustMaterialSource for NostrTrustMaterialSource {
    fn iroh_endpoint_url(&self) -> fedi_decentralized_domain::Url {
        fedi_decentralized_domain::Url(format!(
            "{}{}",
            fman_nostr::IROH_API_ENDPOINT_URL_SCHEME,
            self.iroh_endpoint_id
        ))
    }

    fn holder_authorizations(&self) -> Vec<fedi_decentralized_domain::HolderAuthorizationEnvelope> {
        self.nostr.holder_authorizations()
    }
}
