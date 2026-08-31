//! Process wiring: the shared context, the boot sequence, and the background
//! tasks.
//!
//! Startup is recovery-first — open storage, restore wallet operations and
//! allocation state, resume in-flight work, and only then accept fresh requests
//! and publish the advertisement. A live restore replaces the running
//! generation in place, which is why the serving surfaces read their runtime
//! through [`DaemonShell`] rather than holding it.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use fedi_decentralized_peer_badge_verifier::{PeerBadgeVerifier, PeerBadgeVerifierProvenance};
use fedi_decentralized_service_liquidity_manager::PeerBadgeTrustPolicy;
use fedi_decentralized_service_liquidity_manager::{
    ComponentHealth, GetHealthResponse, HealthComponent, HealthMode, HealthStatus,
    PUBLIC_LIQUIDITY_API_ALPN, PUBLIC_LIQUIDITY_PROTOCOL_VERSION as PROTOCOL_VERSION, SetupStatus,
    Timestamp,
};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{error, info, warn};

use crate::admin;
use crate::advertisement;
use crate::auth::{self, PublicAuthProvider};
use crate::config::{DaemonArgs, DaemonMode, DaemonPaths};
use crate::database::Database;
use crate::holder_authorization::{
    HolderAuthorizationFetcher, LastRelayRead, NostrHolderAuthorizationFetcher,
};
use crate::nostr::{self, RelayPublisher};
use crate::now_timestamp;
use crate::public;
use crate::recovery::{self, RecoveryCounts};
use crate::secret_store::SecretStore;
use crate::target_fedimint::TargetFedimintClients;
use crate::verification::{self, VerificationProvider};
use crate::{funds_admin, gateway_allocation, stability_allocation};
use crate::{setup_store, wallet};

/// Shared daemon state protected by a lock for concurrent readers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DaemonState {
    /// Current daemon lifecycle phase.
    pub phase: DaemonPhase,

    /// Whether startup recovery has completed for this process.
    pub recovery_complete: bool,

    /// Count-only summary from the latest startup recovery pass.
    pub last_recovery_counts: Option<RecoveryCounts>,

    /// Local Iroh node id for the public RPC endpoint, once the transport is bound.
    pub public_iroh_node_id: Option<String>,
}

/// Latest outcome of one periodic worker.
///
/// Worker errors are retried rather than fatal, which is right — a gatewayd
/// blip should not take the daemon down — but it also means a worker failing
/// every single pass is invisible outside the logs. Recording the outcome here
/// turns "restart it and see" into a health lookup.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WorkerHealth {
    /// When the worker last completed a pass.
    pub last_success_at: Option<Timestamp>,

    /// Message from the most recent failure, retained after later successes.
    pub last_error: Option<String>,

    /// When that failure happened.
    pub last_error_at: Option<Timestamp>,

    /// Failures since the last success. Zero after any successful pass.
    pub consecutive_failures: u32,
}

/// Consecutive failures after which a worker is reported unhealthy rather than
/// merely warning. Chosen so a single dependency blip does not page anyone,
/// while a worker that is genuinely stuck surfaces within a few of its periods.
const WORKER_UNHEALTHY_AFTER_FAILURES: u32 = 5;

/// Periodic-worker outcomes keyed by worker name.
/// A periodic background worker, as named in Admin API health.
///
/// The variants are declared in the alphabetical order of the names they
/// display as, because the derived `Ord` is what orders
/// [`WorkerHealthMap`], and that map's iteration order is the order workers
/// appear in the health detail an operator reads. Adding a variant out of order
/// would silently reorder that line.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, strum::Display)]
pub enum Worker {
    #[strum(serialize = "advertisement_publisher")]
    AdvertisementPublisher,
    #[strum(serialize = "gateway_allocation")]
    GatewayAllocation,
    #[strum(serialize = "gateway_observation")]
    GatewayObservation,
    #[strum(serialize = "holder_authorization_initial_read")]
    HolderAuthorizationInitialRead,
    #[strum(serialize = "stability_pool_allocation")]
    StabilityPoolAllocation,
    #[strum(serialize = "wallet_operation_sync")]
    WalletOperationSync,
}

pub(crate) type WorkerHealthMap = std::collections::BTreeMap<Worker, WorkerHealth>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum DaemonPhase {
    /// Startup is still initializing durable dependencies.
    #[default]
    Starting,

    /// Durable workflow recovery is running.
    Recovering,

    /// Durable dependencies and startup recovery are initialized.
    Ready,

    /// Shutdown has started.
    ShuttingDown,
}

/// Whether one runtime generation may create allocations or admit a live restore.
///
/// A generation starts [`AllocationAdmission::Open`]. A successful live-restore
/// admission makes the sole forward transition to
/// [`AllocationAdmission::ClosingForRestore`], which has no transition back to
/// `Open`: the generation is then torn down. `build_generation` constructs a
/// separate generation with a fresh `Open` state after the data-directory swap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AllocationAdmission {
    /// New allocation commits and one live-restore admission remain possible.
    Open,

    /// A restore is queued and this generation admits no allocations or restores.
    ClosingForRestore,
}

impl AllocationAdmission {
    /// Returns whether this generation may commit an allocation that creates authority.
    #[must_use]
    pub(crate) fn accepts_new_allocation(self) -> bool {
        self == Self::Open
    }

    /// Refuses a live restore after this generation has started closing.
    pub(crate) fn ensure_accepts_live_restore(
        self,
    ) -> fedi_decentralized_service_liquidity_manager::ServiceResult<()> {
        match self {
            Self::Open => Ok(()),
            Self::ClosingForRestore => Err(crate::unavailable(
                "the captured runtime generation is already closing for live restore",
            )),
        }
    }

    /// Closes this generation permanently for the one live restore it admits.
    ///
    /// The caller holds this generation's allocation-admission write guard. That
    /// serializes this transition with allocation commits and other restore
    /// handlers.
    pub(crate) fn close_for_live_restore(
        &mut self,
    ) -> fedi_decentralized_service_liquidity_manager::ServiceResult<()> {
        self.ensure_accepts_live_restore()?;
        *self = Self::ClosingForRestore;
        Ok(())
    }
}

/// Runtime context shared by daemon tasks.
#[derive(Clone)]
pub(crate) struct DaemonContext {
    /// Boot-only daemon arguments.
    pub(crate) args: DaemonArgs,

    /// Derived daemon data-dir layout.
    pub(crate) paths: DaemonPaths,

    /// Shared live daemon state.
    pub(crate) daemon_state: Arc<RwLock<DaemonState>>,

    /// SQLite database handle.
    pub(crate) database: Database,

    /// Local encrypted-at-rest secret store.
    pub(crate) secret_store: SecretStore,

    /// Public payload proof boundary. Read it through
    /// [`DaemonContext::auth_provider`] rather than directly, so a concurrent
    /// install cannot split one request across two keys.
    ///
    /// Swappable because the provider signing identity can be installed on a
    /// running daemon: a deployment that boots without a key holds the
    /// fail-closed `UnconfiguredAuthProvider` until one is installed, and must
    /// not need a restart to pick it up.
    pub(crate) auth_provider_slot: Arc<RwLock<Arc<dyn PublicAuthProvider>>>,

    /// Fires when a provider signing identity becomes available, so the public
    /// Iroh transport can bind with its derived key.
    pub(crate) identity_installed: Arc<tokio::sync::watch::Sender<bool>>,

    /// Private federation verification boundary.
    pub(crate) verification_provider: Arc<dyn VerificationProvider>,

    /// Shared PeerBadge verification boundary, injected now so trust handling
    /// cannot later be added as component-local logic.
    #[expect(
        dead_code,
        reason = "injected in Step 0 for later PeerBadge advertisement verification"
    )]
    pub(crate) peer_badge_verifier: PeerBadgeVerifier,

    /// Relay publication backend.
    pub(crate) relay_publisher: Arc<dyn RelayPublisher>,

    /// What the last Holder-authorization reconciliation concluded, so the
    /// operator console can tell "not read yet" from "read and found nothing"
    /// from "the relays are down". Runtime state: a restart genuinely has not
    /// read yet.
    pub(crate) holder_authorization_read: Arc<RwLock<LastRelayRead>>,

    /// Relay fetch backend for Holder-authorization reconciliation.
    ///
    /// Separate from `relay_publisher` because the two directions are separate
    /// boundaries: publication targets the configured relays with the provider
    /// key, while reconciliation reads them anonymously.
    pub(crate) holder_authorization_fetcher: Arc<dyn HolderAuthorizationFetcher>,

    /// Target federation Fedimint clients opened by allocation workers.
    pub(crate) target_fedimint_clients: TargetFedimintClients,

    /// Per-window rate bound on outbound trust-verification work per federation.
    ///
    /// Runtime-generation state rather than a database table: it rations work
    /// in flight, and a restart that forgets it costs one window's allowance,
    /// not correctness.
    #[cfg(test)]
    pub(crate) verification_budget: Arc<crate::verification_budget::VerificationBudget>,

    /// Latest outcome per periodic worker, surfaced through Admin API health.
    pub(crate) worker_health: Arc<RwLock<WorkerHealthMap>>,

    /// Per-generation allocation-admission state, serialized with live-restore validation.
    ///
    /// This starts [`AllocationAdmission::Open`] when this runtime is built. A
    /// request takes the read side only after external verification and holds it
    /// through an allocation commit; existing-allocation reads create no
    /// authority and need no guard. Live restore takes the write side, compares
    /// the staged archive with every committed allocation, then transitions once
    /// to [`AllocationAdmission::ClosingForRestore`] before teardown. The
    /// replacement runtime owns a distinct fresh state.
    pub(crate) allocation_admission: Arc<RwLock<AllocationAdmission>>,

    /// Barrier that holds periodic worker passes still while a backup copies.
    ///
    /// See [`WorkQuiescence`].
    pub(crate) work_quiescence: WorkQuiescence,

    /// Cooperative shutdown signal shared across daemon tasks.
    pub(crate) shutdown: CancellationToken,

    /// Daemon-owned tracker for short-lived background tasks.
    pub(crate) background_tasks: TaskTracker,
}

/// Holds periodic worker passes still so a backup can copy both stores at one
/// instant.
///
/// FLIP's durable state spans SQLite and the target-Fedimint client
/// directories. Reading two mutable stores without a shared snapshot or a
/// quiescence barrier may observe different instants, so an archive taken with
/// the workers running could hold SQLite from one moment and a client database
/// from another.
///
/// This is the barrier. Every periodic worker pass takes the read side for the
/// length of one pass; a backup takes the write side across its copy of both
/// stores. Tokio's `RwLock` is write-preferring, so once a backup queues, no
/// further pass starts, and the write guard is granted only when the passes
/// already running have finished.
///
/// **The unit is one pass, not one statement.** A pass is what touches both
/// stores together — it reads an item from SQLite, acts on a Fedimint client,
/// and writes the result back. Holding a SQLite lock would order the first and
/// last of those and say nothing about the middle.
///
/// **What this does not gate**, deliberately: Admin verbs and allocation
/// admission write SQLite but not the client directories, and the backup copies
/// SQLite with `VACUUM INTO`, which takes its own consistent read snapshot. So a
/// config change or an allocation commit during a backup lands either wholly
/// inside the snapshot or wholly outside it, and neither can tear a client
/// directory.
#[derive(Clone, Default)]
pub(crate) struct WorkQuiescence(Arc<RwLock<()>>);

impl WorkQuiescence {
    /// Marks one periodic worker pass in flight.
    pub(crate) async fn pass(&self) -> tokio::sync::RwLockReadGuard<'_, ()> {
        self.0.read().await
    }

    /// Holds every worker pass still until the returned guard is dropped.
    pub(crate) async fn quiesce(&self) -> tokio::sync::RwLockWriteGuard<'_, ()> {
        self.0.write().await
    }
}

impl DaemonContext {
    /// Current public payload proof boundary.
    ///
    /// Returns an owned handle rather than a guard so callers never hold the
    /// lock across signing or verification work.
    pub(crate) async fn auth_provider(&self) -> Arc<dyn PublicAuthProvider> {
        self.auth_provider_slot.read().await.clone()
    }

    /// Installs the provider signing identity and swaps in the signing
    /// auth provider, so a daemon that booted without a key becomes able to
    /// sign without restarting.
    pub(crate) async fn install_provider_signing_identity(
        &self,
        secret_key_hex: &str,
    ) -> fedi_decentralized_service_liquidity_manager::ServiceResult<(
        fedi_decentralized_service_liquidity_manager::Pubkey,
        bool,
    )> {
        let (identity, installed) = crate::identity::install_production_provider_identity(
            &self.database,
            &self.secret_store,
            secret_key_hex,
        )
        .await?;
        let provider_pubkey = identity.provider_pubkey.clone();
        let provider: Arc<dyn PublicAuthProvider> =
            Arc::new(auth::SchnorrAuthProvider::new(identity).map_err(crate::internal_error)?);
        *self.auth_provider_slot.write().await = provider;
        // Wakes the deferred public transport bind; `send_replace` rather than
        // `send` so a receiver-less daemon (tests, restore) does not error.
        self.identity_installed.send_replace(true);
        Ok((provider_pubkey, installed))
    }

    /// The public Iroh node id, once the transport has bound.
    ///
    /// This is the daemon's own advertised endpoint address for an Iroh
    /// endpoint, so config writes normalize to it rather than trusting operator
    /// input.
    pub(crate) async fn local_iroh_node_id(&self) -> Option<String> {
        self.daemon_state.read().await.public_iroh_node_id.clone()
    }

    /// Records a completed worker pass.
    pub(crate) async fn record_worker_success(&self, worker: Worker) {
        let mut workers = self.worker_health.write().await;
        let entry = workers.entry(worker).or_default();
        entry.consecutive_failures = 0;
        entry.last_success_at = Some(now_timestamp());
    }

    /// Records a failed worker pass. The message is retained after later
    /// successes so a recovered-but-flapping worker is still diagnosable.
    pub(crate) async fn record_worker_failure(&self, worker: Worker, error: String) {
        let mut workers = self.worker_health.write().await;
        let entry = workers.entry(worker).or_default();
        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        entry.last_error = Some(error);
        entry.last_error_at = Some(now_timestamp());
    }

    /// Summarizes periodic-worker outcomes as one health component.
    async fn worker_health_component(&self, observed_at: Timestamp) -> ComponentHealth {
        let workers = self.worker_health.read().await.clone();
        if workers.is_empty() {
            return ComponentHealth {
                component: HealthComponent::BackgroundWorkers,
                status: HealthStatus::Unknown,
                detail: Some("no worker has completed a pass yet".to_owned()),
                observed_at,
            };
        }

        let worst = workers
            .values()
            .map(|health| health.consecutive_failures)
            .max()
            .unwrap_or(0);
        let status = match worst {
            0 => HealthStatus::Healthy,
            failures if failures < WORKER_UNHEALTHY_AFTER_FAILURES => HealthStatus::Warning,
            _ => HealthStatus::Unhealthy,
        };
        let detail = workers
            .iter()
            .map(|(worker, health)| {
                format!(
                    "{worker}: consecutive_failures={}, last_success_at={}, last_error={}",
                    health.consecutive_failures,
                    health
                        .last_success_at
                        .map_or_else(|| "never".to_owned(), |at| at.0.to_string()),
                    health.last_error.as_deref().unwrap_or("none"),
                )
            })
            .collect::<Vec<_>>()
            .join("; ");

        ComponentHealth {
            component: HealthComponent::BackgroundWorkers,
            status,
            detail: Some(detail),
            observed_at,
        }
    }

    /// Builds the Admin API health response.
    pub(crate) async fn health_response(&self) -> GetHealthResponse {
        let observed_at = now_timestamp();
        let state = self.daemon_state.read().await.clone();
        let database_status = match self.database.ping().await {
            Ok(()) => ComponentHealth {
                component: HealthComponent::Database,
                status: HealthStatus::Healthy,
                detail: Some(format!("sqlite={}", self.database.path().display())),
                observed_at,
            },
            Err(error) => ComponentHealth {
                component: HealthComponent::Database,
                status: HealthStatus::Unhealthy,
                detail: Some(error.to_string()),
                observed_at,
            },
        };
        let dependency_components = self.dependency_health_components(observed_at).await;
        let auth_mode = self.auth_provider().await.mode();
        let verification_mode = self.verification_provider.mode();
        let mut components = vec![
            ComponentHealth {
                component: HealthComponent::Daemon,
                status: match state.phase {
                    DaemonPhase::Starting => HealthStatus::Warning,
                    DaemonPhase::Recovering => HealthStatus::Warning,
                    DaemonPhase::Ready => HealthStatus::Healthy,
                    DaemonPhase::ShuttingDown => HealthStatus::Warning,
                },
                detail: Some(daemon_health_detail(&state)),
                observed_at,
            },
            ComponentHealth {
                component: HealthComponent::AdminApi,
                status: HealthStatus::Healthy,
                detail: Some(format!("bind={}", self.args.admin_bind_address)),
                observed_at,
            },
            ComponentHealth {
                component: HealthComponent::PublicLiquidityApi,
                // The transport binds with a key derived from the provider
                // identity, so it stays parked until one is installed. Reporting
                // healthy while unbound would hide exactly the state an
                // operator needs to see on a fresh deployment.
                status: if state.public_iroh_node_id.is_none() {
                    HealthStatus::Warning
                } else if state.phase == DaemonPhase::Ready && state.recovery_complete {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Warning
                },
                detail: Some(format!(
                    "Iroh transport {}; bind={}, version={}, alpn={}, recovery_complete={}, auth_mode={} (signing_ready={}), verification_mode={} (inputs_available={}, fixtures={})",
                    match &state.public_iroh_node_id {
                        Some(node_id) => format!("bound as {node_id}"),
                        None => "awaiting a provider signing identity".to_owned(),
                    },
                    self.args.public_bind_address,
                    PROTOCOL_VERSION.0,
                    String::from_utf8_lossy(PUBLIC_LIQUIDITY_API_ALPN),
                    state.recovery_complete,
                    auth_mode.mode,
                    auth_mode.signing_ready,
                    verification_mode.mode,
                    verification_mode.inputs_available,
                    verification_mode.fixtures
                )),
                observed_at,
            },
            database_status,
        ];
        components.extend(dependency_components);
        components.push(self.worker_health_component(observed_at).await);
        components.push(advertisement::relay_health_component(self, observed_at).await);
        let overall_status = if components
            .iter()
            .any(|component| component.status == HealthStatus::Unhealthy)
        {
            HealthStatus::Unhealthy
        } else {
            HealthStatus::Healthy
        };

        GetHealthResponse {
            overall_status,
            mode: HealthMode::Normal,
            components,
            observed_at,
        }
    }

    async fn dependency_health_components(&self, observed_at: Timestamp) -> Vec<ComponentHealth> {
        let setup = setup_store::load_setup_state(&self.database).await.ok();
        let setup_status = setup.as_ref().map(|setup| setup.status);
        let latest_balance = wallet::latest_wallet_balance_observation(&self.database)
            .await
            .ok()
            .flatten();
        let wallet_detail = match latest_balance {
            Some(balance) => Some(format!(
                "latest_spendable={}, network={}, observed_at={}",
                balance.spendable.0, balance.network, balance.observed_at.0
            )),
            None => Some("no wallet balance observation yet".to_owned()),
        };
        let status = match setup_status {
            Some(SetupStatus::Ready) => HealthStatus::Healthy,
            Some(_) => HealthStatus::Warning,
            None => HealthStatus::Unknown,
        };

        vec![
            ComponentHealth {
                component: HealthComponent::Wallet,
                status,
                detail: wallet_detail,
                observed_at,
            },
            ComponentHealth {
                component: HealthComponent::Gateway,
                status,
                detail: Some(
                    "gatewayd dependency is validated through setup and wallet calls".to_owned(),
                ),
                observed_at,
            },
            ComponentHealth {
                component: HealthComponent::ChainObserver,
                status,
                detail: Some(
                    "chain observer dependency is validated through setup and sync calls"
                        .to_owned(),
                ),
                observed_at,
            },
        ]
    }
}

/// Process-lifetime daemon shell.
///
/// The shell owns what a restore must not disturb — the data-dir lock, the boot
/// arguments, and the Admin API listener — and holds the current runtime
/// generation behind a slot. Everything derived from the data dir lives in that
/// generation and is rebuilt wholesale when the data dir is replaced.
///
/// Splitting it this way is what lets a live restore be argued as equivalent to
/// a restart: the generation is dropped in full before any file moves, so no
/// state derived from the old data dir survives into the new one. What differs
/// from a process restart is that the lock is held continuously — strictly
/// stronger, since a stop/start leaves a window for another process to take it —
/// and the admin socket stays bound, so the operator keeps their connection.
#[derive(Clone)]
pub(crate) struct DaemonShell {
    /// Boot-only daemon arguments.
    pub args: DaemonArgs,

    /// Derived daemon data-dir layout.
    pub paths: DaemonPaths,

    /// The serving generation, absent only while one is being replaced.
    generation: Arc<std::sync::RwLock<Option<DaemonContext>>>,

    /// A validated restore waiting for its generation to stand down.
    pending_restore: Arc<std::sync::Mutex<Option<crate::backup::StagedRestore>>>,

    /// Why the most recent restore failed, retained until one succeeds.
    last_restore_error: Arc<std::sync::RwLock<Option<String>>>,

    /// Whether a live restore is in flight.
    ///
    /// Armed by [`Self::request_restore`] and cleared when the next generation
    /// installs, so it spans the whole swap. The pending slot cannot stand in
    /// for it: [`Self::take_pending_restore`] empties that slot *before* the
    /// data dir is replaced and the runtime rebuilt, which is most of the wait,
    /// and the rollback path rebuilds again after it.
    ///
    /// This is what separates "a restore is swapping the data dir" from "no
    /// generation is installed", and the two are different facts about the
    /// process. Both are reachable: the Admin API binds concurrently with the
    /// first generation build, so a starting daemon has no generation and no
    /// restore.
    restoring: Arc<std::sync::atomic::AtomicBool>,

    /// Process-wide shutdown. Each generation runs on a child token, so a
    /// restore can end one generation without ending the process.
    pub shutdown: CancellationToken,
}

impl DaemonShell {
    fn new(args: DaemonArgs, paths: DaemonPaths) -> Self {
        Self {
            args,
            paths,
            generation: Arc::new(std::sync::RwLock::new(None)),
            pending_restore: Arc::new(std::sync::Mutex::new(None)),
            last_restore_error: Arc::new(std::sync::RwLock::new(None)),
            restoring: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            shutdown: CancellationToken::new(),
        }
    }

    /// Builds a shell already serving `context`, for tests that drive the Admin
    /// API against a runtime they constructed themselves.
    ///
    /// The generation loop is not running, so a restore requested through this
    /// shell arms and stands the generation down but nothing rebuilds it.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_generation(context: DaemonContext) -> Self {
        let shell = Self::new(context.args.clone(), context.paths.clone());
        shell.install(context);
        shell
    }

    /// The serving generation, or `None` while a restore swaps the data dir.
    pub(crate) fn current(&self) -> Option<DaemonContext> {
        self.generation
            .read()
            .expect("daemon generation lock poisoned")
            .clone()
    }

    /// Whether a live restore is in flight right now.
    ///
    /// Read together with [`Self::current`]: with no generation installed, this
    /// is what tells a restore in progress from a daemon that has not built its
    /// first generation yet. It must not also return true whenever no
    /// generation is installed: its only call site is reached only when there is
    /// no generation, so that would make it vacuously true there, report every
    /// runtime-less process as restoring, and leave `HealthMode::NoRuntime`
    /// unreachable.
    pub(crate) fn is_reloading(&self) -> bool {
        self.restoring.load(std::sync::atomic::Ordering::Acquire)
    }

    pub(crate) fn last_restore_error(&self) -> Option<String> {
        self.last_restore_error
            .read()
            .expect("restore error lock poisoned")
            .clone()
    }

    /// Arms a validated restore and stands the current generation down.
    ///
    /// The swap itself happens on the generation loop once the generation has
    /// fully torn down, so the caller can return an HTTP response first.
    pub(crate) fn request_restore(
        &self,
        staged: crate::backup::StagedRestore,
        context: &DaemonContext,
        allocation_admission: &mut AllocationAdmission,
    ) -> fedi_decentralized_service_liquidity_manager::ServiceResult<()> {
        let mut pending = self
            .pending_restore
            .lock()
            .expect("pending restore lock poisoned");
        if pending.is_some() {
            return Err(crate::unavailable(
                "another live restore is already pending",
            ));
        }

        // Close allocation and restore admission before publishing the archive
        // to the process-global pending slot. Callers hold the write side of the
        // generation fence, so no allocation commit or second restore admission
        // can interleave with this transition.
        allocation_admission.close_for_live_restore()?;
        *pending = Some(staged);
        self.restoring
            .store(true, std::sync::atomic::Ordering::Release);
        context.shutdown.cancel();
        Ok(())
    }

    fn take_pending_restore(&self) -> Option<crate::backup::StagedRestore> {
        self.pending_restore
            .lock()
            .expect("pending restore lock poisoned")
            .take()
    }

    fn install(&self, context: DaemonContext) {
        *self
            .generation
            .write()
            .expect("daemon generation lock poisoned") = Some(context);
        *self
            .last_restore_error
            .write()
            .expect("restore error lock poisoned") = None;
        // A generation is serving again, whether it was built from the restored
        // state or from the state a failed restore was rolled back to. Either
        // way the operator is no longer waiting on a swap.
        self.restoring
            .store(false, std::sync::atomic::Ordering::Release);
    }

    pub(crate) fn uninstall(&self) {
        self.generation
            .write()
            .expect("daemon generation lock poisoned")
            .take();
    }

    fn record_restore_failure(&self, error: &anyhow::Error) {
        *self
            .last_restore_error
            .write()
            .expect("restore error lock poisoned") = Some(format!("{error:#}"));
    }
}

/// Start the FLIP daemon.
///
/// The mandatory verifier is retained for deferred complete advertisement
/// verification and is intentionally not invoked yet.
///
/// # Errors
///
/// Returns an error when configuration, durable state, dependency setup, or a
/// daemon listener cannot be initialized or run safely.
pub async fn run_daemon(
    args: DaemonArgs,
    peer_badge_verifier: PeerBadgeVerifier,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        args.mode == DaemonMode::Normal,
        "restore-mode configuration must use run_restore_daemon"
    );
    let selected_profile = args
        .manifold_environment
        .profile()
        .map_err(|err| anyhow::anyhow!("resolve Manifold environment profile: {err}"))?;
    let peer_badge_trust_policy =
        PeerBadgeTrustPolicy::try_new(selected_profile.minimum_peer_badge_trust_level())
            .map_err(|error| anyhow::anyhow!("invalid Manifold PeerBadge trust policy: {error}"))?;
    let expected_verifier_provenance = PeerBadgeVerifierProvenance::ManifoldProfile {
        environment: selected_profile.environment(),
        profile_revision: selected_profile.profile_revision(),
    };
    anyhow::ensure!(
        peer_badge_verifier.provenance() == expected_verifier_provenance,
        "PeerBadge verifier provenance does not match the selected Manifold environment: selected {} revision {}, verifier {:?}",
        selected_profile.environment(),
        selected_profile.profile_revision(),
        peer_badge_verifier.provenance(),
    );
    let paths = args.paths();
    info!(
        ?args,
        manifold_environment = %selected_profile.environment(),
        manifold_profile_revision = selected_profile.profile_revision(),
        "starting FLIP liquidity manager daemon"
    );

    tokio::fs::create_dir_all(&paths.data_dir)
        .await
        .with_context(|| format!("failed to create data dir {:?}", paths.data_dir))?;

    // Held for the whole process, across every generation. A restore therefore
    // never releases it, closing the window a stop/start restore leaves open.
    let _lock = DaemonLock::acquire(&paths.lock_file)?;
    let shell = DaemonShell::new(args, paths);

    supervise_tasks(
        vec![
            ("admin_api", tokio::spawn(admin::serve(shell.clone()))),
            (
                "runtime_generations",
                tokio::spawn(run_generations(
                    shell.clone(),
                    peer_badge_verifier,
                    peer_badge_trust_policy,
                )),
            ),
            (
                "shutdown_signal",
                tokio::spawn(wait_for_shutdown_signal(shell.shutdown.clone())),
            ),
        ],
        &shell.shutdown,
    )
    .await
}

/// Runs runtime generations until shutdown, rebuilding across live restores.
///
/// Each generation owns everything derived from the data dir and is torn down
/// completely before the data dir is replaced. A restored state that cannot be
/// started is rolled back to the previous state rather than left stranded.
async fn run_generations(
    shell: DaemonShell,
    peer_badge_verifier: PeerBadgeVerifier,
    peer_badge_trust_policy: PeerBadgeTrustPolicy,
) -> anyhow::Result<()> {
    let mut restored_from: Option<std::path::PathBuf> = None;

    loop {
        let context =
            match build_generation(&shell, peer_badge_verifier.clone(), peer_badge_trust_policy)
                .await
            {
                Ok(context) => context,
                Err(error) => {
                    let Some(aside_dir) = restored_from.take() else {
                        return Err(error);
                    };
                    // The restored state is unusable. Put back what it replaced and
                    // come up on that instead, so a bad archive costs availability
                    // rather than the deployment.
                    warn!(
                        error = format!("{error:#}"),
                        "restored state failed to start; rolling back to the previous state"
                    );
                    crate::backup::rollback_live_restore(&shell.paths, &aside_dir)
                        .context("rollback after a failed restore failed")?;
                    shell.record_restore_failure(&error);
                    continue;
                }
            };

        shell.install(context.clone());
        let result = serve_generation(&context).await;
        shell.uninstall();

        // Whether this generation is being replaced decides how hard its
        // teardown has to work, so it is resolved before tearing down.
        let staged = shell.take_pending_restore();
        teardown_generation(context, staged.is_some()).await;

        let Some(staged) = staged else {
            return result;
        };
        // A generation that ended for its own reasons is a failure even with a
        // restore queued behind it; the staged archive is discarded on drop.
        result?;

        info!("applying live restore to the data dir");
        let aside_dir = crate::backup::commit_live_restore(&shell.paths, &staged)?;
        // Naming the retained directory is the point: it is what a rollback
        // reads, and an operator who has to reach for it should not have to
        // guess where the daemon put the state it replaced.
        info!(
            retained_previous_state = %aside_dir.display(),
            "live restore committed; rebuilding the runtime generation"
        );
        restored_from = Some(aside_dir);
        drop(staged);
    }
}

/// Builds one runtime generation from the current contents of the data dir.
async fn build_generation(
    shell: &DaemonShell,
    peer_badge_verifier: PeerBadgeVerifier,
    peer_badge_trust_policy: PeerBadgeTrustPolicy,
) -> anyhow::Result<DaemonContext> {
    let args = shell.args.clone();
    let paths = shell.paths.clone();
    let target_fedimint_clients = TargetFedimintClients::new(
        args.max_open_target_clients,
        if args.allow_private_federation_endpoints {
            crate::endpoint_policy::EndpointPolicy::AllowPrivate
        } else {
            crate::endpoint_policy::EndpointPolicy::GlobalOnly
        },
    );

    tokio::fs::create_dir_all(&paths.federations_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create target federation storage dir {:?}",
                paths.federations_dir
            )
        })?;

    let secret_store =
        SecretStore::load_or_create(&paths.secret_store_key, args.secret_store_key.as_deref())
            .with_context(|| {
                format!(
                    "failed to initialize secret store at {:?}",
                    paths.secret_store_key
                )
            })?;
    let database = Database::connect(&paths.sqlite_path).await?;
    database.ping().await?;

    // Backstop for every config-persistence path (including restored
    // backups): a fixture-fed daemon must never come up against a mainnet
    // configuration. It runs per generation precisely so a live restore cannot
    // slip a mainnet config past the check that a restart would have applied.
    if args.trust_fixtures_dir.is_some() || args.allow_private_federation_endpoints {
        let stored = setup_store::load_setup_state(&database)
            .await
            .map_err(|error| anyhow::anyhow!("failed to load setup state: {error}"))?;
        if let Some(config) = &stored.config {
            setup_store::ensure_trust_fixtures_allow_network(
                args.trust_fixtures_dir.is_some(),
                config.network,
            )
            .map_err(|error| anyhow::anyhow!("refusing to start: {error}"))?;
            setup_store::ensure_private_endpoints_allow_network(
                args.allow_private_federation_endpoints,
                config.network,
            )
            .map_err(|error| anyhow::anyhow!("refusing to start: {error}"))?;
        }
    }

    let auth_provider = auth::provider_from_args(&database, &secret_store, &args).await?;
    // A deployment with no provider key yet boots fail-closed and waits for an
    // Admin API install rather than for an operator restart.
    let identity_installed = tokio::sync::watch::channel(auth_provider.mode().signing_ready).0;

    // A child of the process token, so SIGTERM ends the generation and the
    // process, while a restore ends only the generation.
    let shutdown = shell.shutdown.child_token();
    let background_tasks = TaskTracker::new();
    // The FMan advertisement lookup is always the real relay path now; only
    // the invite-code preview is fixture-substitutable.
    let (preview_provider, trust_inputs): (
        std::sync::Arc<dyn crate::federation_preview::FederationPreviewProvider>,
        verification::TrustInputs,
    ) = match &args.trust_fixtures_dir {
        Some(dir) => {
            warn!(
                fixtures_dir = %dir.display(),
                "TRUST FIXTURES ENABLED: the federation preview comes from local \
                 fixture files, not the real network; \
                 this is never a production trust configuration"
            );
            (
                std::sync::Arc::new(
                    crate::trust_fixtures::FixtureFederationPreviewProvider::new(dir.clone()),
                ),
                verification::TrustInputs::Fixtures,
            )
        }
        None => (
            std::sync::Arc::new(
                crate::federation_preview::FedimintFederationPreviewProvider::new(
                    if args.allow_private_federation_endpoints {
                        crate::endpoint_policy::EndpointPolicy::AllowPrivate
                    } else {
                        crate::endpoint_policy::EndpointPolicy::GlobalOnly
                    },
                )
                .await?,
            ),
            verification::TrustInputs::Production,
        ),
    };
    let verification_budget = Arc::new(crate::verification_budget::VerificationBudget::default());
    let verification_provider = std::sync::Arc::new(verification::VerificationPipeline::new(
        verification::VerificationDeps {
            database: database.clone(),
            revocation_fetcher: std::sync::Arc::new(crate::revocation::NostrRevocationFetcher),
            preview_provider,
            verification_budget: verification_budget.clone(),
        },
        trust_inputs,
        peer_badge_trust_policy,
    ));
    let context = DaemonContext {
        args,
        paths: paths.clone(),
        daemon_state: Arc::new(RwLock::new(DaemonState::default())),
        database,
        secret_store,
        auth_provider_slot: Arc::new(RwLock::new(auth_provider)),
        identity_installed: Arc::new(identity_installed),
        verification_provider,
        peer_badge_verifier,
        relay_publisher: nostr::nostr_relay_publisher(),
        holder_authorization_read: Arc::new(RwLock::new(LastRelayRead::NotYet)),
        holder_authorization_fetcher: Arc::new(NostrHolderAuthorizationFetcher),
        target_fedimint_clients,
        #[cfg(test)]
        verification_budget,
        worker_health: Arc::new(RwLock::new(WorkerHealthMap::new())),
        allocation_admission: Arc::new(RwLock::new(AllocationAdmission::Open)),
        work_quiescence: WorkQuiescence::default(),
        shutdown: shutdown.clone(),
        background_tasks: background_tasks.clone(),
    };

    {
        let mut state = context.daemon_state.write().await;
        state.phase = DaemonPhase::Recovering;
    }

    // The same startup recovery a process restart runs. A live restore re-enters
    // it here against the restored database rather than through a special path,
    // which is what keeps the two cases the same execution domain.
    let recovery_snapshot = recovery::run_startup_recovery(&context.database).await?;

    {
        let mut state = context.daemon_state.write().await;
        state.last_recovery_counts = Some(recovery_snapshot.counts());
        state.recovery_complete = true;
        state.phase = DaemonPhase::Ready;
    }

    // Reads the relay once, so a provider whose operator never opens the
    // authorization screen still enrols what a Holder already published.
    background_tasks.spawn(crate::holder_authorization::run_initial_read_task(
        context.clone(),
    ));
    // The advertisement publisher is deliberately not spawned here. It needs
    // the public endpoint identity as a readiness input, so `public::serve`
    // starts it once the Iroh bind has settled.
    background_tasks.spawn(funds_admin::run_operation_sync_task(context.clone()));
    background_tasks.spawn(gateway_allocation::run_gateway_observation_task(
        context.clone(),
    ));
    background_tasks.spawn(gateway_allocation::run_gateway_allocation_task(
        context.clone(),
    ));
    background_tasks.spawn(stability_allocation::run_stability_pool_allocation_task(
        context.clone(),
    ));
    // Track the health phase: shutdown may begin while the Admin API is still
    // draining and serving health requests.
    background_tasks.spawn({
        let context = context.clone();
        async move {
            context.shutdown.cancelled().await;
            context.daemon_state.write().await.phase = DaemonPhase::ShuttingDown;
        }
    });

    Ok(context)
}

/// Serves one generation's public transport until it is stood down.
async fn serve_generation(context: &DaemonContext) -> anyhow::Result<()> {
    supervise_tasks(
        vec![
            ("public_api", tokio::spawn(public::serve(context.clone()))),
            ("generation_shutdown", {
                let shutdown = context.shutdown.clone();
                tokio::spawn(async move {
                    shutdown.cancelled().await;
                    Ok(())
                })
            }),
        ],
        &context.shutdown,
    )
    .await
}

/// Drains a generation, optionally releasing every handle it holds on the data
/// dir.
///
/// Background workers always stop first. `releasing_data_dir` then decides
/// whether to go further, and it is not an optimization toggle:
///
/// - Replacing the generation (a live restore) *must* release the handles, in
///   order. Target federation clients give up their RocksDB locks, then the
///   SQLite pool closes — which is also what drains in-flight Admin API
///   requests, since those run on the shell's listener and are untracked here,
///   but any of them holding a connection keeps `close()` pending until its
///   transaction ends. When this returns nothing derived from the old data dir
///   is still running, which is the precondition for replacing it.
/// - Process shutdown must *not* pay for that. The kernel reclaims the locks
///   and SQLite recovers its WAL on the next open, while shutting a live
///   federation client down can take seconds — and the live harness kills the
///   daemon 10s after SIGTERM (`tests/common/live_liquidity/daemon.rs`). This
///   path therefore stays exactly as long as it was before restores existed.
async fn teardown_generation(context: DaemonContext, releasing_data_dir: bool) {
    context.shutdown.cancel();
    context.background_tasks.close();
    context.background_tasks.wait().await;

    if !releasing_data_dir {
        return;
    }

    let evicted = context.target_fedimint_clients.shutdown_all().await;
    if !evicted.is_empty() {
        info!(
            federations = evicted.len(),
            "closed target federation clients for runtime teardown"
        );
    }
    context.database.close().await;
}

/// Start the isolated restore-only FLIP daemon without trust verification.
///
/// Restore mode exposes health plus authenticated backup inspection and
/// restore; it never constructs or retains a PeerBadge verifier.
///
/// # Errors
///
/// Returns an error when normal-mode arguments are supplied or the restore
/// data directory, process lock, listener, or supervised task fails.
pub async fn run_restore_daemon(args: DaemonArgs) -> anyhow::Result<()> {
    anyhow::ensure!(
        args.mode == DaemonMode::Restore,
        "run_restore_daemon requires restore-mode configuration"
    );
    let paths = args.paths();
    info!(?args, "starting FLIP restore-only daemon");
    tokio::fs::create_dir_all(&paths.data_dir)
        .await
        .with_context(|| format!("failed to create restore data dir {:?}", paths.data_dir))?;

    let _lock = DaemonLock::acquire(&paths.lock_file)?;
    let shutdown = CancellationToken::new();
    let context = admin::RestoreAdminContext {
        args,
        paths,
        shutdown: shutdown.clone(),
        restore_target: crate::backup::RestoreTarget::default(),
    };

    supervise_tasks(
        vec![
            (
                "restore_admin_api",
                tokio::spawn(admin::serve_restore(context)),
            ),
            (
                "shutdown_signal",
                tokio::spawn(wait_for_shutdown_signal(shutdown.clone())),
            ),
        ],
        &shutdown,
    )
    .await
}

/// Supervises named daemon tasks: waits for the first task to exit, cancels
/// the shared shutdown token, then drains the remaining tasks. The first
/// error encountered wins.
async fn supervise_tasks(
    mut tasks: Vec<(&'static str, JoinHandle<anyhow::Result<()>>)>,
    shutdown: &CancellationToken,
) -> anyhow::Result<()> {
    let (first_index, first_result) = std::future::poll_fn(|cx| {
        for (index, (_, handle)) in tasks.iter_mut().enumerate() {
            if let std::task::Poll::Ready(result) = std::pin::Pin::new(handle).poll(cx) {
                return std::task::Poll::Ready((index, result));
            }
        }
        std::task::Poll::Pending
    })
    .await;
    let mut result = handle_task_result(tasks[first_index].0, first_result);

    shutdown.cancel();

    for (index, (name, handle)) in tasks.into_iter().enumerate() {
        if index == first_index {
            continue;
        }
        let task_result = handle_task_result(name, handle.await);
        if result.is_ok() {
            result = task_result;
        }
    }

    result
}

fn handle_task_result(
    task_name: &str,
    result: Result<anyhow::Result<()>, tokio::task::JoinError>,
) -> anyhow::Result<()> {
    match result {
        Ok(Ok(())) => {
            info!(task_name, "daemon task exited");
            Ok(())
        }
        Ok(Err(error)) => {
            error!(task_name, ?error, "daemon task failed");
            Err(error)
        }
        Err(error) if error.is_cancelled() => {
            info!(task_name, "daemon task was cancelled");
            Ok(())
        }
        Err(error) => {
            error!(task_name, ?error, "daemon task panicked");
            bail!("daemon task {task_name} failed")
        }
    }
}

async fn wait_for_shutdown_signal(shutdown: CancellationToken) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .context("failed to install SIGTERM handler")?;
        tokio::select! {
            _ = shutdown.cancelled() => {}
            result = tokio::signal::ctrl_c() => {
                result.context("failed to listen for Ctrl-C")?;
                info!("received Ctrl-C");
                shutdown.cancel();
            }
            _ = terminate.recv() => {
                info!("received SIGTERM");
                shutdown.cancel();
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = shutdown.cancelled() => {}
            result = tokio::signal::ctrl_c() => {
                result.context("failed to listen for Ctrl-C")?;
                info!("received Ctrl-C");
                shutdown.cancel();
            }
        }
    }

    Ok(())
}

fn daemon_health_detail(state: &DaemonState) -> String {
    match state.last_recovery_counts {
        Some(counts) => format!(
            "phase={:?}, recovery_complete={}, active_allocation_items={}, active_wallet_operations={}",
            state.phase,
            state.recovery_complete,
            counts.active_allocation_item_count,
            counts.active_wallet_operation_count,
        ),
        None => format!(
            "phase={:?}, recovery_complete={}",
            state.phase, state.recovery_complete
        ),
    }
}

/// Single-daemon guard backed by an OS advisory file lock, so the kernel
/// releases it on any process exit (including SIGKILL/OOM) and a crashed
/// daemon never blocks its own restart. The file is kept around; only the
/// lock matters.
struct DaemonLock {
    _file: std::fs::File,
}

impl DaemonLock {
    fn acquire(path: &Path) -> anyhow::Result<Self> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("failed to open FLIP daemon lock at {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                bail!(
                    "another FLIP daemon already holds the lock at {}",
                    path.display()
                );
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(error).with_context(|| {
                    format!("failed to acquire FLIP daemon lock at {}", path.display())
                });
            }
        }
        file.set_len(0)
            .and_then(|()| writeln!(file, "pid={}", std::process::id()))
            .with_context(|| format!("failed to write FLIP daemon lock at {}", path.display()))?;

        Ok(Self { _file: file })
    }
}

#[cfg(test)]
#[path = "../tests/daemon.rs"]
mod tests;
