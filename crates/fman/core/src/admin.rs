//! Operator admin surface: JSON-lines over a unix socket in the data root.
//!
//! One request object per connection, one `Result<Value, AdminError>` response
//! line back. Local-only by construction: the socket lives in the fleet data
//! root with owner-only permissions, and the daemon's exclusive data-root
//! flock already guarantees a single server. No additional authentication —
//! whoever can read the data root owns the fleet anyway (it holds the root
//! mnemonic's database).
//!
//! `ShowMnemonic` returns the root mnemonic phrase in the response line;
//! `OnboardFromBackup` carries a phrase the other way. Both are written to,
//! or read from, the connected operator only, and never logged.
//!
//! The socket is bound once and samples the shared operator phase for each
//! connection. Before there is a fleet, onboarding answers the two setup verbs;
//! once the fleet opens, the same listener uses the complete dispatcher.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use fedi_decentralized_domain::FmanVersion;
use fedi_decentralized_service_fleet_manager::{FederationId, FmanName, Plan, SeatHealth, SeatId};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::directory::{DirectoryPresence, OnboardingStatus};
use crate::facts::CompletionCallbackStatus;
use crate::fleet::{Fleet, PaymentFederationStatus};
use crate::guardian_fee::{
    Collected, CollectionFailurePhase, FederationFeeStatus, FeePolicy, Remittance,
};
use crate::seat::{PaymentClaimStatus, SeatPhase, SeatReport, SeatSummary};
use crate::wallet::Msats;

/// The admin socket lives beside the database, under the same directory
/// permissions that protect the root mnemonic.
pub fn socket_path(data_root: &Path) -> PathBuf {
    data_root.join("admin.sock")
}

/// Operator verbs. The CLI (`fman-cli --data-dir ...`) is a thin mapping
/// onto these.
#[derive(serde::Serialize, serde::Deserialize, ts_rs::TS)]
pub enum AdminRequest {
    /// The current offer.
    ShowPlans,
    /// Replace the offer: the price seats are sold at, or none to stop
    /// selling. There is one plan to offer, so the price is the whole offer.
    SetPrice {
        #[ts(type = "number | null")]
        price_msats: Option<u64>,
    },
    /// Current durable admission ceiling.
    ShowCapacity,
    /// Replace the admission ceiling without moving it below active seats.
    SetCapacity { max_seats: u32 },
    /// Payment federations with wallet health and balance: the accepted
    /// common setup-payment set plus wallet-only leftovers of removed
    /// members. Read-only — acceptance is the authenticated common set,
    /// not an operator choice.
    ListPaymentFederations,
    /// The one Lightning destination used by every revenue sweep.
    PayoutDestination,
    /// Replace or clear the global Lightning destination.
    SetPayoutDestination { destination: Option<String> },
    /// Sweep one setup-payment wallet through an automatically selected gateway.
    SweepPaymentFees {
        #[ts(type = "FederationId")]
        federation_id: FederationId,
        /// Caller-generated idempotency identity.
        #[ts(type = "string")]
        request_id: crate::wallet::PayoutRequestId,
    },
    /// Read one durable payout request and its exact native operation.
    PayoutStatus {
        #[ts(type = "string")]
        request_id: crate::wallet::PayoutRequestId,
    },
    /// Await one durable payout request's exact native operation.
    AwaitPayout {
        #[ts(type = "string")]
        request_id: crate::wallet::PayoutRequestId,
    },
    /// Every seat the fleet knows, including decommissioned ones.
    ListSeats,
    /// One seat's durable facts plus its live report.
    SeatStatus {
        #[ts(type = "SeatId")]
        seat_id: SeatId,
    },
    /// Terminal, idempotent operator decommission: stops the child and frees
    /// capacity while retaining the lifetime port allocation.
    DecommissionSeat {
        #[ts(type = "SeatId")]
        seat_id: SeatId,
    },
    /// Rotate the FMan-wide telemetry capability and immediately schedule a
    /// fresh verified registration without returning the bearer.
    ReenrollTelemetry,
    /// Guardian-fee revenue for one seat's federation: the account payers
    /// remit to, current balances, and recent remittances with their
    /// breakdown.
    GuardianFees {
        #[ts(type = "SeatId")]
        seat_id: SeatId,
        #[ts(type = "number | null")]
        limit: Option<u64>,
    },
    /// Move everything remitted into one seat's guardian-fee account out of
    /// the pool. Locked deposits leave at the next cycle turnover, so this
    /// reports what it could take rather than promising an empty account.
    CollectGuardianFees {
        #[ts(type = "SeatId")]
        seat_id: SeatId,
    },
    /// Sweep collected guardian-fee ecash through an automatically selected gateway.
    SweepGuardianFees {
        #[ts(type = "SeatId")]
        seat_id: SeatId,
        /// Caller-generated idempotency identity.
        #[ts(type = "string")]
        request_id: crate::wallet::PayoutRequestId,
    },
    /// Identity material the operator needs for onboarding (registry
    /// listing, holder authorization).
    Onboarding,
    /// Schedule one bounded relay fetch that verifies and durably retains any
    /// Holder authorizations addressed to this FMan.
    RefreshHolderAuthorizations,
    /// Finish first-run setup with the admission limit and initial price. This
    /// is accepted only after a Holder authorization has been retained.
    ConfigureInitialOffer {
        max_seats: u32,
        #[ts(type = "number | null")]
        price_msats: Option<u64>,
    },
    /// The root mnemonic phrase, for the operator's backup. Returned to
    /// the connected operator only — never logged, never persisted
    /// anywhere but the identity database it came from.
    ShowMnemonic,
    /// Onboard this host as a Fleet Manager that has never existed before:
    /// generate the root mnemonic and start with no seats.
    ///
    /// Answered by a daemon that has not been onboarded
    /// ([`crate::onboarding`]); a running fleet refuses it unless the caller
    /// said an already-onboarded host is an acceptable answer.
    OnboardAsNew {
        /// Whether "this host is already onboarded" is success rather than a
        /// refusal. Set by orchestrators whose want is *ensure onboarded* —
        /// they restart a daemon on a data root that may already have an
        /// identity, and the alternative is reading the refusal message,
        /// which makes a log line into a protocol.
        if_needed: bool,
    },
    /// Onboard this host as the recovery of an existing Fleet Manager, from
    /// its root mnemonic (SPEC-nostr-backup-restore, *Restore is onboarding*).
    ///
    /// The other half of the same choice, and answered under the same
    /// condition: a Fleet Manager is set up once.
    OnboardFromBackup {
        /// The phrase, read from the operator's own backup by the CLI. Never
        /// logged, and never written anywhere but the identity table.
        mnemonic: String,
        /// The operator's assertion that the guardians being restored are
        /// permanently offline. Two hosts running one guardian identity
        /// equivocate, and no state the daemon can observe answers this.
        acknowledge_original_host_is_gone: bool,
    },
}

/// Which operator vocabulary both listeners answer from right now.
#[derive(Clone)]
pub(crate) enum Phase {
    /// No identity yet: the two onboarding verbs, and `not_onboarded` for
    /// everything else.
    Onboarding(Arc<crate::onboarding::Onboarding>),
    /// The fleet is open, so the complete surface is served.
    Fleet {
        fleet: Arc<Fleet>,
        directory: tokio::sync::watch::Receiver<DirectoryPresence>,
    },
}

/// The phase both operator transports answer from, switchable while they
/// serve. Nothing waits for the switch, so this is a plain shared cell, not a
/// channel.
#[derive(Clone)]
pub struct OperatorPhase(Arc<std::sync::Mutex<Phase>>);

impl OperatorPhase {
    /// Operator surfaces for a host with no identity yet.
    pub fn onboarding(onboarding: Arc<crate::onboarding::Onboarding>) -> Self {
        Self(Arc::new(std::sync::Mutex::new(Phase::Onboarding(
            onboarding,
        ))))
    }

    /// Operator surfaces for a host whose fleet was already open at binding.
    pub fn fleet(
        fleet: Arc<Fleet>,
        directory: tokio::sync::watch::Receiver<DirectoryPresence>,
    ) -> Self {
        Self(Arc::new(std::sync::Mutex::new(Phase::Fleet {
            fleet,
            directory,
        })))
    }

    /// Switch every serving operator transport to the newly opened fleet.
    pub fn open_fleet(
        &self,
        fleet: Arc<Fleet>,
        directory: tokio::sync::watch::Receiver<DirectoryPresence>,
    ) {
        *self.0.lock().expect("a phase writer panicked") = Phase::Fleet { fleet, directory };
    }

    /// Answer one operator request from the phase current when it arrived.
    /// This is the one dispatcher; both transports are adapters onto it.
    pub(crate) async fn answer(&self, request: AdminRequest) -> anyhow::Result<Value> {
        match self.sample() {
            Phase::Onboarding(onboarding) => onboarding.answer(request).await,
            Phase::Fleet { fleet, directory } => {
                // Sampled once per request: the answer is what the directory
                // runtime had last published when the operator asked, never a
                // value it goes on to fetch.
                let directory = directory.borrow().clone();
                dispatch(&fleet, &directory, request).await
            }
        }
    }

    /// Clone the cheap `Arc`-built phase out; the lock is never held across
    /// an await.
    fn sample(&self) -> Phase {
        self.0.lock().expect("a phase writer panicked").clone()
    }
}

/// Serve the admin socket until the daemon exits. Replaces a stale socket
/// file from a previous run — the data-root flock already proved no other
/// daemon is alive.
pub fn serve(phase: &OperatorPhase, path: &Path) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let listener = bind(path)?;
    let phase = phase.clone();
    Ok(tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let phase = phase.clone();
                    tokio::spawn(async move {
                        // Transport errors surface in the local CLI.
                        let _ = answer_one(stream, |request| async move {
                            Ok((phase.answer(request).await?, ()))
                        })
                        .await;
                    });
                }
                Err(err) => {
                    tracing::warn!(?err, "admin socket accept failed");
                    tracing::warn!(
                        safe_to_share = true,
                        stage = "admin_socket",
                        failure_kind = "accept_failed",
                        "admin socket accept failed"
                    );
                }
            }
        }
    }))
}

/// Bind the admin socket, owner-only.
///
/// The daemon's exclusive data-root flock is what guarantees a single server,
/// so a stale socket file from a previous run is removed rather than treated as
/// a conflict.
fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)
        .with_context(|| format!("bind admin socket {}", path.display()))?;
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .context("restrict admin socket to owner")?;
    Ok(listener)
}

/// Why a request failed: the operator's sentence, plus the discriminant a
/// program branches on.
///
/// `message` is unchanged and still the only thing the CLI prints. `kind`
/// exists because the browser setup wizard has to *choose* a recovery action —
/// retype the phrase, remove a seat directory, upgrade the build — and matching
/// prose to do it made rewording an error a breaking change.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdminError {
    pub kind: AdminErrorKind,
    pub message: String,
}

/// The closed set of refusals a consumer may branch on.
///
/// `Other` is the honest default: a failure that has no distinct operator
/// action gets no discriminant, rather than a new one nobody handles. Adding a
/// variant here is a contract change — mirror it in
/// `operator-ui/packages/types/src/fleet.ts` and regenerate the fixtures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminErrorKind {
    /// The request line did not deserialize as an `AdminRequest`.
    UnparsableRequest,
    /// This host has no identity yet, so the verb has no fleet to run
    /// against. The browser setup wizard opens on exactly this: it is how a
    /// consumer tells "not set up" from "not answering".
    NotOnboarded,
    /// A host is set up once, and this one already is.
    AlreadyOnboarded,
    /// The phrase offered to `OnboardFromBackup` is not a mnemonic.
    InvalidMnemonic,
    /// `OnboardFromBackup` was called without the operator's assertion that
    /// the original guardians are permanently offline.
    RestoreNotAcknowledged,
    /// The mnemonic published a backup document this build cannot read.
    UnreadableBackupDocument,
    /// A seat directory left by an interrupted restore blocks this one.
    SeatDirectoryExists,
    /// A formed seat's guardian archive is not on the relays.
    MissingGuardianArchive,
    /// Everything else.
    Other,
}

impl AdminError {
    /// Classify a failure on its way to the wire.
    ///
    /// The downcast reads the whole `anyhow` chain, so a `RestoreError` keeps
    /// its discriminant through the `?` that boxed it and through any context
    /// added above it.
    pub(crate) fn from_error(error: &anyhow::Error) -> Self {
        let kind = if error.is::<crate::onboarding::NotOnboarded>() {
            AdminErrorKind::NotOnboarded
        } else {
            error
                .downcast_ref::<crate::restore::RestoreError>()
                .map_or(AdminErrorKind::Other, crate::restore::RestoreError::kind)
        };
        Self {
            kind,
            message: format!("{error:#}"),
        }
    }

    fn unparsable(error: &serde_json::Error) -> Self {
        Self {
            kind: AdminErrorKind::UnparsableRequest,
            message: format!("unparsable admin request: {error}"),
        }
    }
}

/// The socket protocol, in one place: read one request object, answer one
/// `Result<Value, AdminError>` line, close.
///
/// `handle` returns the response value together with whatever the caller wants
/// out of a successful request — `()` for the running daemon, the chosen
/// identity for onboarding. It is returned only after the response line has
/// been written, so an operator always sees the answer before the daemon acts
/// on it. `Ok(None)` is a connection that closed without asking anything, and a
/// refused or unparsable request, both of which the caller may ignore.
pub(crate) async fn answer_one<T, F, Fut>(
    stream: UnixStream,
    handle: F,
) -> anyhow::Result<Option<T>>
where
    F: FnOnce(AdminRequest) -> Fut,
    Fut: Future<Output = anyhow::Result<(Value, T)>>,
{
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut request = None;
    while let Some(line) = lines.next_line().await? {
        if !line.trim().is_empty() {
            request = Some(line);
            break;
        }
    }
    let Some(line) = request else {
        return Ok(None);
    };
    let (response, answered): (Result<Value, AdminError>, Option<T>) =
        match serde_json::from_str(&line) {
            Ok(request) => match handle(request).await {
                Ok((value, answered)) => (Ok(value), Some(answered)),
                Err(err) => (Err(AdminError::from_error(&err)), None),
            },
            Err(err) => (Err(AdminError::unparsable(&err)), None),
        };
    let mut line = serde_json::to_string(&response)?;
    line.push('\n');
    write.write_all(line.as_bytes()).await?;
    Ok(answered)
}

/// Read what a verb asks for off the fleet, then hand the values to the
/// matching response shaper below.
///
/// The split is deliberate: every response body is produced by a named, pure
/// `*_json` function, so the committed contract fixtures
/// (`tests/support/contract_fixtures.rs`) are encoded by the same code the
/// daemon answers with rather than by a second description of it.
pub(crate) async fn dispatch(
    fleet: &Fleet,
    directory: &DirectoryPresence,
    request: AdminRequest,
) -> anyhow::Result<Value> {
    match request {
        AdminRequest::ShowPlans => Ok(plans_json(fleet.offered_plans().await)),
        AdminRequest::SetPrice { price_msats } => {
            fleet.set_offered_price(price_msats.map(Msats)).await?;
            Ok(plans_json(fleet.offered_plans().await))
        }
        AdminRequest::ShowCapacity => Ok(capacity_json(
            fleet.max_seats().await,
            fleet.available_slots().await,
        )),
        AdminRequest::SetCapacity { max_seats } => {
            fleet.set_max_seats(max_seats).await?;
            Ok(capacity_json(
                fleet.max_seats().await,
                fleet.available_slots().await,
            ))
        }
        AdminRequest::ListPaymentFederations => Ok(payment_federations_json(
            fleet.payment_federation_statuses().await,
        )),
        AdminRequest::PayoutDestination => {
            Ok(payout_destination_json(fleet.payout_destination().await?))
        }
        AdminRequest::SetPayoutDestination { destination } => {
            fleet.set_payout_destination(destination.as_deref()).await?;
            Ok(payout_destination_json(fleet.payout_destination().await?))
        }
        AdminRequest::SweepPaymentFees {
            federation_id,
            request_id,
        } => Ok(serde_json::to_value(
            fleet
                .payout_payment_fees(&federation_id, &request_id)
                .await?,
        )?),
        AdminRequest::PayoutStatus { request_id } => Ok(serde_json::to_value(
            fleet.payout_job_status(&request_id).await?,
        )?),
        AdminRequest::AwaitPayout { request_id } => Ok(serde_json::to_value(
            fleet.await_payout_job(&request_id).await?,
        )?),
        AdminRequest::ListSeats => Ok(seats_json(
            fleet.seat_summaries().await?,
            fleet.backup_scan(),
        )),
        AdminRequest::SeatStatus { seat_id } => {
            let Some((summary, report)) = fleet.admin_seat_status(&seat_id).await? else {
                anyhow::bail!("unknown seat");
            };
            Ok(seat_status_json(
                summary,
                report,
                read_seat_guardian_fee(fleet, &seat_id).await,
            ))
        }
        AdminRequest::DecommissionSeat { seat_id } => Ok(decommission_seat_json(
            fleet.decommission_seat(&seat_id).await?,
        )),
        AdminRequest::ReenrollTelemetry => {
            fleet.reenroll_telemetry().await?;
            Ok(reenroll_telemetry_json())
        }
        AdminRequest::GuardianFees { seat_id, limit } => {
            let status = fleet.guardian_fee_status(&seat_id).await?;
            // A decommissioned guardian has no live consensus connection, but
            // its retained fee pool and wallet remain operator-drainable.
            // Policy is therefore a best-effort diagnostic, not a gate on the
            // monetary state in this response.
            let policy = fleet.guardian_fee_policy(&seat_id).await;
            let remittances = fleet
                .guardian_fee_remittances(&seat_id, limit.unwrap_or(20))
                .await?;
            let wallet = fleet.guardian_fee_drain_status(&seat_id).await?;
            Ok(guardian_fees_json(
                &seat_id,
                &status,
                fleet.guardian_fee_account(&seat_id)?,
                wallet,
                fleet.guardian_fee_total_remitted(&seat_id).await?.msats,
                policy.as_ref().map_err(|err| format!("{err:#}")),
                remittances,
            ))
        }
        AdminRequest::CollectGuardianFees { seat_id } => Ok(collect_guardian_fees_json(
            fleet.guardian_fee_collect(&seat_id).await?,
        )),
        AdminRequest::SweepGuardianFees {
            seat_id,
            request_id,
        } => Ok(serde_json::to_value(
            fleet.payout_guardian_fees(&seat_id, &request_id).await?,
        )?),
        AdminRequest::Onboarding => Ok(onboarding_json(
            &fleet.identity().derive_service_pubkey().to_string(),
            directory,
            &env!("CARGO_PKG_VERSION")
                .parse::<FmanVersion>()
                .expect("workspace package version is valid SemVer"),
        )),
        AdminRequest::RefreshHolderAuthorizations => {
            Err(crate::restore::RestoreError::AlreadyOnboarded.into())
        }
        AdminRequest::ConfigureInitialOffer { .. } => {
            Err(crate::restore::RestoreError::AlreadyOnboarded.into())
        }
        AdminRequest::ShowMnemonic => Ok(mnemonic_json(&fleet.identity().phrase())),
        // Onboarding happened before this fleet existed, and happens once.
        AdminRequest::OnboardAsNew { if_needed: true } => Ok(onboarded_already_json()),
        // The same refusal the identity stage raises, so it reaches a
        // consumer as the same discriminant. Which phase answered is not an
        // operator action.
        AdminRequest::OnboardAsNew { .. } | AdminRequest::OnboardFromBackup { .. } => {
            Err(crate::restore::RestoreError::AlreadyOnboarded.into())
        }
    }
}

/// `ShowPlans` and `SetPrice` answer the same view, so a write needs no
/// follow-up read.
pub fn plans_json(plans: Vec<Plan>) -> Value {
    json!({ "plans": plans })
}

pub fn capacity_json(max_seats: u32, available_slots: u32) -> Value {
    json!({ "max_seats": max_seats, "available_slots": available_slots })
}

pub fn payment_federations_json(statuses: Vec<PaymentFederationStatus>) -> Value {
    let federations: Vec<Value> = statuses.into_iter().map(payment_federation_json).collect();
    json!({ "federations": federations })
}

/// Wallet amounts retain their distinct economic meanings; in particular,
/// encumbered outgoing value is never added to available ecash.
fn payment_federation_json(status: PaymentFederationStatus) -> Value {
    json!({
        "federation_id": status.federation_id,
        "accepted": status.accepted,
        "receivable": status.receivable,
        "wallet": status.wallet,
    })
}

/// Both `PayoutDestination` and `SetPayoutDestination` answer with the stored
/// destination, or null for "no destination configured".
pub fn payout_destination_json(destination: Option<String>) -> Value {
    json!({ "destination": destination })
}

pub fn seats_json(
    summaries: Vec<SeatSummary>,
    backup_scan: Option<crate::backup_worker::BackupScanOutcome>,
) -> Value {
    let seats: Vec<Value> = summaries.into_iter().map(summary_json).collect();
    let backup_scan = backup_scan.map(|scan| {
        json!({
            "completed_at_ms": scan.completed_at_ms,
            "pending_seats": scan.pending_seats,
        })
    });
    json!({ "seats": seats, "backup_scan": backup_scan })
}

/// One seat's list entry, widened with the live report and the fee summary the
/// list deliberately omits.
pub fn seat_status_json(summary: SeatSummary, report: SeatReport, guardian_fee: Value) -> Value {
    let mut value = summary_json(summary);
    value["report"] = report_json(report);
    value["guardian_fee"] = guardian_fee;
    value
}

/// Decommissioning is idempotent, so the answer states both that the seat is
/// decommissioned and whether this call is what did it.
pub fn decommission_seat_json(newly_decommissioned: bool) -> Value {
    json!({ "decommissioned": true, "already_decommissioned": !newly_decommissioned })
}

/// The rotated bearer is deliberately absent from the operator response.
pub fn reenroll_telemetry_json() -> Value {
    json!({ "telemetry_reenrollment": "scheduled" })
}

pub fn guardian_fees_json(
    seat_id: &SeatId,
    status: &FederationFeeStatus,
    remittance_account: String,
    wallet: crate::payout_wire::WalletDrainStatusWire,
    lifetime_remitted_msat: u64,
    policy: Result<&FeePolicy, String>,
    remittances: Vec<Remittance>,
) -> Value {
    let policy = match policy {
        Ok(policy) => json!({
            "configured": policy.configured,
            "send_ppm": policy.send_ppm,
            "recipients": policy.recipients,
            "share_matches_policy": policy.share_matches_policy(),
            "our_weight": policy.our_share.map(|(ours, _)| ours),
            "total_weight": policy.our_share.map(|(_, total)| total),
        }),
        Err(error) => json!({ "error": error }),
    };
    json!({
        "seat_id": seat_id,
        "federation_id": status.federation_id.to_string(),
        "remittance_account": remittance_account,
        "collectable_msat": status.collectable().msats,
        "staged_msat": status.staged.msats,
        "locked_msat": status.locked.msats,
        "idle_msat": status.idle.msats,
        "wallet": wallet,
        "lifetime_remitted_msat": lifetime_remitted_msat,
        "policy": policy,
        "remittances": remittances.into_iter().map(remittance_json).collect::<Vec<_>>(),
    })
}

/// Locked deposits leave at the next cycle turnover, so a collection reports
/// what it could take rather than promising an empty account.
pub fn collect_guardian_fees_json(collected: Collected) -> Value {
    match collected {
        Collected::Complete {
            claimed,
            awaiting_cycle,
        } => json!({
            "claimed_msat": claimed.msats,
            "awaiting_cycle_msat": awaiting_cycle.msats,
        }),
        Collected::Incomplete {
            confirmed_claimed,
            observed_awaiting_cycle,
            failure,
        } => {
            let (phase, action) = match failure.phase {
                CollectionFailurePhase::IdleClaim => ("idle_claim", "idle-balance claim"),
                CollectionFailurePhase::Unlock => ("unlock", "unlock"),
                CollectionFailurePhase::BalanceRefresh => ("balance_refresh", "balance refresh"),
            };
            let message = if failure.operation_submitted {
                format!(
                    "guardian-fee {action} was submitted but did not complete; refresh status \
                     before retrying"
                )
            } else if failure.phase == CollectionFailurePhase::BalanceRefresh {
                "guardian-fee operations completed but the updated balance could not be read; \
                 refresh status before retrying"
                    .to_owned()
            } else {
                format!("guardian-fee {action} could not be submitted; collection stopped")
            };
            json!({
                "claimed_msat": confirmed_claimed.msats,
                "awaiting_cycle_msat": observed_awaiting_cycle.map(|amount| amount.msats),
                "incomplete": {
                    "phase": phase,
                    "operation_submitted": failure.operation_submitted,
                    "error": message,
                },
            })
        }
    }
}

/// The two identities an operator needs for onboarding, plus what the
/// directory last observed about who may authorize this FMan.
pub fn onboarding_json(
    service_pubkey: &str,
    directory: &DirectoryPresence,
    current_fman_version: &FmanVersion,
) -> Value {
    let update_required = directory
        .latest_fman_version
        .as_ref()
        .is_some_and(|latest| current_fman_version < latest);
    // `checked_at` is present on every state that has one, and absent on the
    // two that do not: a dashboard must be able to say "not checked yet"
    // without reading a zero as a timestamp.
    let status = match &directory.onboarding {
        OnboardingStatus::Checking => json!({ "state": "checking" }),
        OnboardingStatus::NotObserved { checked_at } => json!({
            "state": "not_observed",
            "checked_at": checked_at,
        }),
        OnboardingStatus::AuthorizationObserved {
            authorizations,
            holders,
            checked_at,
        } => json!({
            "state": "authorization_observed",
            "authorizations": authorizations,
            "holders": holders.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "checked_at": checked_at,
        }),
        OnboardingStatus::RelayError { error } => json!({
            "state": "relay_error",
            "error": error,
        }),
    };
    json!({
        "stage": "complete",
        "runtime": "ready",
        "fman_name": FmanName::from_fman_id(directory.service_nostr_pubkey).to_string(),
        "service_pubkey": service_pubkey,
        "service_nostr_pubkey": directory.service_nostr_pubkey.to_string(),
        "nostr": status,
        "fman_version": {
            "current": current_fman_version.to_string(),
            "latest": directory.latest_fman_version.as_ref().map(ToString::to_string),
            "update_required": update_required,
        },
    })
}

pub fn mnemonic_json(phrase: &str) -> Value {
    json!({ "mnemonic": phrase })
}

/// What a running fleet answers `OnboardAsNew { if_needed: true }` with.
pub fn onboarded_already_json() -> Value {
    json!({ "onboarded": "already" })
}

/// Whether a seat is still being paid, reported with the seat rather than
/// only behind `GuardianFees`.
///
/// The daemon does not withdraw service when a federation stops paying — that
/// is the operator's decision
/// ([REQ-guardian-fee-remittance](../../../../specs/REQ-guardian-fee-remittance.md))
/// — and an operator who has to remember a second verb to notice being cut out
/// will not notice. Reading it is best-effort: a seat with no federation yet,
/// or one whose metadata cannot be read, reports why instead of failing the
/// whole status.
pub(crate) async fn read_seat_guardian_fee(fleet: &Fleet, seat_id: &SeatId) -> Value {
    match fleet.guardian_fee_account(seat_id) {
        // Without an account there is nothing to report a policy against, so
        // the policy is not read at all.
        Err(err) => seat_guardian_fee_error_json(&format!("{err:#}")),
        Ok(account) => seat_guardian_fee_json(account, fleet.guardian_fee_policy(seat_id).await),
    }
}

/// No account could be derived — a strictly different fact from an account
/// whose policy could not be read, and a different JSON shape.
pub fn seat_guardian_fee_error_json(error: &str) -> Value {
    json!({ "error": error })
}

/// The account exists; the policy read behind it may still have failed.
pub fn seat_guardian_fee_json(
    remittance_account: String,
    policy: anyhow::Result<FeePolicy>,
) -> Value {
    let mut value = json!({ "remittance_account": remittance_account });
    match policy {
        Ok(policy) => {
            value["share_matches_policy"] = json!(policy.share_matches_policy());
            value["send_ppm"] = json!(policy.send_ppm);
            value["our_weight"] = json!(policy.our_share.map(|(ours, _)| ours));
            value["total_weight"] = json!(policy.our_share.map(|(_, total)| total));
        }
        // Before DKG there is no federation to carry metadata, and an
        // unreadable value is not the same fact as an exclusion.
        Err(err) => value["policy_error"] = json!(format!("{err:#}")),
    }
    value
}

/// A remittance whose breakdown does not open is still money we were paid,
/// so the amount is reported either way and the failure is shown rather than
/// swallowed.
pub fn remittance_json(remittance: Remittance) -> Value {
    let mut value = json!({
        "amount_msat": remittance.amount.msats,
        "txid": remittance.txid,
    });
    match remittance.metadata {
        Ok(metadata) => {
            value["remitted_at_unix"] = json!(metadata.remitted_at_unix);
            value["total_msat"] = json!(metadata.total_msats);
            value["breakdown"] = json!(
                metadata
                    .breakdown
                    .into_iter()
                    .map(|item| json!({
                        "module": item.module,
                        "direction": item.direction,
                        "amount_msat": item.amount_msats,
                    }))
                    .collect::<Vec<_>>()
            );
        }
        Err(err) => value["breakdown_error"] = json!(err),
    }
    value
}

pub fn summary_json(summary: SeatSummary) -> Value {
    let payment_claim = match summary.payment_claim {
        PaymentClaimStatus::NotPaid => json!({ "state": "not_paid" }),
        PaymentClaimStatus::Pending => json!({ "state": "pending" }),
        PaymentClaimStatus::Success { at_ms } => json!({ "state": "success", "at_ms": at_ms }),
        PaymentClaimStatus::AlreadySpent { at_ms } => {
            json!({ "state": "already_spent", "at_ms": at_ms })
        }
    };
    let backup = summary.backup.map(|backup| {
        json!({
            "published_at_ms": backup.published_at_ms,
            "archive_confirmed": backup.archive_confirmed,
        })
    });
    json!({
        "seat_id": summary.seat_id.to_string(),
        "fi_id": summary.fi_id,
        "plan": summary.plan,
        "created_at_ms": summary.created_at_ms,
        "payment_claim": payment_claim,
        "decommissioned": summary.decommissioned,
        "completion_callback": completion_callback_json(summary.completion_callback),
        "backup": backup,
    })
}

pub fn completion_callback_json(status: CompletionCallbackStatus) -> Value {
    match status {
        CompletionCallbackStatus::NotConfigured => json!({ "state": "not_configured" }),
        CompletionCallbackStatus::Pending {
            attempts,
            next_attempt_at_ms,
            last_reason,
        } => json!({
            "state": "pending",
            "attempts": attempts,
            "next_attempt_at_ms": next_attempt_at_ms,
            "last_reason": last_reason.map(|reason| reason.as_str()),
        }),
        CompletionCallbackStatus::OperatorBlocked { attempts, reason } => json!({
            "state": "operator_blocked",
            "attempts": attempts,
            "reason": reason.as_str(),
        }),
        CompletionCallbackStatus::Delivered { attempts, at_ms } => json!({
            "state": "delivered",
            "attempts": attempts,
            "at_ms": at_ms,
        }),
        CompletionCallbackStatus::Terminal {
            attempts,
            at_ms,
            reason,
        } => json!({
            "state": "terminal",
            "attempts": attempts,
            "at_ms": at_ms,
            "reason": reason.as_str(),
        }),
    }
}

pub fn report_json(report: SeatReport) -> Value {
    match report {
        SeatReport::Decommissioned { at_ms } => {
            json!({ "state": "decommissioned", "at_ms": at_ms })
        }
        SeatReport::Active { phase, health } => {
            let health = match health {
                SeatHealth::Healthy => "healthy",
                SeatHealth::Unavailable => "unavailable",
                SeatHealth::Failed => "failed",
            };
            let phase = match phase {
                SeatPhase::Created => json!({ "phase": "created" }),
                SeatPhase::DkgInProgress => json!({ "phase": "dkg_in_progress" }),
                SeatPhase::DataLoss { invite_code } => json!({
                    "phase": "data_loss",
                    "invite_code": invite_code,
                }),
                SeatPhase::Running { invite_code } => json!({
                    "phase": "running",
                    "invite_code": invite_code,
                }),
            };
            let mut value = phase;
            value["state"] = json!("active");
            value["health"] = json!(health);
            value
        }
    }
}

/// One request/response round trip from the CLI side.
pub async fn request(
    path: &Path,
    request: &AdminRequest,
) -> anyhow::Result<Result<Value, AdminError>> {
    let stream = UnixStream::connect(path).await.with_context(|| {
        format!(
            "connect to admin socket {} (is the daemon running?)",
            path.display()
        )
    })?;
    let (read, mut write) = stream.into_split();
    let mut line = serde_json::to_string(request)?;
    line.push('\n');
    write.write_all(line.as_bytes()).await?;
    let mut lines = BufReader::new(read).lines();
    let line = lines
        .next_line()
        .await?
        .ok_or_else(|| anyhow::anyhow!("admin socket closed without a response"))?;
    serde_json::from_str(&line).context("unparsable admin response")
}

#[cfg(test)]
#[path = "../tests/admin.rs"]
mod tests;
