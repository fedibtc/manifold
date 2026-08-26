//! One guardian seat's runtime: a cheap command-and-snapshot handle and
//! the single task that owns its lifecycle (the durable vocabulary it acts
//! on lives in [`crate::facts`]).
//!
//! `SeatLoop` carries everything one active seat's verbs need — the durable
//! store, process configuration, derived keys, child process, and mutable state
//! — so a verb never reaches back into the fleet. It executes lifecycle
//! commands and periodic health checks one at a time, which makes partial
//! lifecycle observations and interleaved child mutations structurally
//! impossible. `Seat` sends commands and reads the shared watch value. A
//! decommissioned seat has no loop.
//!
//! Setup state comes from the in-memory driven-DKG session and event stream;
//! FMan never probes a setup network API. Once configured, only the periodic
//! watchdog uses [`FedimintApi::probe`]; requests read its published health.
//! The immutable formed row and final data directory make missing guardian
//! data detectable after every crash window.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use fedi_decentralized_domain::{
    FMAN_SEAT_BINDINGS_META_FIELD_KEY, FederationSeat, FederationSeats,
    FmanPeerAttestationStatement, FmanSeatBindings, PeerId, Pubkey, SeatEndpointProof,
    VerifiedSeatBinding, federation_seats, parse_protocol_peer_id, protocol_peer_id,
};
use fedi_decentralized_service_fleet_manager::{
    FEDERATION_METADATA_OBJECT_MAX_BYTES, FederationName, FiId, FormationSeatBinding, GuardianCode,
    InviteCode, MetaConsensusBase, MetaFieldKey, MetaFieldValue, Plan, SeatHealth, SeatId,
    ServiceStatus,
};
use fedimint_core::base32::{self, FEDIMINT_PREFIX};
use fedimint_core::config::ClientConfig;
use fedimint_core::core::{ModuleInstanceId, ModuleKind};
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::setup_code::{PeerEndpoints, PeerSetupCode};
use fedimint_core::util::SafeUrl;
use fedimint_meta_common::{DEFAULT_META_KEY, MetaValue};
use fedimint_server::config::driven::{ChildState, DrivenDkgClient, DrivenDkgEvent, RunDkgParams};
use fman_meta_fields::{MetaFieldError, validate_meta_field};
use stability_pool_client::common::Account;
use tokio::sync::{mpsc, oneshot, watch};

use crate::backup_worker::BackupWorker;
use crate::db::Db;
use crate::facts::{CompletionCallbackStatus, DkgCodeSet, SeatFacts, SeatPorts};
use crate::fedimint_api::{FedimintApi, FedimintApiError};
use crate::guardian_fee::{AccountId, FeePolicy};
use crate::identity::SeatKeys;
use crate::push_callback::{CompletionHookWake, ValidatedDkgCompletionCallback};
use crate::seat_process::{
    DKG_START_TIMEOUT, ObservedSeatExit, RespawnPolicy, SeatProcess, SeatProcessConfig,
    SeatProcessError, SeatProcessSpawner, effective_iroh_api_key, effective_iroh_p2p_key,
    seat_data_dir,
};

/// Content digest of one candidate whole-object metadata target.
///
/// Deliberately not a [`MetaConsensusBase`]: a candidate submission has no
/// consensus revision until adopted, so its admission identity is content
/// alone — exact replay under the same base must keep matching.
type MetaTargetDigest = [u8; 32];

#[derive(Debug, thiserror::Error)]
pub enum SeatVerbError {
    /// Unknown seat id — or a known one queried by the wrong FI:
    /// seat ids are unguessable, so refusing to distinguish the two
    /// keeps them recipient-binding without an information oracle.
    #[error("unknown seat")]
    UnknownSeat,
    /// The seat's fedimintd is not serving; retryable.
    #[error("seat unavailable")]
    SeatUnavailable,
    /// Formation and cancellation are unavailable after the seat is formed.
    #[error("the seat's federation is running")]
    FederationIsRunning,
    #[error("wrong state: {status:?}")]
    WrongState { status: ServiceStatus },
    #[error("invalid DKG input: {0}")]
    InvalidDkgInput(String),
    /// `SetMetaField` named a key with no compiled validator.
    #[error("meta key refused")]
    MetaKeyRefused,
    /// `SetMetaField` carried a value its key's validator rejected.
    #[error("meta value invalid")]
    MetaValueInvalid,
    /// A metadata mutation was based on an older consensus metadata object.
    #[error("meta consensus changed")]
    MetaConsensusChanged,
    /// The formation directory is already consensus and cannot be replaced.
    #[error("formation metadata already published")]
    FormationMetaAlreadyPublished,
    /// A metadata mutation named an exact base this process already admitted
    /// for a *different* whole-object target. Unlike a stale base, rereading
    /// cannot clear it: the base stays pinned to its first admitted target
    /// until consensus moves or this process restarts. Exact replay of the
    /// admitted target remains permitted.
    #[error("meta target conflict")]
    MetaTargetConflict,
    #[error(transparent)]
    Internal(anyhow::Error),
}

impl SeatVerbError {
    fn internal(err: impl Into<anyhow::Error>) -> Self {
        SeatVerbError::Internal(err.into())
    }

    /// Maps a fedimintd client failure for verbs that need the child serving.
    fn child_needed(err: FedimintApiError) -> Self {
        match err {
            FedimintApiError::Unreachable(_) => SeatVerbError::SeatUnavailable,
            other => SeatVerbError::internal(other),
        }
    }
}

fn seat_verb_error_kind(error: &SeatVerbError) -> &'static str {
    match error {
        SeatVerbError::UnknownSeat => "unknown_seat",
        SeatVerbError::SeatUnavailable => "seat_unavailable",
        SeatVerbError::FederationIsRunning => "federation_is_running",
        SeatVerbError::WrongState { .. } => "wrong_state",
        SeatVerbError::InvalidDkgInput(_) => "invalid_dkg_input",
        SeatVerbError::MetaKeyRefused => "meta_key_refused",
        SeatVerbError::MetaValueInvalid => "meta_value_invalid",
        SeatVerbError::MetaConsensusChanged => "meta_consensus_changed",
        SeatVerbError::FormationMetaAlreadyPublished => "formation_meta_already_published",
        SeatVerbError::MetaTargetConflict => "meta_target_conflict",
        SeatVerbError::Internal(_) => "internal",
    }
}

const HEALTHY_WATCHDOG_INTERVAL: Duration = Duration::from_secs(60);
const UNHEALTHY_WATCHDOG_INTERVAL: Duration = Duration::from_secs(5);
const WATCHDOG_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Operator-facing durable facts of one seat (admin `seats list`).
#[derive(Clone, Debug)]
pub struct SeatSummary {
    pub seat_id: SeatId,
    pub fi_id: FiId,
    pub plan: Plan,
    pub created_at_ms: i64,
    /// Claim state for the accepted payment, including terminal rejection.
    pub payment_claim: PaymentClaimStatus,
    /// Operator-decommissioned (terminal).
    pub decommissioned: bool,
    /// Sanitized callback delivery state; never includes bearer material.
    pub completion_callback: CompletionCallbackStatus,
    /// Last relay-confirmed publication of this seat's recovery document.
    /// `None` on an install with a relay configured means the relay holds
    /// nothing current for this seat — the absence an operator needs to see,
    /// not infer.
    pub backup: Option<SeatBackupStatus>,
}

/// What the semi-trusted relay demonstrably serves for one seat
/// (`seat_backup_publications`): the worker records a publication only after
/// reading the event back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeatBackupStatus {
    pub published_at_ms: i64,
    /// The published document names a guardian archive whose events are
    /// confirmed on the relay: a formed seat showing `false` here is backed
    /// up in name only.
    pub archive_confirmed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaymentClaimStatus {
    NotPaid,
    Pending,
    Success { at_ms: i64 },
    AlreadySpent { at_ms: i64 },
}

/// What a seat knows about itself when asked for status. `GetStatus`'s
/// wire shape is projected from this at the RPC boundary. Health exists only
/// for a created seat — before creation no child exists to be healthy
/// — so the shape carries it only there.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SeatReport {
    /// Operator-decommissioned (terminal).
    Decommissioned { at_ms: i64 },
    /// Live seat: the derived ceremony phase plus the child's health.
    Active {
        phase: SeatPhase,
        health: SeatHealth,
    },
}

/// A created seat's ceremony phase derived from runtime and durable evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SeatPhase {
    /// fedimintd holds no local params yet.
    Created,
    /// DKG was started and fedimintd is inside its API gap.
    DkgInProgress,
    /// A formed record exists but the final data directory is absent.
    DataLoss { invite_code: InviteCode },
    /// Consensus is serving.
    Running { invite_code: InviteCode },
}

/// One registry entry. `Arc` clones are cheap handles; an active seat loop
/// owns lifecycle publication and process state. Once a command enters its
/// queue, dropping the caller cannot cancel the operation.
pub(crate) struct Seat {
    facts: Arc<SeatFacts>,
    #[allow(dead_code)] // exposed to process-probing tests
    ports: SeatPorts,
    commands: mpsc::Sender<SeatCommand>,
    data_dir: PathBuf,
    state: watch::Sender<SeatRuntimeFacts>,
}

/// The sole publisher of an active seat's lifecycle and owner of its
/// child process.
struct SeatLoop {
    facts: Arc<SeatFacts>,
    db: Db,
    process: SeatProcessConfig,
    data_dir: PathBuf,
    policy: RespawnPolicy,
    keys: SeatKeys,
    /// The public key this install signs peer attestations with. The
    /// seat-binding validator requires the directory's entry for this
    /// guardian's own seat to carry exactly this key.
    own_fman_pubkey: Pubkey,
    ports: SeatPorts,
    fedimint_api: FedimintApi,
    state: watch::Sender<SeatRuntimeFacts>,
    /// Where this seat's recovery-document publication is hinted. The loop
    /// marks; nothing here ever waits on, or reads back from, a relay.
    backup: Arc<BackupWorker>,
    /// The one whole-object target admitted for the live consensus
    /// occurrence. The guarded verbs consult it only with the freshly read
    /// current base, and a superseded occurrence can never become current
    /// again (bases are bound to the module's monotone revision), so a pin
    /// for any other base is unreachable by construction and is simply
    /// replaced on the next admission — no history, cap, or eviction policy.
    /// A restart clears the pin together with every delayed handler it
    /// fences.
    meta_admission: Option<(MetaConsensusBase, MetaTargetDigest)>,
    completion_hooks: CompletionHookWake,
    process_spawner: SeatProcessSpawner,
    child: ProcessSlot,
    child_started: Option<tokio::time::Instant>,
    respawn_at: Option<tokio::time::Instant>,
    watchdog_at: Option<tokio::time::Instant>,
    backoff: Duration,
}

enum ProcessSlot {
    Empty,
    Parked {
        child: SeatProcess,
        client: DrivenDkgClient<tokio::net::UnixStream>,
    },
    DkgAcknowledged {
        child: SeatProcess,
        client: DrivenDkgClient<tokio::net::UnixStream>,
    },
    Configured {
        child: SeatProcess,
        client: Option<DrivenDkgClient<tokio::net::UnixStream>>,
    },
    /// A consuming stop failed, so exit was not proved and replacement is
    /// permanently refused for this daemon lifetime.
    ExitUnproven,
}

#[derive(Clone, Copy)]
enum ProcessSlotPhase {
    Empty,
    Parked,
    DkgAcknowledged,
    Configured,
    ExitUnproven,
}

enum ProcessObservation {
    Driven(Option<anyhow::Result<DrivenDkgEvent>>),
    Exited(Result<ObservedSeatExit, SeatProcessError>),
}

impl ProcessSlot {
    fn phase(&self) -> ProcessSlotPhase {
        match self {
            Self::Empty => ProcessSlotPhase::Empty,
            Self::Parked { .. } => ProcessSlotPhase::Parked,
            Self::DkgAcknowledged { .. } => ProcessSlotPhase::DkgAcknowledged,
            Self::Configured { .. } => ProcessSlotPhase::Configured,
            Self::ExitUnproven => ProcessSlotPhase::ExitUnproven,
        }
    }

    async fn observe(&mut self) -> ProcessObservation {
        match self {
            Self::Parked { child, client, .. } | Self::DkgAcknowledged { child, client, .. } => {
                tokio::select! {
                    biased;
                    event = client.next_event() => ProcessObservation::Driven(event),
                    exit = child.wait() => ProcessObservation::Exited(exit),
                }
            }
            Self::Configured {
                child,
                client: Some(client),
            } => tokio::select! {
                biased;
                event = client.next_event() => ProcessObservation::Driven(event),
                exit = child.wait() => ProcessObservation::Exited(exit),
            },
            Self::Configured {
                child,
                client: None,
            } => ProcessObservation::Exited(child.wait().await),
            Self::Empty | Self::ExitUnproven => unreachable!("live child guard"),
        }
    }

    fn client(&self) -> Option<&DrivenDkgClient<tokio::net::UnixStream>> {
        match self {
            Self::Parked { client, .. } | Self::DkgAcknowledged { client, .. } => Some(client),
            Self::Configured { client, .. } => client.as_ref(),
            Self::Empty | Self::ExitUnproven => None,
        }
    }

    fn client_mut(&mut self) -> Option<&mut DrivenDkgClient<tokio::net::UnixStream>> {
        match self {
            Self::Parked { client, .. } | Self::DkgAcknowledged { client, .. } => Some(client),
            Self::Configured { client, .. } => client.as_mut(),
            Self::Empty | Self::ExitUnproven => None,
        }
    }

    fn is_live(&self) -> bool {
        matches!(
            self,
            Self::Parked { .. } | Self::DkgAcknowledged { .. } | Self::Configured { .. }
        )
    }
}

/// Durable inputs reconstructed before starting one seat runtime.
pub(crate) struct SeatDurableState {
    /// Immutable seat facts.
    pub(crate) facts: SeatFacts,
    /// The immutable formed federation, which permanently closes restart.
    pub(crate) formed_invite: Option<InviteCode>,
    /// Terminal decommission timestamp.
    pub(crate) decommissioned_at_ms: Option<i64>,
}

/// Runtime capabilities and process configuration owned by one seat.
pub(crate) struct SeatRuntimeDependencies {
    /// Fleet database handle.
    pub(crate) db: Db,
    /// Supervised child-process configuration.
    pub(crate) process: SeatProcessConfig,
    /// Child restart policy.
    pub(crate) policy: RespawnPolicy,
    /// Root-derived per-seat authorities.
    pub(crate) keys: SeatKeys,
    /// This FMan's public service identity.
    pub(crate) own_fman_pubkey: Pubkey,
    /// Non-reused child port allocation.
    pub(crate) ports: SeatPorts,
    /// Native Fedimint connector registry shared by all local guardian APIs.
    pub(crate) fedimint_connectors: fedimint_connectors::ConnectorRegistry,
    /// Durable encrypted-backup publisher.
    pub(crate) backup: Arc<BackupWorker>,
    /// Fleet-wide durable completion-hook reconciler.
    pub(crate) completion_hooks: CompletionHookWake,
    pub(crate) process_spawner: SeatProcessSpawner,
}

#[derive(Clone)]
struct SeatRuntimeFacts {
    formed_invite: Option<InviteCode>,
    process_slot: ProcessSlotPhase,
    decommissioned_at_ms: Option<i64>,
    health: SeatHealth,
}

enum SeatCommand {
    #[cfg(test)]
    WatchdogTick {
        reply: oneshot::Sender<()>,
    },
    DkgCode {
        federation_name: Option<FederationName>,
        reply: oneshot::Sender<Result<GuardianCode, SeatVerbError>>,
    },
    StartDkg {
        codes: Vec<GuardianCode>,
        completion_callback: Option<ValidatedDkgCompletionCallback>,
        reply: oneshot::Sender<Result<(), SeatVerbError>>,
    },
    RestartDkg {
        codes: Vec<GuardianCode>,
        reply: oneshot::Sender<Result<ServiceStatus, SeatVerbError>>,
    },
    InviteCode {
        reply: oneshot::Sender<Result<InviteCode, SeatVerbError>>,
    },
    FederationBinding {
        reply: oneshot::Sender<Result<SeatFederationBinding, SeatVerbError>>,
    },
    SignEndpointProof {
        statement: FmanPeerAttestationStatement,
        reply: oneshot::Sender<Result<SeatEndpointProof, SeatVerbError>>,
    },
    ProposeFormationMeta {
        expected_base: MetaConsensusBase,
        seat_bindings: Vec<FormationSeatBinding>,
        fi_fee_account: Account,
        send_ppm: u64,
        min_send_ppm: u64,
        guardian_verification_fee_account: Account,
        reply: oneshot::Sender<Result<(), SeatVerbError>>,
    },
    SubmitMetaField {
        expected_base: MetaConsensusBase,
        key: MetaFieldKey,
        value: MetaFieldValue,
        min_send_ppm: u64,
        guardian_verification_fee_account: Option<Account>,
        reply: oneshot::Sender<Result<(), SeatVerbError>>,
    },
    RegisterGateway {
        gateway_api: SafeUrl,
        reply: oneshot::Sender<Result<bool, SeatVerbError>>,
    },
    GuardianFeePolicy {
        our_account_id: AccountId,
        reply: oneshot::Sender<Result<FeePolicy, SeatVerbError>>,
    },
    Decommission {
        reply: oneshot::Sender<anyhow::Result<bool>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// What one running seat can prove about the federation it is a guardian of.
///
/// Both halves come from the seat's own live `fedimintd`: `federation` is
/// derived from the final client config, and `seat` is the entry in it for
/// the peer this guardian actually is. Together they are exactly the material
/// an [`FmanPeerAttestation`](fedi_decentralized_domain::FmanPeerAttestation)
/// binds, which is why the two travel as one value — deriving them from
/// different reads would let an attestation name a seat from another config.
#[derive(Clone, Debug)]
pub struct SeatFederationBinding {
    /// Durable seat id used to derive this seat's fee account.
    pub seat_id: SeatId,

    /// Authoritative facts derived from the seat's final client config.
    pub federation: FederationSeats,

    /// This guardian's own entry in that config's peer set.
    pub seat: FederationSeat,
}

impl Seat {
    /// Bring a durably created seat to life: install the runtime mirror and
    /// spawn the supervised fedimintd (unless decommissioned). Shared by the
    /// startup rebuild and fresh creation. Registry publication uses an exclusive map
    /// entry so concurrent durable replays can start at most one runtime.
    pub(crate) fn start(durable: SeatDurableState, runtime: SeatRuntimeDependencies) -> Arc<Self> {
        let SeatDurableState {
            facts,
            formed_invite,
            decommissioned_at_ms,
        } = durable;
        let SeatRuntimeDependencies {
            db,
            process,
            policy,
            keys,
            own_fman_pubkey,
            ports,
            fedimint_connectors,
            backup,
            completion_hooks,
            process_spawner,
        } = runtime;
        let facts = Arc::new(facts);
        let data_dir = seat_data_dir(&process, facts.seat_no);
        let (state, _state_rx) = watch::channel(SeatRuntimeFacts {
            formed_invite,
            process_slot: ProcessSlotPhase::Empty,
            decommissioned_at_ms,
            health: SeatHealth::Unavailable,
        });
        // Keep only the next lifecycle command queued behind the one in progress.
        let (commands, command_rx) = mpsc::channel(1);

        if decommissioned_at_ms.is_none() {
            let fedimint_api = FedimintApi::new(fedimint_connectors, ports.api(), &keys.api_auth);
            let seat_loop = SeatLoop {
                facts: facts.clone(),
                db,
                process,
                data_dir: data_dir.clone(),
                policy,
                keys,
                own_fman_pubkey,
                ports,
                fedimint_api,
                state: state.clone(),
                backup,
                meta_admission: None,
                completion_hooks,
                process_spawner,
                child: ProcessSlot::Empty,
                child_started: None,
                respawn_at: None,
                watchdog_at: None,
                backoff: policy.initial_backoff,
            };
            tokio::spawn(async move {
                seat_loop.run(command_rx).await;
            });
        } else {
            // Closing the receiver makes every lifecycle request fail without
            // keeping an otherwise inert task alive.
            drop(command_rx);
        }

        Arc::new(Self {
            facts,
            ports,
            commands,
            data_dir,
            state,
        })
    }

    pub(crate) fn facts(&self) -> &SeatFacts {
        &self.facts
    }

    /// Live seats consume capacity; the fleet's allocation counting and
    /// operator listing read this.
    pub(crate) fn is_decommissioned(&self) -> bool {
        self.state.borrow().decommissioned_at_ms.is_some()
    }

    /// A decommissioned seat refuses mutation. Used by RPC callers as a fast
    /// precheck; the closed command channel is the terminal backstop.
    pub(crate) fn reject_decommissioned(&self) -> Result<(), SeatVerbError> {
        self.state.borrow().reject_decommissioned()
    }

    #[cfg(test)]
    pub(crate) fn ports(&self) -> SeatPorts {
        self.ports
    }

    #[cfg(test)]
    pub(crate) fn cached_report_for_test(&self) -> SeatReport {
        self.state
            .borrow()
            .report(self.data_dir.try_exists().unwrap())
    }

    #[cfg(test)]
    pub(crate) async fn watchdog_tick_for_test(&self) {
        self.request(|reply| SeatCommand::WatchdogTick { reply })
            .await
            .unwrap();
    }

    /// Loopback Prometheus port of this seat's child.
    ///
    /// Kept as a narrow scalar rather than exposing all child ports to the
    /// telemetry adapter. The adapter still checks the seat is Running before
    /// returning it to a loopback-only HTTP client.
    pub(crate) fn metrics_port(&self) -> u16 {
        self.ports.metrics()
    }

    async fn request<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> SeatCommand,
    ) -> anyhow::Result<T> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| anyhow!("seat command loop stopped"))?;
        response
            .await
            .map_err(|_| anyhow!("seat command loop stopped before replying"))
    }

    async fn verb_request<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, SeatVerbError>>) -> SeatCommand,
    ) -> Result<T, SeatVerbError> {
        match self.request(build).await {
            Ok(result) => result,
            Err(err) => {
                self.commands.closed().await;
                self.state.borrow().reject_decommissioned()?;
                Err(SeatVerbError::internal(anyhow!(
                    "seat command loop closed before replying: {err}"
                )))
            }
        }
    }

    /// Stop the supervised child if one is running (daemon shutdown).
    pub(crate) async fn stop(&self) {
        if self.is_decommissioned() {
            // A concurrent decommission publishes its durable marker before
            // its loop exits; do not race daemon shutdown past the loop.
            self.commands.closed().await;
            return;
        }
        let _ = self.request(|reply| SeatCommand::Shutdown { reply }).await;
        self.commands.closed().await;
    }

    pub(crate) async fn dkg_code(
        &self,
        federation_name: Option<&FederationName>,
    ) -> Result<GuardianCode, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::DkgCode {
            federation_name: federation_name.cloned(),
            reply,
        })
        .await
    }

    pub(crate) async fn start_dkg(
        &self,
        codes: &[GuardianCode],
        completion_callback: Option<ValidatedDkgCompletionCallback>,
    ) -> Result<(), SeatVerbError> {
        self.verb_request(|reply| SeatCommand::StartDkg {
            codes: codes.to_vec(),
            completion_callback,
            reply,
        })
        .await
    }

    pub(crate) async fn restart_dkg(
        &self,
        codes: &[GuardianCode],
    ) -> Result<ServiceStatus, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::RestartDkg {
            codes: codes.to_vec(),
            reply,
        })
        .await
    }

    pub(crate) async fn report(&self) -> Result<SeatReport, SeatVerbError> {
        let final_data_exists = self.data_dir.try_exists().map_err(|error| {
            SeatVerbError::internal(anyhow!("inspect final seat data dir: {error}"))
        })?;
        Ok(self.state.borrow().report(final_data_exists))
    }

    /// Operator summary: durable facts plus the decommission flag.
    pub(crate) fn summary(
        &self,
        payment_claim: PaymentClaimStatus,
        completion_callback: CompletionCallbackStatus,
        backup: Option<SeatBackupStatus>,
    ) -> SeatSummary {
        SeatSummary {
            seat_id: self.facts.seat_id.clone(),
            fi_id: self.facts.fi_id,
            plan: self.facts.plan.clone(),
            created_at_ms: self.facts.created_at_ms,
            payment_claim,
            decommissioned: self.is_decommissioned(),
            completion_callback,
            backup,
        }
    }

    pub(crate) async fn decommission(&self) -> anyhow::Result<bool> {
        if self.is_decommissioned() {
            // A concurrent first call may still be stopping the child.
            self.commands.closed().await;
            return Ok(false);
        }
        match self
            .request(|reply| SeatCommand::Decommission { reply })
            .await
        {
            Ok(result) => {
                if self.is_decommissioned() {
                    self.commands.closed().await;
                }
                result
            }
            Err(err) => {
                self.commands.closed().await;
                if self.is_decommissioned() {
                    Ok(false)
                } else {
                    Err(anyhow!(
                        "seat command loop closed during decommission: {err}"
                    ))
                }
            }
        }
    }

    pub(crate) async fn invite_code(&self) -> Result<InviteCode, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::InviteCode { reply })
            .await
    }

    /// Invite already present in the shared runtime mirror. Telemetry
    /// discovery must never turn listing into a child probe or wait behind a
    /// lifecycle command.
    pub(crate) fn cached_invite_code(&self) -> Option<InviteCode> {
        self.state.borrow().formed_invite.clone()
    }

    /// `GetPeerAttestation`'s unsigned material: what this seat can prove
    /// about its federation and its own place in it.
    pub(crate) async fn federation_binding(&self) -> Result<SeatFederationBinding, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::FederationBinding { reply })
            .await
    }

    /// Sign an attestation with this seat's configured API endpoint key.
    pub(crate) async fn sign_endpoint_proof(
        &self,
        statement: FmanPeerAttestationStatement,
    ) -> Result<SeatEndpointProof, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::SignEndpointProof { statement, reply })
            .await
    }

    /// Validate and cast one formation-only metadata vote.
    pub(crate) async fn propose_formation_meta(
        &self,
        expected_base: MetaConsensusBase,
        seat_bindings: Vec<FormationSeatBinding>,
        fi_fee_account: Account,
        send_ppm: u64,
        min_send_ppm: u64,
        guardian_verification_fee_account: Account,
    ) -> Result<(), SeatVerbError> {
        self.verb_request(|reply| SeatCommand::ProposeFormationMeta {
            expected_base,
            seat_bindings,
            fi_fee_account,
            send_ppm,
            min_send_ppm,
            guardian_verification_fee_account,
            reply,
        })
        .await
    }

    /// `SetMetaField`: validate the proposal and cast this guardian's vote.
    pub(crate) async fn submit_meta_field(
        &self,
        expected_base: MetaConsensusBase,
        key: MetaFieldKey,
        value: MetaFieldValue,
        min_send_ppm: u64,
        guardian_verification_fee_account: Option<Account>,
    ) -> Result<(), SeatVerbError> {
        self.verb_request(|reply| SeatCommand::SubmitMetaField {
            expected_base,
            key,
            value,
            min_send_ppm,
            guardian_verification_fee_account,
            reply,
        })
        .await
    }

    /// Store a gateway URL in this guardian's local LNv2 module.
    pub(crate) async fn register_gateway(
        &self,
        gateway_api: SafeUrl,
    ) -> Result<bool, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::RegisterGateway { gateway_api, reply })
            .await
    }

    /// Read guardian-fee policy through this seat's own fedimintd.
    pub(crate) async fn guardian_fee_policy(
        &self,
        our_account_id: AccountId,
    ) -> Result<FeePolicy, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::GuardianFeePolicy {
            our_account_id,
            reply,
        })
        .await
    }
}

impl SeatRuntimeFacts {
    fn unformed_status(&self) -> ServiceStatus {
        match self.process_slot {
            ProcessSlotPhase::DkgAcknowledged => ServiceStatus::DkgInProcess,
            _ => ServiceStatus::New,
        }
    }

    fn reject_decommissioned(&self) -> Result<(), SeatVerbError> {
        if self.decommissioned_at_ms.is_some() {
            return Err(SeatVerbError::WrongState {
                status: ServiceStatus::Decommissioned,
            });
        }
        Ok(())
    }

    fn service_status(&self, final_data_exists: bool) -> ServiceStatus {
        if self.decommissioned_at_ms.is_some() {
            ServiceStatus::Decommissioned
        } else if self.formed_invite.is_some() {
            if final_data_exists {
                ServiceStatus::Running
            } else {
                ServiceStatus::DataLoss
            }
        } else {
            self.unformed_status()
        }
    }

    fn report(&self, final_data_exists: bool) -> SeatReport {
        if let Some(at_ms) = self.decommissioned_at_ms {
            return SeatReport::Decommissioned { at_ms };
        }
        let (phase, health) = if let Some(invite_code) = &self.formed_invite {
            (
                if final_data_exists {
                    SeatPhase::Running {
                        invite_code: invite_code.clone(),
                    }
                } else {
                    SeatPhase::DataLoss {
                        invite_code: invite_code.clone(),
                    }
                },
                if final_data_exists {
                    self.health.clone()
                } else {
                    SeatHealth::Unavailable
                },
            )
        } else {
            match self.process_slot {
                ProcessSlotPhase::DkgAcknowledged => {
                    (SeatPhase::DkgInProgress, SeatHealth::Healthy)
                }
                _ => (SeatPhase::Created, SeatHealth::Unavailable),
            }
        };
        SeatReport::Active { phase, health }
    }
}

impl SeatLoop {
    fn final_data_exists(&self) -> Result<bool, SeatVerbError> {
        self.data_dir.try_exists().map_err(|error| {
            SeatVerbError::internal(anyhow!("inspect final seat data dir: {error}"))
        })
    }

    fn watchdog_eligible(&self) -> std::io::Result<bool> {
        if self.state.borrow().formed_invite.is_none() {
            return Ok(false);
        }
        self.data_dir.try_exists()
    }

    fn deschedule_watchdog(&mut self) {
        self.watchdog_at = None;
        self.state.send_if_modified(|state| {
            if state.health == SeatHealth::Unavailable {
                return false;
            }
            state.health = SeatHealth::Unavailable;
            true
        });
    }

    fn schedule_watchdog(&mut self, delay: Duration) {
        self.watchdog_at = match self.watchdog_eligible() {
            Ok(true) => Some(tokio::time::Instant::now() + delay),
            // Both call sites enter with Unavailable health. Only a watchdog
            // tick can deschedule a previously observed Healthy state.
            Ok(false) => None,
            Err(error) => {
                tracing::warn!(seat_id = %self.facts.seat_id, %error, "failed to inspect watchdog eligibility");
                None
            }
        };
    }

    async fn watchdog_tick(&mut self) {
        match self.watchdog_eligible() {
            Ok(true) => {}
            Ok(false) => {
                self.deschedule_watchdog();
                return;
            }
            Err(error) => {
                tracing::warn!(seat_id = %self.facts.seat_id, %error, "failed to inspect watchdog eligibility");
                self.deschedule_watchdog();
                return;
            }
        }

        let result = tokio::time::timeout(WATCHDOG_PROBE_TIMEOUT, self.client().probe()).await;
        let (health, failure_kind) = match result {
            Ok(Ok(())) => (SeatHealth::Healthy, None),
            Err(_) => (SeatHealth::Unavailable, Some("timeout")),
            Ok(Err(FedimintApiError::Unreachable(_))) => {
                (SeatHealth::Unavailable, Some("unreachable"))
            }
            Ok(Err(FedimintApiError::Rejected { .. })) => {
                (SeatHealth::Unavailable, Some("rejected"))
            }
            Ok(Err(FedimintApiError::InvalidResponse { .. })) => {
                (SeatHealth::Unavailable, Some("invalid_response"))
            }
        };
        let interval = if health == SeatHealth::Healthy {
            HEALTHY_WATCHDOG_INTERVAL
        } else {
            UNHEALTHY_WATCHDOG_INTERVAL
        };
        self.set_watchdog_health(health == SeatHealth::Healthy, failure_kind);
        self.watchdog_at = Some(tokio::time::Instant::now() + interval);
    }

    fn set_watchdog_health(&self, healthy: bool, failure_kind: Option<&'static str>) {
        let health = if healthy {
            SeatHealth::Healthy
        } else {
            SeatHealth::Unavailable
        };
        let changed = self.state.send_if_modified(|state| {
            if state.health == health {
                return false;
            }
            state.health = health.clone();
            true
        });
        if !changed {
            return;
        }
        if healthy {
            tracing::info!(
                safe_to_share = true,
                seat_id = %self.facts.seat_id,
                "seat federation health recovered"
            );
        } else {
            tracing::warn!(
                safe_to_share = true,
                seat_id = %self.facts.seat_id,
                failure_kind = failure_kind.unwrap_or("unknown"),
                "seat federation health became unavailable"
            );
        }
    }

    fn trace_formation_operation<T>(
        &self,
        operation: &'static str,
        result: &Result<T, SeatVerbError>,
    ) {
        match result {
            Ok(_) => tracing::info!(
                safe_to_share = true,
                seat_id = %self.facts.seat_id,
                operation,
                "formation seat operation completed"
            ),
            Err(error) => {
                match error {
                    SeatVerbError::Internal(source) => tracing::warn!(
                        seat_id = %self.facts.seat_id,
                        operation,
                        error = format_args!("{source:#}"),
                        "formation seat operation failed"
                    ),
                    _ => tracing::warn!(
                        seat_id = %self.facts.seat_id,
                        operation,
                        %error,
                        "formation seat operation failed"
                    ),
                }
                tracing::warn!(
                    safe_to_share = true,
                    seat_id = %self.facts.seat_id,
                    operation,
                    failure_kind = seat_verb_error_kind(error),
                    "formation seat operation failed"
                );
            }
        }
    }

    async fn run(mut self, mut commands: mpsc::Receiver<SeatCommand>) {
        if let Err(error) = self.spawn_child().await {
            tracing::warn!(seat_id = %self.facts.seat_id, error = format_args!("{error:#}"), "seat fedimintd spawn failed");
            self.schedule_respawn();
        }
        self.schedule_watchdog(Duration::ZERO);
        loop {
            let command = tokio::select! {
                biased;
                observation = self.child.observe(), if self.child.is_live() => {
                    self.handle_process_observation(observation).await;
                    continue;
                }
                _ = async { tokio::time::sleep_until(self.respawn_at.expect("respawn deadline is present")).await }, if self.respawn_at.is_some() => {
                    self.respawn_at = None;
                    if let Err(error) = self.spawn_child().await {
                        tracing::warn!(seat_id = %self.facts.seat_id, error = format_args!("{error:#}"), "seat fedimintd spawn failed");
                        self.schedule_respawn();
                    }
                    continue;
                }
                _ = async { tokio::time::sleep_until(self.watchdog_at.expect("watchdog deadline is present")).await }, if self.watchdog_at.is_some() => {
                    self.watchdog_at = None;
                    self.watchdog_tick().await;
                    continue;
                }
                command = commands.recv() => command,
            };
            let Some(command) = command else {
                break;
            };
            match command {
                #[cfg(test)]
                SeatCommand::WatchdogTick { reply } => {
                    self.watchdog_tick().await;
                    let _ = reply.send(());
                }
                SeatCommand::DkgCode {
                    federation_name,
                    reply,
                } => {
                    let result = self.dkg_code(federation_name.as_ref()).await;
                    self.trace_formation_operation("get_dkg_code", &result);
                    let _ = reply.send(result);
                }
                SeatCommand::StartDkg {
                    codes,
                    completion_callback,
                    reply,
                } => {
                    let result = self.start_dkg(&codes, completion_callback).await;
                    self.trace_formation_operation("start_dkg", &result);
                    let _ = reply.send(result);
                }
                SeatCommand::RestartDkg { codes, reply } => {
                    let result = self.restart_dkg(&codes).await;
                    self.trace_formation_operation("restart_dkg", &result);
                    let _ = reply.send(result);
                }
                SeatCommand::InviteCode { reply } => {
                    let result = self.invite_code().await;
                    self.trace_formation_operation("get_invite_code", &result);
                    let _ = reply.send(result);
                }
                SeatCommand::FederationBinding { reply } => {
                    let result = self.federation_binding().await;
                    let _ = reply.send(result);
                }
                SeatCommand::SignEndpointProof { statement, reply } => {
                    let result = self.sign_endpoint_proof(&statement);
                    let _ = reply.send(result);
                }
                SeatCommand::ProposeFormationMeta {
                    expected_base,
                    seat_bindings,
                    fi_fee_account,
                    send_ppm,
                    min_send_ppm,
                    guardian_verification_fee_account,
                    reply,
                } => {
                    let result = self
                        .propose_formation_meta(
                            expected_base,
                            &seat_bindings,
                            &fi_fee_account,
                            send_ppm,
                            min_send_ppm,
                            &guardian_verification_fee_account,
                        )
                        .await;
                    let _ = reply.send(result);
                }
                SeatCommand::SubmitMetaField {
                    expected_base,
                    key,
                    value,
                    min_send_ppm,
                    guardian_verification_fee_account,
                    reply,
                } => {
                    let result = self
                        .submit_meta_field(
                            expected_base,
                            &key,
                            &value,
                            min_send_ppm,
                            guardian_verification_fee_account.as_ref(),
                        )
                        .await;
                    let _ = reply.send(result);
                }
                SeatCommand::RegisterGateway { gateway_api, reply } => {
                    let result = self.register_gateway(gateway_api).await;
                    let _ = reply.send(result);
                }
                SeatCommand::GuardianFeePolicy {
                    our_account_id,
                    reply,
                } => {
                    let result = self.guardian_fee_policy(our_account_id).await;
                    let _ = reply.send(result);
                }
                SeatCommand::Decommission { reply } => {
                    let result = self.decommission().await;
                    // The database marker is the terminal authority.
                    let terminal = self.state.borrow().decommissioned_at_ms.is_some();
                    let _ = reply.send(result);
                    if terminal {
                        // Dropping the receiver rejects every command ordered
                        // behind durable decommission and signals completion.
                        return;
                    }
                }
                SeatCommand::Shutdown { reply } => {
                    if let Err(error) = self.stop_child().await {
                        tracing::warn!(seat_id = %self.facts.seat_id, %error, "failed to stop seat during shutdown");
                    }
                    let _ = reply.send(());
                    return;
                }
            }
        }
        if let Err(error) = self.stop_child().await {
            tracing::warn!(seat_id = %self.facts.seat_id, %error, "failed to stop seat after command channel closed");
        }
    }

    async fn handle_process_observation(&mut self, observation: ProcessObservation) {
        match observation {
            ProcessObservation::Driven(event) => match event {
                Some(Ok(event)) => {
                    let terminal_failure = matches!(
                        event,
                        DrivenDkgEvent::ParamsRejected { .. } | DrivenDkgEvent::DkgFailed { .. }
                    );
                    let retired = event == DrivenDkgEvent::ControlChannelRetired;
                    if let Err(error) = self.handle_driven_event(Ok(event)).await {
                        tracing::warn!(seat_id = %self.facts.seat_id, error = format_args!("{error:#}"), "driven-DKG event handling failed");
                    }
                    if terminal_failure {
                        if let Err(error) = self.retire_failed_ceremony().await {
                            tracing::warn!(seat_id = %self.facts.seat_id, %error, "failed to retire failed ceremony child");
                        }
                    } else if retired {
                        self.retire_control_channel();
                    }
                }
                Some(Err(error)) => {
                    if let Err(error) = self.handle_driven_event(Err(error)).await {
                        tracing::warn!(seat_id = %self.facts.seat_id, error = format_args!("{error:#}"), "driven-DKG event handling failed");
                    }
                    if let Err(error) = self.retire_failed_ceremony().await {
                        tracing::warn!(seat_id = %self.facts.seat_id, %error, "failed to retire ceremony child after protocol error");
                    }
                }
                None => {
                    if let Err(error) = self.retire_failed_ceremony().await {
                        tracing::warn!(seat_id = %self.facts.seat_id, error = format_args!("{error:#}"), "failed to stop ceremony child after control stream closed");
                    }
                }
            },
            ProcessObservation::Exited(exit) => self.handle_child_exit(exit).await,
        }
    }

    async fn handle_child_exit(&mut self, exit: Result<ObservedSeatExit, SeatProcessError>) {
        let slot = std::mem::replace(&mut self.child, ProcessSlot::Empty);
        self.set_process_slot(ProcessSlot::Empty);
        let mut client = match slot {
            ProcessSlot::Parked { client, .. } | ProcessSlot::DkgAcknowledged { client, .. } => {
                Some(client)
            }
            ProcessSlot::Configured { client, .. } => client,
            ProcessSlot::Empty | ProcessSlot::ExitUnproven => unreachable!("live child guard"),
        };
        if let Some(mut client) = client.take() {
            while let Some(event) = client.next_event().await {
                match event {
                    Ok(event) => {
                        if let Err(error) = self.handle_driven_event(Ok(event)).await {
                            tracing::warn!(seat_id = %self.facts.seat_id, error = format_args!("{error:#}"), "driven-DKG event handling failed");
                        }
                    }
                    Err(error) => {
                        if let Err(error) = self.handle_driven_event(Err(error)).await {
                            tracing::warn!(seat_id = %self.facts.seat_id, error = format_args!("{error:#}"), "driven-DKG event handling failed");
                        }
                        break;
                    }
                }
            }
        }
        match exit {
            Ok(exit) => tracing::warn!(
                safe_to_share = true,
                seat_id = %self.facts.seat_id,
                exit_code = ?exit.status_code,
                signal = ?exit.signal,
                "seat fedimintd exited"
            ),
            Err(err) => {
                tracing::warn!(seat_id = %self.facts.seat_id, %err, "seat fedimintd wait failed")
            }
        }
        if self
            .child_started
            .take()
            .is_some_and(|started| started.elapsed() >= self.policy.backoff_reset_after)
        {
            self.backoff = self.policy.initial_backoff;
        }
        self.schedule_respawn();
    }

    fn client(&self) -> FedimintApi {
        self.fedimint_api.clone()
    }

    /// Publish the process slot's source phase alongside the independently
    /// owned durable and runtime facts used by status readers.
    fn set_process_slot(&mut self, slot: ProcessSlot) {
        let phase = slot.phase();
        self.child = slot;
        self.state.send_modify(|state| state.process_slot = phase);
    }

    fn mark_dkg_acknowledged(&mut self) {
        let slot = std::mem::replace(&mut self.child, ProcessSlot::Empty);
        let slot = match slot {
            ProcessSlot::Parked { child, client } => ProcessSlot::DkgAcknowledged { child, client },
            other => other,
        };
        self.set_process_slot(slot);
    }

    fn mark_configured(&mut self) {
        let slot = std::mem::replace(&mut self.child, ProcessSlot::Empty);
        let slot = match slot {
            ProcessSlot::Parked { child, client }
            | ProcessSlot::DkgAcknowledged { child, client } => ProcessSlot::Configured {
                child,
                client: Some(client),
            },
            other => other,
        };
        self.set_process_slot(slot);
    }

    fn retire_control_channel(&mut self) {
        let slot = std::mem::replace(&mut self.child, ProcessSlot::Empty);
        let slot = match slot {
            ProcessSlot::Configured { child, .. } => ProcessSlot::Configured {
                child,
                client: None,
            },
            other => other,
        };
        self.set_process_slot(slot);
    }

    async fn stop_child(&mut self) -> anyhow::Result<()> {
        self.respawn_at = None;
        let child = std::mem::replace(&mut self.child, ProcessSlot::Empty);
        self.set_process_slot(ProcessSlot::Empty);
        match child {
            ProcessSlot::Parked { child, .. }
            | ProcessSlot::DkgAcknowledged { child, .. }
            | ProcessSlot::Configured { child, .. } => {
                if let Err(error) = child.stop().await {
                    self.set_process_slot(ProcessSlot::ExitUnproven);
                    return Err(error.into());
                }
            }
            ProcessSlot::ExitUnproven => {
                self.set_process_slot(ProcessSlot::ExitUnproven);
                anyhow::bail!(
                    "a prior child stop failed; process exit is required before replacement"
                );
            }
            ProcessSlot::Empty => {}
        }
        self.child_started = None;
        Ok(())
    }

    async fn spawn_child(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(self.child, ProcessSlot::Empty),
            "child replacement requires a proved empty process slot"
        );
        let mut child = self
            .process_spawner
            .start(
                &self.process,
                self.facts.seat_id.clone(),
                self.facts.seat_no,
                self.ports,
            )
            .await
            .map_err(anyhow::Error::new)?;
        let client = match child.driven_client().await {
            Ok(client) => client,
            Err(error) => {
                if let Err(stop_error) = child.stop().await {
                    self.set_process_slot(ProcessSlot::ExitUnproven);
                    return Err(anyhow!("{error:#}; child stop also failed: {stop_error}"));
                }
                return Err(error);
            }
        };
        let state = client.child_state().clone();
        self.set_process_slot(match state {
            ChildState::NeedsParams => ProcessSlot::Parked { child, client },
            ChildState::AlreadyConfigured { .. } => ProcessSlot::Configured {
                child,
                client: Some(client),
            },
        });
        self.child_started = Some(tokio::time::Instant::now());
        self.handle_child_state(Ok(state)).await
    }

    fn schedule_respawn(&mut self) {
        if matches!(self.child, ProcessSlot::ExitUnproven) {
            return;
        }
        self.respawn_at = Some(tokio::time::Instant::now() + self.backoff);
        self.backoff = (self.backoff * 2).min(self.policy.max_backoff);
    }

    /// Clear the ephemeral ceremony and replace its child after backoff. A
    /// failed stop deliberately leaves the process slot exit-unproven and
    /// therefore permanently ineligible for replacement in this process.
    async fn retire_failed_ceremony(&mut self) -> anyhow::Result<()> {
        self.stop_child().await?;
        self.schedule_respawn();
        Ok(())
    }

    async fn handle_child_state(
        &mut self,
        state: anyhow::Result<ChildState>,
    ) -> anyhow::Result<()> {
        match state? {
            ChildState::NeedsParams => Ok(()),
            ChildState::AlreadyConfigured { invite_code } => {
                self.handle_persisted_invite(InviteCode(invite_code)).await
            }
        }
    }

    async fn handle_driven_event(
        &mut self,
        event: anyhow::Result<DrivenDkgEvent>,
    ) -> anyhow::Result<()> {
        let event = match event {
            Ok(event) => event,
            Err(error) => return Err(error),
        };
        match event {
            DrivenDkgEvent::ConfigPersisted { invite_code, .. } => {
                self.handle_persisted_invite(InviteCode(invite_code))
                    .await?;
            }
            DrivenDkgEvent::DkgStarted => {
                self.mark_dkg_acknowledged();
                tracing::info!(
                    safe_to_share = true,
                    seat_id = %self.facts.seat_id,
                    stage = "dkg_started",
                    "driven DKG start was observed"
                );
            }
            DrivenDkgEvent::ParamsRejected { .. } => {}
            DrivenDkgEvent::DkgFailed { reason: _ } => {}
            DrivenDkgEvent::ConsensusStarted | DrivenDkgEvent::ControlChannelRetired => {}
        }
        Ok(())
    }

    async fn handle_persisted_invite(&mut self, invite_code: InviteCode) -> anyhow::Result<()> {
        self.db
            .record_formed(&self.facts.seat_id, &invite_code)
            .await?;
        self.state
            .send_modify(|state| state.formed_invite = Some(invite_code.clone()));
        self.mark_configured();
        self.schedule_watchdog(Duration::ZERO);
        self.backup.mark();
        self.completion_hooks.mark();
        tracing::info!(
            safe_to_share = true,
            seat_id = %self.facts.seat_id,
            stage = "configuration_persisted",
            "driven DKG configuration is durable"
        );
        Ok(())
    }

    /// `GetDkgCode`: ensure fedimintd has this seat's local params and return
    /// the setup code. Idempotent for a repeated identical request; a request
    /// that disagrees with the active runtime session must be cancelled first.
    async fn dkg_code(
        &mut self,
        federation_name: Option<&FederationName>,
    ) -> Result<GuardianCode, SeatVerbError> {
        let final_data_exists = self.final_data_exists()?;
        let state = self.state.borrow();
        state.reject_decommissioned()?;
        if state.formed_invite.is_some() {
            return Err(SeatVerbError::WrongState {
                status: state.service_status(final_data_exists),
            });
        }
        drop(state);
        self.mint_setup_code(federation_name)
    }

    /// `StartDkg` owns only an in-memory session. Its own code is identified by
    /// the seat's Iroh API key and then recomputed byte-for-byte from the
    /// submitted code's federation-name field.
    async fn start_dkg(
        &mut self,
        codes: &[GuardianCode],
        completion_callback: Option<ValidatedDkgCompletionCallback>,
    ) -> Result<(), SeatVerbError> {
        let final_data_exists = self.final_data_exists()?;
        self.state.borrow().reject_decommissioned()?;
        if self.state.borrow().formed_invite.is_some() {
            return Err(SeatVerbError::WrongState {
                status: self.state.borrow().service_status(final_data_exists),
            });
        }
        if self.state.borrow().unformed_status() == ServiceStatus::DkgInProcess {
            return Err(SeatVerbError::WrongState {
                status: ServiceStatus::DkgInProcess,
            });
        }
        if !matches!(
            self.child.client().map(|client| client.child_state()),
            Some(ChildState::NeedsParams)
        ) {
            return Err(SeatVerbError::SeatUnavailable);
        }
        let (submitted, own_code) = self.validate_dkg_codes(codes)?;

        // The parked child has proved that this seat can receive a ceremony.
        // Retain the callback before sending parameters so a crash after the
        // child persists its configuration cannot lose formation-level work.
        let callback = completion_callback.map(ValidatedDkgCompletionCallback::into_inner);
        self.db
            .install_completion_callback(&self.facts.seat_id, callback.as_ref())
            .await
            .map_err(SeatVerbError::internal)?;
        self.completion_hooks.mark();

        self.run_validated_dkg(&submitted, &own_code).await
    }

    /// Validate one complete ceremony envelope without mutating the child.
    fn validate_dkg_codes(
        &self,
        codes: &[GuardianCode],
    ) -> Result<(DkgCodeSet, GuardianCode), SeatVerbError> {
        let own_api_key = effective_iroh_api_key(&self.keys, self.ports).public();
        let mut own = None;
        for code in codes {
            let setup: PeerSetupCode = base32::decode_prefixed(FEDIMINT_PREFIX, &code.0)
                .map_err(|err| SeatVerbError::InvalidDkgInput(err.to_string()))?;
            if matches!(&setup.endpoints, PeerEndpoints::Iroh { api_pk, .. } if api_pk.as_bytes() == own_api_key.as_bytes())
            {
                if own.is_some() {
                    return Err(SeatVerbError::InvalidDkgInput(
                        "multiple guardian codes use this seat's API key".to_owned(),
                    ));
                }
                let expected = self.mint_setup_code(
                    setup
                        .federation_name
                        .as_ref()
                        .map(|name| FederationName(name.clone()))
                        .as_ref(),
                )?;
                if expected != *code {
                    return Err(SeatVerbError::InvalidDkgInput(
                        "own guardian code failed deterministic recomputation".to_owned(),
                    ));
                }
                own = Some(code.clone());
            }
        }
        let own_code = own.ok_or_else(|| {
            SeatVerbError::InvalidDkgInput("own guardian code missing from the set".to_owned())
        })?;
        let submitted = DkgCodeSet::validate(codes, self.facts.federation_size, &own_code)
            .map_err(|err| SeatVerbError::InvalidDkgInput(err.to_string()))?;
        Ok((submitted, own_code))
    }

    /// Send already-validated ceremony parameters and wait through the
    /// child's `DkgStarted` acknowledgement under the shared timeout.
    async fn run_validated_dkg(
        &mut self,
        submitted: &DkgCodeSet,
        own_code: &GuardianCode,
    ) -> Result<(), SeatVerbError> {
        let (our_index, codes) = Self::ceremony_params(&submitted, &own_code)?;
        let started = tokio::time::timeout(DKG_START_TIMEOUT, async {
            self.child
                .client_mut()
                .ok_or(SeatVerbError::SeatUnavailable)?
                .run_dkg(RunDkgParams {
                    our_index,
                    codes,
                    iroh_api_sk: effective_iroh_api_key(&self.keys, self.ports).to_bytes(),
                    iroh_p2p_sk: effective_iroh_p2p_key(&self.keys, self.ports),
                    tls_key: None,
                    api_auth: self.keys.api_auth.clone(),
                    network: self.process.bitcoin_network.to_string(),
                })
                .await
                .map_err(SeatVerbError::internal)?;
            self.await_dkg_started().await
        })
        .await;
        match started {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => match self.retire_failed_ceremony().await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(SeatVerbError::internal(anyhow!(
                    "{error}; ceremony cleanup also failed: {cleanup:#}"
                ))),
            },
            Err(_) => {
                self.retire_failed_ceremony()
                    .await
                    .map_err(SeatVerbError::internal)?;
                Err(SeatVerbError::internal(anyhow!(
                    "timed out waiting for driven DKG start acknowledgement"
                )))
            }
        }
    }

    fn ceremony_params(
        submitted: &DkgCodeSet,
        own_code: &GuardianCode,
    ) -> Result<(u16, Vec<String>), SeatVerbError> {
        let mut ordered = submitted
            .iter()
            .map(|code| {
                let upstream = code.0.clone();
                let decoded: PeerSetupCode = base32::decode_prefixed(FEDIMINT_PREFIX, &upstream)
                    .map_err(|err| SeatVerbError::InvalidDkgInput(err.to_string()))?;
                Ok((decoded, upstream, code == own_code))
            })
            .collect::<Result<Vec<_>, SeatVerbError>>()?;
        ordered.sort_by(|left, right| left.0.cmp(&right.0));
        let our_index = ordered
            .iter()
            .position(|entry| entry.2)
            .and_then(|index| u16::try_from(index).ok())
            .expect("validated federation size and own code fit a peer id");
        Ok((
            our_index,
            ordered.into_iter().map(|(_, code, _)| code).collect(),
        ))
    }

    async fn await_dkg_started(&mut self) -> Result<(), SeatVerbError> {
        loop {
            let event = self
                .child
                .client_mut()
                .ok_or(SeatVerbError::SeatUnavailable)?
                .next_event()
                .await;
            let Some(event) = event else {
                return Err(SeatVerbError::internal(anyhow!(
                    "driven-DKG event stream closed"
                )));
            };
            let event = event.map_err(SeatVerbError::internal)?;
            match event {
                DrivenDkgEvent::DkgStarted => {
                    self.handle_driven_event(Ok(DrivenDkgEvent::DkgStarted))
                        .await
                        .map_err(SeatVerbError::internal)?;
                    return Ok(());
                }
                DrivenDkgEvent::ConsensusStarted if self.state.borrow().formed_invite.is_some() => {
                    return Ok(());
                }
                DrivenDkgEvent::ParamsRejected { reason } => {
                    return Err(SeatVerbError::InvalidDkgInput(reason));
                }
                DrivenDkgEvent::DkgFailed { reason } => {
                    return Err(SeatVerbError::Internal(anyhow!("DKG failed: {reason}")));
                }
                other => self
                    .handle_driven_event(Ok(other))
                    .await
                    .map_err(SeatVerbError::internal)?,
            }
        }
    }

    /// `RestartDkg`: stop and reap the current child, replace it, and start a
    /// fresh ceremony unless the replacement reports that formation won the
    /// race.
    ///
    /// The final directory is the authoritative destructive-safety gate even
    /// if a prior crash prevented the configured event from reaching this
    /// process. Setup attempts use only the transient directory owned by the
    /// child, so FMan never has setup state to wipe or reconstruct.
    /// Restart only the in-memory driven session. The child's staging directory
    /// dies with that session; the final directory is never removed here.
    async fn restart_dkg(
        &mut self,
        codes: &[GuardianCode],
    ) -> Result<ServiceStatus, SeatVerbError> {
        let final_data_exists = self.final_data_exists()?;
        self.state.borrow().reject_decommissioned()?;
        if self.state.borrow().formed_invite.is_some() {
            return Err(SeatVerbError::WrongState {
                status: self.state.borrow().service_status(final_data_exists),
            });
        }
        self.stop_child().await.map_err(SeatVerbError::internal)?;
        if let Err(error) = self.spawn_child().await {
            self.schedule_respawn();
            return Err(SeatVerbError::internal(error));
        }
        if self.state.borrow().formed_invite.is_some() {
            return Ok(ServiceStatus::Running);
        }
        if !matches!(
            self.child.client().map(|client| client.child_state()),
            Some(ChildState::NeedsParams)
        ) {
            return Err(SeatVerbError::SeatUnavailable);
        }
        let (submitted, own_code) = self.validate_dkg_codes(codes)?;
        self.run_validated_dkg(&submitted, &own_code).await?;
        Ok(ServiceStatus::DkgInProcess)
    }

    /// Operator decommission: terminal, idempotent. Stops the child, frees
    /// the capacity slot, retains the lifetime port allocation, and keeps the
    /// seat's durable record (creation commitments and payment evidence stay
    /// dispute material). Only an active loop can execute this command;
    /// repeated calls are answered by the terminal handle without a loop.
    ///
    /// The seat's on-disk guardian data is retained. Decommission is an
    /// operator's own decision about capacity, and it must not be the act
    /// that destroys key shares a federation may still need; removing that
    /// data stays a separate, deliberate step.
    ///
    /// The durable mark lands before the child stops, so a crash between the
    /// two cannot leave a seat that is running but no longer recorded as
    /// live.
    async fn decommission(&mut self) -> anyhow::Result<bool> {
        let at_ms = self.db.decommission_seat(&self.facts.seat_id).await?;
        self.state
            .send_modify(|state| state.decommissioned_at_ms = Some(at_ms));
        self.completion_hooks.mark();
        self.stop_child().await?;
        Ok(true)
    }

    /// `GetInviteCode`: available once the seat's federation is running.
    ///
    /// The recorded invite answers without a probe: it is constant once
    /// consensus has run (the seat's iroh keys are deterministic and the code
    /// names this guardian's peer), so a re-fetch could only agree or mean
    /// the child is momentarily unreachable — neither is worth a round trip
    /// or an availability dependency on the child.
    async fn invite_code(&mut self) -> Result<InviteCode, SeatVerbError> {
        let state = self.state.borrow();
        state
            .formed_invite
            .clone()
            .ok_or_else(|| SeatVerbError::WrongState {
                status: state.unformed_status(),
            })
    }

    /// `GetPeerAttestation`: derive what this seat can attest to.
    ///
    /// The peer id comes from the invite code rather than from any config
    /// lookup ([`own_peer_id`]).
    async fn federation_binding(&mut self) -> Result<SeatFederationBinding, SeatVerbError> {
        if self.state.borrow().formed_invite.is_none() {
            let status = self.state.borrow().unformed_status();
            return Err(SeatVerbError::WrongState { status });
        }
        let (client, config) = self.running_client_config().await?;
        let federation = derive_federation_seats(&config)?;
        let peer_id = own_peer_id(&client).await?;

        let seat = federation.seat(&peer_id).cloned().ok_or_else(|| {
            SeatVerbError::internal(anyhow!(
                "seat's own peer {} is absent from its federation config",
                peer_id.0
            ))
        })?;

        Ok(SeatFederationBinding {
            seat_id: self.facts.seat_id.clone(),
            federation,
            seat,
        })
    }

    fn sign_endpoint_proof(
        &self,
        statement: &FmanPeerAttestationStatement,
    ) -> Result<SeatEndpointProof, SeatVerbError> {
        let signature = effective_iroh_api_key(&self.keys, self.ports).sign(
            &statement
                .seat_endpoint_proof_message()
                .map_err(SeatVerbError::internal)?,
        );
        Ok(SeatEndpointProof {
            signature: signature.to_bytes().to_vec(),
        })
    }

    async fn propose_formation_meta(
        &mut self,
        expected_base: MetaConsensusBase,
        seat_bindings: &[FormationSeatBinding],
        fi_fee_account: &Account,
        send_ppm: u64,
        min_send_ppm: u64,
        guardian_verification_fee_account: &Account,
    ) -> Result<(), SeatVerbError> {
        crate::guardian_fee::prevalidate_guardian_fee_rate(send_ppm, min_send_ppm)
            .map_err(|_| SeatVerbError::MetaValueInvalid)?;

        let (client, config) = self.running_client_config().await?;
        let meta_id = meta_module_id(&config).ok_or_else(|| {
            SeatVerbError::internal(anyhow!("federation config carries no meta module"))
        })?;
        let current = client
            .meta_get_consensus(meta_id, DEFAULT_META_KEY)
            .await
            .map_err(SeatVerbError::child_needed)?;
        if let Some(consensus) = &current {
            self.ensure_meta_object_size(consensus.value.as_slice(), "propose_formation_meta")?;
        }
        let actual_base = MetaConsensusBase::from_consensus(
            current
                .as_ref()
                .map(|consensus| (consensus.revision, consensus.value.as_slice())),
        );
        let mut fields: BTreeMap<String, serde_json::Value> = match &current {
            Some(consensus) => {
                serde_json::from_slice(consensus.value.as_slice()).map_err(|err| {
                    SeatVerbError::internal(anyhow!(
                        "federation's consensus metadata is not a JSON object: {err}"
                    ))
                })?
            }
            None => BTreeMap::new(),
        };
        if fields.contains_key(FMAN_SEAT_BINDINGS_META_FIELD_KEY) {
            return Err(SeatVerbError::FormationMetaAlreadyPublished);
        }
        if actual_base != expected_base {
            return Err(SeatVerbError::MetaConsensusChanged);
        }

        let federation = derive_federation_seats(&config)?;
        let bindings = FmanSeatBindings::new(
            seat_bindings
                .iter()
                .map(|binding| binding.attestation.clone()),
        )
        .map_err(|_| SeatVerbError::MetaValueInvalid)?;
        let canonical_bindings = bindings
            .canonical_string()
            .map_err(|_| SeatVerbError::MetaValueInvalid)?;
        let verified = bindings
            .verify_for_federation(&federation)
            .map_err(|_| SeatVerbError::MetaValueInvalid)?;
        let own_peer = own_peer_id(&client).await?;
        if !verified.iter().any(|binding| {
            binding.peer_id == own_peer && binding.fman_pubkey == self.own_fman_pubkey
        }) {
            return Err(SeatVerbError::MetaValueInvalid);
        }
        self.verify_endpoint_proofs(&config, seat_bindings)?;

        let guardian_accounts = guardian_fee_bindings(&verified);
        let recipients = crate::guardian_fee::canonical_formation_proposal(
            send_ppm,
            &guardian_accounts,
            fi_fee_account,
            guardian_verification_fee_account,
        )
        .map_err(|_| SeatVerbError::MetaValueInvalid)?;

        fields.insert(
            FMAN_SEAT_BINDINGS_META_FIELD_KEY.to_owned(),
            serde_json::Value::String(canonical_bindings),
        );
        fields.insert(
            crate::guardian_fee::SEND_PPM_META_KEY.to_owned(),
            serde_json::Value::String(send_ppm.to_string()),
        );
        fields.insert(
            crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY.to_owned(),
            serde_json::Value::String(recipients),
        );
        let encoded = serde_json_canonicalizer::to_vec(&fields).map_err(|err| {
            SeatVerbError::internal(anyhow!("could not canonicalize consensus metadata: {err}"))
        })?;
        self.ensure_meta_object_size(&encoded, "propose_formation_meta")?;
        self.submit_admitted_meta_target(
            &client,
            meta_id,
            expected_base,
            encoded,
            "propose_formation_meta",
        )
        .await
    }

    fn verify_endpoint_proofs(
        &self,
        config: &ClientConfig,
        bindings: &[FormationSeatBinding],
    ) -> Result<(), SeatVerbError> {
        for binding in bindings {
            let peer_id = &binding.attestation.attestation.peer_id;
            let proof = &binding.endpoint_proof;
            let config_peer =
                parse_protocol_peer_id(peer_id).ok_or(SeatVerbError::MetaValueInvalid)?;
            let endpoint = config
                .global
                .api_endpoints
                .get(&config_peer)
                .ok_or(SeatVerbError::MetaValueInvalid)?;
            let endpoint_key = endpoint
                .url
                .as_str()
                .strip_prefix("iroh://")
                .and_then(|value| value.split('?').next())
                .and_then(|value| value.parse::<iroh_base_035::PublicKey>().ok())
                .ok_or(SeatVerbError::MetaValueInvalid)?;
            let signature = iroh_base_035::Signature::try_from(proof.signature.as_slice())
                .map_err(|_| SeatVerbError::MetaValueInvalid)?;
            endpoint_key
                .verify(
                    &binding
                        .attestation
                        .attestation
                        .seat_endpoint_proof_message()
                        .map_err(SeatVerbError::internal)?,
                    &signature,
                )
                .map_err(|_| SeatVerbError::MetaValueInvalid)?;
        }
        Ok(())
    }

    /// `SetMetaField`: validate the proposal, then cast this guardian's vote.
    ///
    /// Returning `Ok` means *submitted*, not live. The meta module promotes a
    /// value only once `NumPeers::threshold()` guardians have submitted
    /// byte-identical bytes, so the FI learns the write landed by reading
    /// consensus back, never from this response
    /// ([`SPEC-federation-trust-directory`](../../../domain/specs/SPEC-federation-trust-directory.md)).
    ///
    /// The write is a read-modify-write over the whole meta object. The signed
    /// request commits to the exact value the FI read, so a different field
    /// reaching consensus first makes this proposal stale instead of letting
    /// a late whole-object vote silently discard it. The FI serializes typed
    /// maintenance operations and confirms each target by consensus readback.
    async fn submit_meta_field(
        &mut self,
        expected_base: MetaConsensusBase,
        key: &MetaFieldKey,
        value: &MetaFieldValue,
        min_send_ppm: u64,
        guardian_verification_fee_account: Option<&Account>,
    ) -> Result<(), SeatVerbError> {
        validate_meta_field(key, value).map_err(|err| self.meta_field_error(key.0.len(), err))?;
        if key.0 == crate::guardian_fee::SEND_PPM_META_KEY {
            let send_ppm = value
                .0
                .parse::<u64>()
                .map_err(|_| SeatVerbError::MetaValueInvalid)?;
            crate::guardian_fee::prevalidate_guardian_fee_rate(send_ppm, min_send_ppm)
                .map_err(|_| SeatVerbError::MetaValueInvalid)?;
        }
        let (client, config) = self.running_client_config().await?;
        // A JSON string field inside the meta object, matching Fedimint's own
        // `ClientConfig::meta` convention and what FLIP's reader expects.
        self.submit_meta_patch(
            &client,
            &config,
            expected_base,
            BTreeMap::from([(key.0.clone(), serde_json::Value::String(value.0.clone()))]),
            guardian_verification_fee_account,
            "set_meta_field",
        )
        .await
    }

    async fn register_gateway(&mut self, gateway_api: SafeUrl) -> Result<bool, SeatVerbError> {
        let client = self.running_client().await?;
        client
            .add_gateway(gateway_api)
            .await
            .map_err(SeatVerbError::child_needed)
    }

    /// Guarded read/merge/canonicalize/submit primitive shared by every FI
    /// writer of the opaque meta-module value.
    async fn submit_meta_patch(
        &mut self,
        client: &FedimintApi,
        config: &ClientConfig,
        expected_base: MetaConsensusBase,
        updates: BTreeMap<String, serde_json::Value>,
        guardian_verification_fee_account: Option<&Account>,
        operation: &'static str,
    ) -> Result<(), SeatVerbError> {
        self.submit_meta_mutation(
            client,
            config,
            expected_base,
            guardian_verification_fee_account,
            operation,
            |fields| {
                fields.extend(updates);
                Ok(())
            },
        )
        .await
    }

    async fn submit_meta_mutation(
        &mut self,
        client: &FedimintApi,
        config: &ClientConfig,
        expected_base: MetaConsensusBase,
        guardian_verification_fee_account: Option<&Account>,
        operation: &'static str,
        mutate: impl FnOnce(&mut BTreeMap<String, serde_json::Value>) -> Result<(), SeatVerbError>,
    ) -> Result<(), SeatVerbError> {
        let meta_id = meta_module_id(config).ok_or_else(|| {
            SeatVerbError::internal(anyhow!("federation config carries no meta module"))
        })?;
        let current = client
            .meta_get_consensus(meta_id, DEFAULT_META_KEY)
            .await
            .map_err(SeatVerbError::child_needed)?;
        if let Some(consensus) = &current {
            self.ensure_meta_object_size(consensus.value.as_slice(), operation)?;
        }
        let actual_base = MetaConsensusBase::from_consensus(
            current
                .as_ref()
                .map(|consensus| (consensus.revision, consensus.value.as_slice())),
        );
        if actual_base != expected_base {
            tracing::warn!(
                seat_id = %self.facts.seat_id,
                operation,
                "refusing a metadata mutation based on stale consensus",
            );
            return Err(SeatVerbError::MetaConsensusChanged);
        }
        let mut fields: BTreeMap<String, serde_json::Value> = match &current {
            Some(consensus) => {
                serde_json::from_slice(consensus.value.as_slice()).map_err(|err| {
                    SeatVerbError::internal(anyhow!(
                        "federation's consensus metadata is not a JSON object: {err}"
                    ))
                })?
            }
            None => BTreeMap::new(),
        };
        mutate(&mut fields)?;
        self.validate_carried_guardian_fee_policy(
            &fields,
            config,
            guardian_verification_fee_account,
        )?;

        // Canonicalize rather than merely serialize: guardians reach threshold
        // only on byte-identical submissions, so encoding cannot depend on the
        // input object's field order.
        let encoded = serde_json_canonicalizer::to_vec(&fields).map_err(|err| {
            SeatVerbError::internal(anyhow!("could not canonicalize consensus metadata: {err}"))
        })?;
        self.ensure_meta_object_size(&encoded, operation)?;

        self.submit_admitted_meta_target(client, meta_id, expected_base, encoded, operation)
            .await
    }

    /// Every meta submission is a vote for the entire object, including fee
    /// keys that an unrelated maintenance request merely carries forward.
    /// Re-derive the authenticated split here so the generic path cannot vote
    /// for a hostile or stale fee policy by accident.
    fn validate_carried_guardian_fee_policy(
        &self,
        fields: &BTreeMap<String, serde_json::Value>,
        config: &ClientConfig,
        guardian_verification_fee_account: Option<&Account>,
    ) -> Result<(), SeatVerbError> {
        let send_ppm = fields.get(crate::guardian_fee::SEND_PPM_META_KEY);
        let recipients = fields.get(crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY);
        let (Some(send_ppm), Some(recipients)) = (send_ppm, recipients) else {
            return if send_ppm.is_none() && recipients.is_none() {
                Ok(())
            } else {
                Err(SeatVerbError::MetaValueInvalid)
            };
        };
        let send_ppm = send_ppm
            .as_str()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(SeatVerbError::MetaValueInvalid)?;
        let recipients = recipients.as_str().ok_or(SeatVerbError::MetaValueInvalid)?;
        let guardian_verification_fee_account =
            guardian_verification_fee_account.ok_or(SeatVerbError::MetaValueInvalid)?;
        let federation = derive_federation_seats(config)?;
        let directory_value = fields
            .get(FMAN_SEAT_BINDINGS_META_FIELD_KEY)
            .and_then(serde_json::Value::as_str)
            .ok_or(SeatVerbError::MetaValueInvalid)?;
        let bindings = FmanSeatBindings::parse_canonical(directory_value)
            .and_then(|bindings| bindings.verify_for_federation(&federation))
            .map_err(|_| SeatVerbError::MetaValueInvalid)?;
        let guardians = guardian_fee_bindings(&bindings);
        crate::guardian_fee::validate_canonical_proposal_value(
            send_ppm,
            recipients,
            &guardians,
            guardian_verification_fee_account,
        )
        .map_err(|_| SeatVerbError::MetaValueInvalid)
    }

    async fn submit_admitted_meta_target(
        &mut self,
        client: &FedimintApi,
        meta_id: ModuleInstanceId,
        expected_base: MetaConsensusBase,
        encoded: Vec<u8>,
        operation: &'static str,
    ) -> Result<(), SeatVerbError> {
        let target: MetaTargetDigest = {
            use sha2::Digest as _;
            sha2::Sha256::digest(&encoded).into()
        };
        match &self.meta_admission {
            // The pin names this exact live occurrence: only the admitted
            // target may be submitted or exactly replayed for it.
            Some((base, admitted)) if *base == expected_base => {
                if *admitted != target {
                    tracing::warn!(
                        seat_id = %self.facts.seat_id,
                        operation,
                        "refusing a conflicting whole-metadata target already admitted for this base",
                    );
                    return Err(SeatVerbError::MetaTargetConflict);
                }
            }
            // No pin, or a pin for a superseded occurrence. Callers pass the
            // freshly read current base and a superseded occurrence cannot
            // recur, so the old pin fences nothing and is replaced. Pin
            // before entering the fallible child RPC: an error response
            // cannot prove that the child did not accept the vote, so pinning
            // only after success would reopen the same-occurrence conflict
            // window on an ambiguous result.
            _ => {
                self.meta_admission = Some((expected_base, target));
            }
        }
        client
            .meta_submit(
                meta_id,
                DEFAULT_META_KEY,
                MetaValue::from(encoded.as_slice()),
            )
            .await
            .map_err(SeatVerbError::child_needed)?;
        Ok(())
    }

    fn ensure_meta_object_size(
        &self,
        value: &[u8],
        operation: &'static str,
    ) -> Result<(), SeatVerbError> {
        if value.len() > FEDERATION_METADATA_OBJECT_MAX_BYTES {
            tracing::warn!(
                seat_id = %self.facts.seat_id,
                operation,
                actual_bytes = value.len(),
                max_bytes = FEDERATION_METADATA_OBJECT_MAX_BYTES,
                "refusing an oversized whole consensus-metadata object",
            );
            return Err(SeatVerbError::MetaValueInvalid);
        }
        Ok(())
    }

    fn meta_field_error(&self, key_len: usize, err: MetaFieldError) -> SeatVerbError {
        match err {
            MetaFieldError::UnknownKey => {
                tracing::warn!(
                    seat_id = %self.facts.seat_id,
                    key_len,
                    "refusing a meta field with no compiled validator",
                );
                SeatVerbError::MetaKeyRefused
            }
            MetaFieldError::InvalidValue(reason) => {
                tracing::warn!(
                    seat_id = %self.facts.seat_id,
                    key_len,
                    %reason,
                    "refusing an invalid meta field value",
                );
                SeatVerbError::MetaValueInvalid
            }
        }
    }

    /// Read the consensus meta-map from our own guardian, never from config
    /// metadata or a joined federation wallet client.
    async fn guardian_fee_policy(
        &mut self,
        our_account_id: AccountId,
    ) -> Result<FeePolicy, SeatVerbError> {
        let (client, config) = self.running_client_config().await?;
        let (_, fields) = consensus_meta_fields(&client, &config).await?;
        // Non-string values are skipped rather than failing the read: the
        // payer reads the fee keys as strings, so a non-string fee value is
        // not honoured anyway, and an unrelated foreign key must not make
        // this FMan's own policy unreadable.
        let meta = fields
            .into_iter()
            .filter_map(|(key, value)| match value {
                serde_json::Value::String(value) => Some((key, value)),
                _ => None,
            })
            .collect();
        Ok(crate::guardian_fee::fee_policy_from_meta(
            &meta,
            our_account_id,
        ))
    }

    /// The seat's final client config, refusing the pre-consensus phases the
    /// same way [`Self::invite_code`] does: neither verb has an answer until
    /// DKG has produced the config they describe.
    async fn running_client_config(
        &mut self,
    ) -> Result<(FedimintApi, ClientConfig), SeatVerbError> {
        let client = self.running_client().await?;
        let config = client
            .client_config()
            .await
            .map_err(SeatVerbError::child_needed)?;
        Ok((client, config))
    }

    async fn running_client(&mut self) -> Result<FedimintApi, SeatVerbError> {
        if self.state.borrow().formed_invite.is_none() {
            let status = self.state.borrow().unformed_status();
            return Err(SeatVerbError::WrongState { status });
        }
        match self.state.borrow().health.clone() {
            SeatHealth::Healthy => Ok(self.client()),
            SeatHealth::Unavailable | SeatHealth::Failed => Err(SeatVerbError::SeatUnavailable),
        }
    }

    /// Pure deterministic guardian-code construction.
    fn mint_setup_code(
        &self,
        federation_name: Option<&FederationName>,
    ) -> Result<GuardianCode, SeatVerbError> {
        let (federation_name, federation_size, enabled_modules) = match federation_name {
            Some(name) => (
                Some(name.0.clone()),
                Some(u32::from(self.facts.federation_size.0)),
                Some(
                    [
                        "lnv2",
                        "meta",
                        "mintv2",
                        "multi_sig_stability_pool",
                        "walletv2",
                    ]
                    .into_iter()
                    .map(ModuleKind::clone_from_str)
                    .collect::<BTreeSet<_>>(),
                ),
            ),
            None => (None, None, None),
        };
        let seat_id = self.facts.seat_id.to_string();
        let setup = PeerSetupCode {
            name: format!("fm-{}", &seat_id[..8]),
            endpoints: PeerEndpoints::Iroh {
                api_pk: iroh_base_035::PublicKey::from_bytes(
                    effective_iroh_api_key(&self.keys, self.ports)
                        .public()
                        .as_bytes(),
                )
                .expect("an Iroh public key has canonical bytes"),
                p2p_pk: iroh_base_035::SecretKey::from_bytes(&effective_iroh_p2p_key(
                    &self.keys, self.ports,
                ))
                .public(),
            },
            federation_name,
            disable_base_fees: None,
            enabled_modules,
            federation_size,
        };
        Ok(GuardianCode(base32::encode_prefixed(
            FEDIMINT_PREFIX,
            &setup,
        )))
    }
}

/// Return each authenticated guardian account from the verified directory.
fn guardian_fee_bindings(bindings: &[VerifiedSeatBinding]) -> Vec<Account> {
    bindings
        .iter()
        .map(|binding| binding.guardian_fee_account.clone())
        .collect()
}

/// This guardian's own peer id, from the invite code its own fedimintd hands
/// out.
///
/// The invite code is the only trustworthy source: `ServerConfig::get_invite_code`
/// builds the code around `self.local.identity`, so the code a guardian's own
/// fedimintd hands out names *that* guardian's peer. There is no consensus
/// endpoint for a guardian's own id, and matching on the display name would
/// bind the answer to a value the operator chose rather than one consensus
/// enforces.
async fn own_peer_id(client: &FedimintApi) -> Result<PeerId, SeatVerbError> {
    let code = client
        .invite_code()
        .await
        .map_err(SeatVerbError::child_needed)?;
    Ok(protocol_peer_id(
        FedimintInviteCode::from_str(&code)
            .map_err(|err| {
                SeatVerbError::internal(anyhow!(
                    "fedimintd returned an unparsable invite code: {err}"
                ))
            })?
            .peer(),
    ))
}

/// Derive the shared federation facts from a seat's own final config.
///
/// A failure here is an internal error rather than a policy answer: the config
/// came from this seat's own running fedimintd, so a config the shared
/// derivation cannot read means the daemon and its child disagree about what
/// a federation is, not that the caller asked for something invalid.
fn derive_federation_seats(config: &ClientConfig) -> Result<FederationSeats, SeatVerbError> {
    federation_seats(config).map_err(|err| {
        SeatVerbError::internal(anyhow!("seat's own federation config is unusable: {err}"))
    })
}

/// The instance id of the federation's `meta` module, if it has one.
///
/// `fedimintd` attaches the meta module by default and the daemon never sets
/// `FM_DISABLE_META_MODULE`, so an FMan-formed federation always has one; a
/// federation formed elsewhere might not.
fn meta_module_id(config: &ClientConfig) -> Option<ModuleInstanceId> {
    config
        .modules
        .iter()
        .find(|(_, module)| module.kind == fedimint_meta_common::KIND)
        .map(|(instance_id, _)| *instance_id)
}

/// Read the consensus meta-map through this guardian's own meta module.
/// The read verb calls this directly; the guarded patch primitive and the
/// unguarded fee verb carry their own locate/parse sequence.
async fn consensus_meta_fields(
    client: &FedimintApi,
    config: &ClientConfig,
) -> Result<(ModuleInstanceId, BTreeMap<String, serde_json::Value>), SeatVerbError> {
    let meta_id = meta_module_id(config).ok_or_else(|| {
        SeatVerbError::internal(anyhow!("federation config carries no meta module"))
    })?;
    let current = client
        .meta_get_consensus(meta_id, DEFAULT_META_KEY)
        .await
        .map_err(SeatVerbError::child_needed)?;
    let fields = match current {
        Some(consensus) => serde_json::from_slice(consensus.value.as_slice()).map_err(|err| {
            SeatVerbError::internal(anyhow!(
                "federation's consensus metadata is not a JSON object: {err}"
            ))
        })?,
        None => BTreeMap::new(),
    };
    Ok((meta_id, fields))
}
