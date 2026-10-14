//! Crash-safe Manifold Guardian formation and payment-reservation driver.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::str::FromStr as _;
use std::time::Duration;

use fedi_decentralized_domain::{
    MG_SEAT_BINDINGS_META_FIELD_KEY, MgSeatBindings, federation_seats,
};
use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_service_manifold_guardian::*;
use fedimint_core::config::{ClientConfig, FederationId as FedimintFederationId};
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::runtime::{Instant, sleep, timeout};
use futures::stream::{FuturesUnordered, StreamExt as _};
use sha2::{Digest as _, Sha256};
use stability_pool_common::Account;
use tokio::sync::OnceCell;

use crate::db::{
    AdmissionEffect, DriverLease, FaStore, FormationMetaTarget, FormationRecovery, InitialSeat,
    MgAdmission, NewCreation, PurchaseRecovery, QuoteAuthorization, StoredVerifierProvenance,
};
use crate::meta_write::{MetaWriteError, SeatVote, write_consensus_meta};
use crate::mg::MgClient;
use crate::ports::FaIdentityExt as _;
use crate::selection::AvailabilityMismatch;
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FederationConsensusReader,
    FederationConsensusSnapshot, FormationFreshness, FormationId, FormationPhase,
    FormationSnapshot, GuardianFeePpm, ManifoldGuardianConnector, MgCause, MgFailure, MgOperation,
    MgSelectionPreview, MgSelectionRequest, PaymentRequirements, PaymentReservationId,
    PaymentReservationRecovery, PurchasePhase, ResolvedSeatPurchase, SeatPaymentRecovery,
    SeatPhase, SeatPurchaseSnapshot, SelectionReauthorizationReason,
};

fn log_mg_failure(formation_id: &FormationId, error: &FaError, outcome: &'static str) {
    let failure = match error {
        FaError::Mg(failure)
        | FaError::SelectionReauthorizationRequired {
            failure: Some(failure),
            ..
        } => failure,
        _ => return,
    };
    tracing::warn!(
        safe_to_share = true,
        formation_id = %formation_id.0,
        mg_pubkey = %failure.mg,
        operation = %failure.operation,
        cause = %failure.cause,
        outcome,
        "formation request failed"
    );
}

/// Smallest duration represented without truncation by native and WASM runtime timers.
pub(crate) const MIN_RUNTIME_TIMER_DURATION: Duration = Duration::from_millis(1);
/// Largest duration represented without overflow by native and WASM runtime timers.
pub(crate) const MAX_RUNTIME_TIMER_DURATION: Duration = Duration::from_millis(i32::MAX as u64);
const SELECTED_MG_CONNECT_RETRY_BUDGET: Duration = Duration::from_secs(2 * 60);
const SELECTED_MG_CONNECT_RETRY_MIN_DELAY: Duration = Duration::from_millis(100);
const SELECTED_MG_CONNECT_RETRY_MAX_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuoteAttemptPolicy {
    /// The verified row has not crossed its output/acceptance boundary, so a
    /// definite transport failure may be retried and eventually returned as
    /// a fresh-selection requirement.
    ReplaceableSelection,
    /// The row is already value-bound; failures remain exact
    /// recovery errors and never require a fresh selection.
    ExactRecovery,
}

impl QuoteAttemptPolicy {
    fn allows_selection_reauthorization(self) -> bool {
        self == Self::ReplaceableSelection
    }
}

fn quote_attempt_policy(admission: &MgAdmission) -> QuoteAttemptPolicy {
    if admission.requires_effect_authorization() {
        QuoteAttemptPolicy::ReplaceableSelection
    } else {
        QuoteAttemptPolicy::ExactRecovery
    }
}

/// One field of [`FormationRunOptionsConfig`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormationTimingField {
    /// Probe interval.
    PollInterval,
    /// Per-invocation run timeout.
    RunTimeout,
    /// Per-capability request timeout.
    RequestTimeout,
}

impl std::fmt::Display for FormationTimingField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PollInterval => "poll interval",
            Self::RunTimeout => "run timeout",
            Self::RequestTimeout => "request timeout",
        })
    }
}

/// Reason that formation timing options could not be constructed.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InvalidFormationRunOptions {
    /// A timer would truncate below the shared runtime quantum.
    #[error("invalid formation options: formation {field} must be at least one millisecond")]
    BelowMinimum {
        /// Invalid configuration field.
        field: FormationTimingField,
    },
    /// A timer exceeds the shared native/WASM representation.
    #[error("invalid formation options: formation {field} exceeds the runtime timer range")]
    AboveMaximum {
        /// Invalid configuration field.
        field: FormationTimingField,
    },
    /// A duration would be truncated by the WASM millisecond timer.
    #[error("invalid formation options: formation {field} must be an integral millisecond value")]
    NonIntegral {
        /// Invalid configuration field.
        field: FormationTimingField,
    },
    /// A derived durable lease duration overflowed.
    #[error("invalid formation options: formation deadline is too large")]
    LeaseOverflow,
    /// A runtime monotonic deadline could not represent the value.
    #[error("invalid formation options: formation {field} is too large")]
    DeadlineOverflow {
        /// Invalid configuration field.
        field: FormationTimingField,
    },
}

/// Timing bounds for one formation driver/API invocation.
#[derive(Clone, Copy, Debug)]
pub struct FormationRunOptions {
    /// Runtime-representable delay between child-readiness and running-status probes.
    poll_interval: Duration,
    /// Runtime-representable maximum elapsed time for one driver/API invocation.
    run_timeout: Duration,
    /// Runtime-representable maximum time for one ordinary capability call.
    request_timeout: Duration,
    /// Precomputed maximum durable lease duration.
    lease_duration: Duration,
    /// Precomputed durable lease renewal duration.
    lease_renewal_duration: Duration,
}

/// Named inputs for checked formation timing bounds.
pub struct FormationRunOptionsConfig {
    /// Delay between child-readiness and running-status probes.
    pub poll_interval: Duration,
    /// Maximum elapsed time for one driver/API invocation.
    pub run_timeout: Duration,
    /// Maximum time for one ordinary capability call.
    /// Payment creation, recovery, and refund settlement use the remaining run timeout.
    pub request_timeout: Duration,
}

impl Default for FormationRunOptionsConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(2),
            run_timeout: Duration::from_secs(600),
            request_timeout: Duration::from_secs(30),
        }
    }
}

impl FormationRunOptions {
    /// Construct timing options that are valid on native and WASM runtimes.
    ///
    /// Every value must be an integral number of milliseconds in the inclusive
    /// 1..=2,147,483,647 range and fit the derived lease sums. Invalid values return
    /// [`InvalidFormationRunOptions`] before lease, durable-state, wallet, or
    /// RPC effects.
    pub fn new(config: FormationRunOptionsConfig) -> Result<Self, InvalidFormationRunOptions> {
        let timings = [
            (FormationTimingField::PollInterval, config.poll_interval),
            (FormationTimingField::RunTimeout, config.run_timeout),
            (FormationTimingField::RequestTimeout, config.request_timeout),
        ];
        for (name, duration) in timings {
            if duration < MIN_RUNTIME_TIMER_DURATION {
                return Err(InvalidFormationRunOptions::BelowMinimum { field: name });
            }
            if MAX_RUNTIME_TIMER_DURATION < duration {
                return Err(InvalidFormationRunOptions::AboveMaximum { field: name });
            }
            if !duration.subsec_nanos().is_multiple_of(1_000_000) {
                return Err(InvalidFormationRunOptions::NonIntegral { field: name });
            }
        }
        let lease_duration = config
            .run_timeout
            .checked_add(Duration::from_secs(60))
            .ok_or(InvalidFormationRunOptions::LeaseOverflow)?;
        let lease_renewal_duration = config
            .request_timeout
            .min(config.run_timeout)
            .checked_add(Duration::from_secs(60))
            .ok_or(InvalidFormationRunOptions::LeaseOverflow)?;
        let now = Instant::now();
        for (name, duration) in timings {
            now.checked_add(duration)
                .ok_or(InvalidFormationRunOptions::DeadlineOverflow { field: name })?;
        }
        Ok(Self {
            poll_interval: config.poll_interval,
            run_timeout: config.run_timeout,
            request_timeout: config.request_timeout,
            lease_duration,
            lease_renewal_duration,
        })
    }

    /// Return the per-capability request timeout.
    pub(crate) fn request_timeout(self) -> Duration {
        self.request_timeout
    }

    /// Return the configured retry/readback interval.
    pub(crate) fn poll_interval(self) -> Duration {
        self.poll_interval
    }

    /// Return the maximum persisted lease horizon.
    pub(crate) fn lease_duration(self) -> Duration {
        self.lease_duration
    }

    /// Return the persisted lease renewal duration.
    pub(crate) fn lease_renewal_duration(self) -> Duration {
        self.lease_renewal_duration
    }

    /// Recheck time-dependent runtime and store-clock bounds before any effect.
    pub(crate) fn validate_for_start(self, store: &FaStore) -> FaResult<()> {
        checked_deadline(
            Instant::now(),
            self.run_timeout,
            FormationTimingField::RunTimeout,
        )?;
        store.validate_driver_lease_durations(self.lease_duration, self.lease_renewal_duration)
    }
}

impl Default for FormationRunOptions {
    fn default() -> Self {
        Self::new(FormationRunOptionsConfig::default())
            .expect("default formation timings are valid")
    }
}

#[derive(Clone, Copy)]
/// One bounded driver's deadline, ownership fence, and call timeout policy.
pub(crate) struct DriverRun<'a> {
    options: FormationRunOptions,
    deadline: Instant,
    lease: &'a DriverLease,
}

impl<'a> DriverRun<'a> {
    /// Bind an acquired lease to the absolute deadline established before acquisition.
    pub(crate) fn new(
        options: FormationRunOptions,
        deadline: Instant,
        lease: &'a DriverLease,
    ) -> Self {
        Self {
            options,
            deadline,
            lease,
        }
    }

    /// Return the per-capability timeout for composite fenced operations.
    pub(crate) fn request_timeout(self) -> Duration {
        self.options.request_timeout()
    }

    /// Return this invocation's absolute monotonic deadline.
    pub(crate) fn deadline(self) -> Instant {
        self.deadline
    }

    /// Return the retry/readback interval selected by the caller.
    pub(crate) fn poll_interval(self) -> Duration {
        self.options.poll_interval()
    }

    /// Fence deferred capability construction and polling independently.
    ///
    /// The sequence is monotonic deadline check, backend owner renewal, deferred
    /// construction, a second deadline check and renewal, then polling under the
    /// original absolute deadline.
    pub(crate) async fn call<T, Fut>(
        &self,
        operation: &'static str,
        make_future: impl FnOnce() -> FaResult<Fut>,
    ) -> FaResult<T>
    where
        Fut: Future<Output = T>,
    {
        self.call_with_timeout(operation, self.options.request_timeout(), make_future)
            .await
    }

    async fn call_with_timeout<T, Fut>(
        &self,
        operation: &'static str,
        call_timeout: Duration,
        make_future: impl FnOnce() -> FaResult<Fut>,
    ) -> FaResult<T>
    where
        Fut: Future<Output = T>,
    {
        ensure_effective_time_remaining(self.deadline, operation)?;
        self.lease.renew().await?;
        ensure_effective_time_remaining(self.deadline, operation)?;
        let future = make_future()?;
        ensure_effective_time_remaining(self.deadline, operation)?;
        self.lease.renew().await?;
        let duration = select_timer_duration(
            call_timeout,
            self.deadline.saturating_duration_since(Instant::now()),
            operation,
        )?;
        self.poll_with_lease(operation, duration, future).await
    }

    /// Keep ownership live while a capability is pending, without spawning a
    /// task or extending either its timeout or the invocation deadline.
    async fn poll_with_lease<T>(
        &self,
        operation: &'static str,
        duration: Duration,
        future: impl Future<Output = T>,
    ) -> FaResult<T> {
        timeout(duration, async {
            futures::pin_mut!(future);
            loop {
                tokio::select! {
                    result = &mut future => return Ok(result),
                    _ = sleep(self.options.lease_renewal_duration() / 4) => {
                        self.lease.renew().await?;
                    }
                }
            }
        })
        .await
        .map_err(|_| FaError::Timeout(operation.to_owned()))?
    }

    /// Prepare the timeout budget for a value-moving wallet call.
    ///
    /// Acceptance, spendable change, and signature collection share the remaining
    /// invocation budget rather than the timeout for one network request.
    ///
    /// This refreshes the coarse run guard and captures the call's time budget.
    /// Constructing the future remains effect-free; the wallet body starts only
    /// when `poll_value_call` polls it.
    async fn prepare_value_call_budget(
        &self,
        operation: &'static str,
    ) -> FaResult<ValueCallTimeoutBudget> {
        ensure_effective_time_remaining(self.deadline, operation)?;
        self.lease.renew().await?;
        ensure_effective_time_remaining(self.deadline, operation)?;
        Ok(ValueCallTimeoutBudget {
            operation,
            deadline: self.deadline,
        })
    }

    /// Atomically record the durable output-start boundary.
    async fn arm_payment_outputs_started(
        &self,
        formation_id: &FormationId,
        verifier_provenance: StoredVerifierProvenance,
    ) -> FaResult<()> {
        ensure_effective_time_remaining(self.deadline, "arming seat payment outputs")?;
        self.lease
            .arm_payment_outputs_started(formation_id, verifier_provenance)
            .await
    }

    /// Atomically authorize exact selected effects before their first poll.
    async fn authorize_seat_effects(
        &self,
        formation_id: &FormationId,
        effects: &[(u16, QuoteId, AdmissionEffect)],
        verifier_provenance: StoredVerifierProvenance,
    ) -> FaResult<()> {
        ensure_effective_time_remaining(self.deadline, "authorizing selected-seat effect wave")?;
        self.lease
            .authorize_seat_effects(formation_id, effects, verifier_provenance)
            .await
    }

    /// Construct a synchronous request under the current run guard.
    pub(crate) async fn construct<T>(
        &self,
        operation: &'static str,
        construct: impl FnOnce() -> FaResult<T>,
    ) -> FaResult<T> {
        ensure_effective_time_remaining(self.deadline, operation)?;
        self.lease.renew().await?;
        ensure_effective_time_remaining(self.deadline, operation)?;
        construct()
    }
}

/// Absolute run deadline prepared for one wallet-output poll.
struct ValueCallTimeoutBudget {
    operation: &'static str,
    deadline: Instant,
}

impl ValueCallTimeoutBudget {
    async fn poll_value_call<T>(
        self,
        run: DriverRun<'_>,
        future: impl Future<Output = T>,
    ) -> FaResult<T> {
        // Preflight can precede the atomic output-start boundary and other
        // per-seat preparation. Recompute the relative timer only when this
        // wallet future is about to be polled, so that intervening work cannot
        // extend the absolute formation run deadline.
        ensure_effective_time_remaining(self.deadline, self.operation)?;
        let duration = select_timer_duration(
            run.options.run_timeout,
            self.deadline.saturating_duration_since(Instant::now()),
            self.operation,
        )?;
        run.poll_with_lease(self.operation, duration, future).await
    }
}

pub(crate) struct SeatSession<C> {
    pub(crate) index: u16,
    pub(crate) client: C,
    pub(crate) seat_id: SeatId,
}

/// One guardian's reply to an explicit restart. An error does not prove that
/// its old ceremony survived: the request or acknowledgement may have been lost.
#[derive(Debug)]
pub struct DkgRestartResult {
    /// Index in the saved formation's guardian set.
    pub index: u16,
    /// Identity of the Manifold Guardian that owns this guardian.
    pub mg: secp256k1::XOnlyPublicKey,
    /// `DkgInProcess` acknowledges a new ceremony; `Running` means completion
    /// won the race. Errors may follow a partial change on the guardian.
    pub result: FaResult<ServiceStatus>,
}

#[derive(Clone)]
pub(crate) struct PostFormedSeat {
    pub(crate) index: u16,
    pub(crate) mg_id: nostr_sdk::PublicKey,
    pub(crate) locator: Locator,
    pub(crate) seat_id: SeatId,
}

pub(crate) struct PostFormedAuthority {
    pub(crate) formation_id: FormationId,
    pub(crate) invite_code: InviteCode,
    pub(crate) seats: Vec<PostFormedSeat>,
}

/// One guardian's guarded metadata-vote result.
pub(crate) enum MetaFieldSubmission {
    /// The guardian accepted and submitted this exact base-bound mutation.
    Accepted,
    /// The guardian observed another consensus base before it could submit.
    BaseChanged,
}

/// Failure from the shared guarded metadata-vote primitive.
pub(crate) enum MetaFieldSubmissionError {
    /// Local identity, lease, storage, or request-timeout failure.
    Driver(FaError),
    /// Typed refusal returned by the MG protocol.
    ManifoldGuardian(ManifoldGuardianError),
}

enum SeatCreation<T> {
    Accepted(SeatAcceptance),
    Refused(SeatRefusal<T>),
}

#[derive(Clone)]
struct SeatAcceptance {
    seat_id: SeatId,
    guardian_fee_account: Account,
}

struct FreeSeatQuote {
    signed: SignedResponse<GetQuoteResponse>,
    verified: SignatureVerified<GetQuoteResponse>,
}

struct PaidSeatQuote {
    signed: SignedResponse<GetQuoteResponse>,
    verified: SignatureVerified<GetQuoteResponse>,
}

enum SeatAcquisition<R> {
    Free(FreeSeatQuote),
    ReplayPrepared {
        quote: PaidSeatQuote,
        prepared: crate::PreparedSeatPayment<R>,
    },
}

struct PendingSeatFunding<C, R> {
    position: usize,
    quote: PaidSeatQuote,
    reservation: R,
    timeout_budget: ValueCallTimeoutBudget,
    client: C,
    locator: Locator,
}

struct PendingSeatPresentation<C, R> {
    position: usize,
    client: C,
    locator: Locator,
    source: SeatPresentationSource<R>,
}

enum SeatPresentationSource<R> {
    Existing(SeatAcceptance),
    Acquire(Box<SeatAcquisition<R>>),
}

enum PendingSeatWork<C, Refund, Reservation> {
    Funding(Box<PendingSeatFunding<C, Reservation>>),
    Presentation(PendingSeatPresentation<C, Refund>),
}

struct CompletedSeatWork<C, T> {
    position: usize,
    client: C,
    creation: SeatCreation<T>,
    checkpoint: SeatCheckpoint,
}

enum SeatCheckpoint {
    AlreadyDurable,
    Required,
}

enum SeatPresentation<R> {
    Free(FreeSeatQuote),
    Paid {
        quote: PaidSeatQuote,
        prepared: crate::PreparedSeatPayment<R>,
    },
}

struct SeatRefusal<T> {
    error: MgFailure,
    release_proof: Option<T>,
}

enum TerminalSeatOutcome {
    PaymentRejected { position: usize },
    SeatRefused(MgFailure),
}

impl TerminalSeatOutcome {
    fn into_error(self) -> FaError {
        match self {
            Self::PaymentRejected { position } => FaError::Payment(format!(
                "payment for Manifold Guardian {} was rejected; its quote was cleared",
                position + 1
            )),
            Self::SeatRefused(error) => error.into(),
        }
    }

    /// Classify a proven terminal outcome against the formation's value
    /// boundary. A selected all-free formation has consumed its exact
    /// quote-bound admission but has not armed wallet outputs; re-quoting that
    /// same row would not carry authority for a second presentation. Return
    /// the typed fresh-selection transition so the outer driver abandons the
    /// purchase. Post-output refusals retain their exact recovery behavior.
    fn into_formation_error(self, payment_outputs_started: bool) -> FaError {
        match self {
            Self::SeatRefused(failure) if !payment_outputs_started => {
                FaError::SelectionReauthorizationRequired {
                    reason: SelectionReauthorizationReason::SelectedMgUnavailable,
                    failure: Some(failure),
                }
            }
            outcome => outcome.into_error(),
        }
    }

    fn requires_pre_output_reauthorization(&self, payment_outputs_started: bool) -> bool {
        !payment_outputs_started && matches!(self, Self::SeatRefused(_))
    }
}

fn apply_authorized_effect_in_memory(
    recovery: &mut PurchaseRecovery,
    position: usize,
    quote_id: QuoteId,
    effect: AdmissionEffect,
) -> FaResult<()> {
    let seat = recovery.seats.get_mut(position).ok_or_else(|| {
        FaError::Storage(format!(
            "authorized effect names missing FA seat row {position}"
        ))
    })?;
    seat.admission.mark_effect_authorized(quote_id, effect)?;
    Ok(())
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Start creating a federation by buying the seats of one approved
    /// preview.
    ///
    /// The preview is the approval, valid for five minutes, and must select
    /// every seat of the federation. A missing name asks `fa-client` to
    /// generate a human-friendly default. When a completion callback is
    /// given, FA persists it before any remote work, sends the same value to
    /// every guardian at `StartDkg`, never exposes the bearer in public
    /// state, and drops its copy with the DKG checkpoint.
    ///
    /// This saves the creation and its purchase, then returns.
    /// [`Self::run`] requests each seat's quote for exactly the previewed
    /// plan, pays automatically, and forms the federation once every seat is
    /// bought. Without a payment federation, the purchase proceeds only if
    /// every selected Manifold Guardian's live offer and exact signed quote
    /// are free. When a purchase ends before it buys its seats, the state
    /// asks for a new preview of the missing seats with its typed
    /// [`SelectionReauthorizationReason`].
    pub async fn create_federation(
        &self,
        federation_name: Option<FederationName>,
        completion_callback: Option<DkgCompletionCallback>,
        preview: &MgSelectionPreview,
        payment_federation_id: Option<FederationId>,
    ) -> FaResult<()> {
        let fa_id = self.fa_id()?;
        let federation_name = federation_name
            .unwrap_or_else(|| default_federation_name(fa_id, now_secs().unwrap_or_default()));
        crate::state::validate_federation_name(&federation_name)?;
        let request = &preview.request;
        if request.seat_count() != request.federation_size().0 {
            return Err(FaError::InvalidIntent(
                "a new federation needs a preview of all its seats".to_owned(),
            ));
        }
        let creation = NewCreation {
            federation_name,
            federation_size: request.federation_size(),
            fedimintd_versions: request.fedimintd_versions().clone(),
            plan: request.plan(),
            dkg_completion_callback: completion_callback,
        };
        self.start_purchase(preview, payment_federation_id, Some(creation))
            .await
    }

    /// Preview the seats the current federation creation still needs.
    ///
    /// The preview keeps the DKG line of the seats already bought and
    /// avoids their MGs, service keys, and badge holders.
    pub async fn preview_missing_seats(
        &self,
        options: crate::MgDiscoveryOptions,
    ) -> FaResult<MgSelectionPreview> {
        let fa_id = self.fa_id()?;
        let creation = self
            .inner
            .store
            .load_creation(fa_id)
            .await?
            .ok_or(FaError::NoActiveFormation)?;
        let pool = self.inner.store.pool_seats().await;
        let bought_count = u16::try_from(pool.len())
            .map_err(|_| FaError::Storage("too many pool seats".to_owned()))?;
        let request = MgSelectionRequest::new(
            creation.federation_size,
            creation.fedimintd_versions,
            creation.plan,
        )?
        .missing_seats(self.inner.store.bought_seats().await, bought_count);
        self.preview_mg_selection(&request, options).await
    }

    /// Buy the missing seats of the current federation creation.
    ///
    /// The preview must come from [`Self::preview_missing_seats`] for the
    /// current pool. Like [`Self::create_federation`], this saves the
    /// purchase and returns; [`Self::run`] buys the seats.
    pub async fn buy_seats(
        &self,
        preview: &MgSelectionPreview,
        payment_federation_id: Option<FederationId>,
    ) -> FaResult<()> {
        let fa_id = self.fa_id()?;
        let creation = self
            .inner
            .store
            .load_creation(fa_id)
            .await?
            .ok_or(FaError::NoActiveFormation)?;
        let pool = self.inner.store.pool_seats().await;
        let bought = self.inner.store.bought_seats().await;
        let request = &preview.request;
        let missing = usize::from(creation.federation_size.0).saturating_sub(pool.len());
        if request.federation_size() != creation.federation_size
            || request.fedimintd_versions() != &creation.fedimintd_versions
            || request.plan() != creation.plan
            || request.bought != bought
            || preview.seats().len() != missing
        {
            return Err(FaError::InvalidIntent(
                "the preview does not match the seats this federation still needs".to_owned(),
            ));
        }
        self.start_purchase(preview, payment_federation_id, None)
            .await
    }

    /// Validate the preview and payer, then save the purchase and wake the
    /// run loop.
    async fn start_purchase(
        &self,
        preview: &MgSelectionPreview,
        payment_federation_id: Option<FederationId>,
        creation: Option<NewCreation>,
    ) -> FaResult<()> {
        let request = &preview.request;
        if preview.verifier_provenance != self.inner.peer_badge_verifier.provenance() {
            return Err(FaError::InvalidIntent(
                "selection preview belongs to a different verifier environment".to_owned(),
            ));
        }
        let fedimintd_dkg_version = preview.fedimintd_dkg_version.clone();
        if !fedimintd_dkg_version.is_fedi()
            || !request
                .fedimintd_versions()
                .overlaps_dkg(&fedimintd_dkg_version)
        {
            return Err(FaError::InvalidIntent(
                "selected Fedimint DKG identity is outside the purchase request".to_owned(),
            ));
        }
        let verifier_provenance = preview.verifier_provenance.into();
        let seats = preview
            .seats_to_buy_at(Timestamp(now_secs()?))?
            .into_iter()
            .enumerate()
            .map(|(index, seat)| {
                InitialSeat::new(
                    index,
                    seat.locator,
                    MgAdmission::fresh_peer_badge(
                        seat.mg_id,
                        seat.holder,
                        seat.plan,
                        verifier_provenance,
                        preview.valid_until(),
                    ),
                )
            })
            .collect();
        let options = FormationRunOptions::default();
        let _run = self.claim_run_guard().await?;
        options.validate_for_start(&self.inner.store)?;
        let fa_id = self.fa_id()?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = async {
            if let Some(requested) = &payment_federation_id {
                // A purchase must not obtain even its first exact quote until
                // the explicitly selected payer is authenticated and currently
                // Ready. Exact balance plus fee sufficiency is checked once
                // the complete verified aggregate exists.
                self.require_setup_payment_federation(requested, run)
                    .await?;
            }
            let created_at = now_secs()?;
            let purchase_id = FormationId(format!("{}-{created_at}", fa_id.0));
            let request = ResolvedSeatPurchase {
                federation_size: request.federation_size(),
                plan: request.plan(),
                fedimintd_versions: request.fedimintd_versions().clone(),
                fedimintd_dkg_version,
            };
            self.inner
                .store
                .initialize(
                    fa_id,
                    purchase_id,
                    request,
                    seats,
                    payment_federation_id,
                    creation,
                )
                .await?;
            let creation = self.inner.store.load_creation(fa_id).await?;
            let recovery = self.active_recovery(fa_id).await?;
            self.inner.progress.modify(|status| {
                status.creation = creation;
                status.reauthorization = None;
                status.purchase = Some(recovery.snapshot);
            });
            Ok(())
        }
        .await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    /// Stop creating a federation before any seat is bought.
    ///
    /// A running purchase ends while it is still value-safe: until wallet
    /// output generation is durably armed. Zero-price seats a Manifold
    /// Guardian already accepted server-side are **forfeited**, not
    /// released. Once seats are bought or payment outputs started, this
    /// returns the typed [`FaError::AbandonUnavailable`]: the seats stay with
    /// the creation until its federation forms.
    ///
    /// The authenticated setup-payment policy retention is deliberately
    /// preserved; it is deployment policy, not creation state.
    pub async fn cancel_creation(&self) -> FaResult<()> {
        let options = FormationRunOptions::default();
        let _run = self.claim_run_guard().await?;
        options.validate_for_start(&self.inner.store)?;
        let fa_id = self.fa_id()?;
        // The same bounded run fences reconstruction and explicit release of
        // any exact pre-output wallet reservation before the local wipe.
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = async {
            // Refuse before ending a running purchase of the missing seats.
            if !self.inner.store.pool_seats().await.is_empty() {
                return Err(FaError::AbandonUnavailable(
                    crate::AbandonUnavailableReason::SeatsBought,
                ));
            }
            if let Some(mut recovery) = self.inner.store.load_purchase(fa_id).await? {
                self.release_and_abandon_pre_output(
                    &mut recovery,
                    fa_id,
                    ReservationCleanup::ReconstructIfAuthorized,
                    run,
                )
                .await?;
            }
            self.inner.store.cancel_creation().await?;
            self.inner.progress.modify(|status| {
                status.creation = None;
                status.reauthorization = None;
            });
            Ok(())
        }
        .await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    /// Propose a new guardian-fee rate after formation.
    ///
    /// Formation fixes the canonical recipient split. This operation changes
    /// only its rate through the generic metadata verb, bound to the exact live
    /// whole-object metadata base; success is confirmed by a fresh consensus
    /// read containing the requested rate.
    ///
    /// The resolved FA account must be a single-signature `BtcDepositor`
    /// account and the rate must be within the pinned payer's inclusive
    /// `0..=210_000`-ppm ceiling and at or above the published minimum
    /// carried by the admitted setup-payment publication (1,500 ppm when none
    /// is admitted).
    /// Both bounds are refused here, before any guardian is contacted, so a
    /// rate every MG would vote down reports its own reason instead of
    /// retrying to a timeout. MG remains
    /// the authoritative split validator and may reject a proposal before
    /// casting its guardian vote. Transport, stale-base, or consensus-read
    /// failures may yield a timeout; cancellation stores no partial policy
    /// record, and retrying the same call safely rereads, rebases, and confirms
    /// the live consensus value.
    pub async fn propose_guardian_fees(
        &self,
        send_ppm: GuardianFeePpm,
        options: FormationRunOptions,
    ) -> FaResult<()> {
        let _run = self.claim_run_guard().await?;
        options.validate_for_start(&self.inner.store)?;
        if send_ppm.value() > crate::MAX_GUARDIAN_FEE_PPM {
            return Err(FaError::InvalidIntent(format!(
                "guardian fee ppm exceeds the payer ceiling of {}",
                crate::MAX_GUARDIAN_FEE_PPM
            )));
        }
        let min_send_ppm = self.min_guardian_fee_ppm().await;
        if u64::from(send_ppm.value()) < min_send_ppm {
            return Err(FaError::InvalidIntent(format!(
                "guardian fee ppm is below the published minimum of {min_send_ppm}"
            )));
        }
        let fa_id = self.fa_id()?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let key = MetaFieldKey(
            fedi_decentralized_service_manifold_guardian::GUARDIAN_FEE_SEND_PPM_META_FIELD_KEY
                .to_owned(),
        );
        let value = MetaFieldValue(send_ppm.value().to_string());
        let result = self.update_meta_field_pinned(key, value, fa_id, run).await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    /// Explicitly replace every unfinished guardian ceremony, without paying
    /// again. All status checks finish before any restart is sent. Guardians
    /// can still finish or disappear afterward, so this is a best-effort wave.
    ///
    /// No request is retried. Cancellation or a missing reply leaves the
    /// outcome unknown; another restart requires a new user decision. The
    /// run loop yields to this call, then continues the formation.
    pub async fn restart_dkg(
        &self,
        options: FormationRunOptions,
    ) -> FaResult<Vec<DkgRestartResult>> {
        let _guard = self.claim_run_guard().await?;
        options.validate_for_start(&self.inner.store)?;
        let fa_id = self.fa_id()?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = async {
            let mut recovery = self.active_formation(fa_id).await?;
            let formation_id = recovery.snapshot.formation_id.clone();
            let formation_id = &formation_id;

            if recovery.snapshot.phase.dkg_complete() {
                return Err(FaError::InvalidIntent(
                    "formation cannot restart DKG in its current state".into(),
                ));
            }
            let codes = recovery
                .seats
                .iter()
                .map(|seat| {
                    seat.progress.guardian_code.clone().ok_or_else(|| {
                        FaError::InvalidIntent("restart requires every saved guardian code".into())
                    })
                })
                .collect::<FaResult<Vec<_>>>()?;
            let sessions = self.seat_sessions(&recovery, run).await?;
            let mut checks = FuturesUnordered::new();
            for session in &sessions {
                checks.push(async {
                    let request = self.sign(&GetStatusRequest {
                        ts: Timestamp(now_secs()?),
                        fa_id,
                        seat_id: session.seat_id.clone(),
                    })?;
                    let response = session
                        .client
                        .call(run, MgOperation::GetStatus, |client| {
                            client.get_status(request)
                        })
                        .await?;
                    if !matches!(
                        response.status,
                        ServiceStatus::New | ServiceStatus::DkgInProcess
                    ) {
                        return Err(session
                            .client
                            .failure(
                                MgOperation::RestartDkg,
                                MgCause::WrongState(response.status),
                            )
                            .into());
                    }
                    Ok::<_, FaError>(())
                });
            }
            while let Some(check) = checks.next().await {
                check?;
            }
            drop(checks);
            recovery.snapshot.freshness = FormationFreshness::Unsynced;
            self.publish_formation(recovery.snapshot.clone());
            let mut pending = FuturesUnordered::new();
            for session in &sessions {
                let codes = &codes;
                pending.push(async move {
                    let result = async {
                        let request = self.sign(&RestartDkgRequest {
                            ts: Timestamp(now_secs()?),
                            fa_id,
                            seat_id: session.seat_id.clone(),
                            guardian_codes: codes.clone(),
                        })?;
                        let response = session
                            .client
                            .call(run, MgOperation::RestartDkg, |client| {
                                client.restart_dkg(request)
                            })
                            .await?;
                        Ok(response.status)
                    }
                    .await;
                    if let Err(error) = &result {
                        log_mg_failure(&formation_id, error, "returned");
                    }
                    DkgRestartResult {
                        index: session.index,
                        mg: session.client.mg,
                        result,
                    }
                });
            }
            let mut results = Vec::new();
            while let Some(result) = pending.next().await {
                results.push(result);
            }
            results.sort_by_key(|result| result.index);
            Ok(results)
        }
        .await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    pub(crate) async fn drive_purchase(
        &self,
        mut recovery: PurchaseRecovery,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let formation_id = recovery.snapshot.purchase_id.clone();

        let result = async {
            let validated_fallback = recovery.snapshot.clone();
            let result = self.drive_purchase_inner(&mut recovery, fa_id, run).await;
            if matches!(
                result.as_ref(),
                Err(FaError::SelectionReauthorizationRequired { .. })
            ) && !recovery.payment_outputs_started
            {
                // A reserve-port error is the one reauthorization outcome that
                // proves this invocation obtained no capability. Other outcomes
                // may follow a wallet commit whose FA checkpoint was interrupted,
                // so they must reconstruct the deterministic reservation before
                // deleting the formation.
                let cleanup = if matches!(
                    result.as_ref(),
                    Err(FaError::SelectionReauthorizationRequired {
                        reason: SelectionReauthorizationReason::SelectedPayerInsufficientFunds,
                        ..
                    })
                ) && recovery.payment_reservation_id.is_none()
                {
                    ReservationCleanup::DefinitivelyAbsent
                } else {
                    ReservationCleanup::ReconstructIfAuthorized
                };
                match self
                    .release_and_abandon_pre_output(&mut recovery, fa_id, cleanup, run)
                    .await
                {
                    Ok(()) => return result,
                    Err(error) => return Err(error),
                }
            }
            if let Err(error) = &result {
                // Prefer the latest durable projection: another seat may have
                // reached a checkpoint before a sibling failed.
                let mut snapshot = self
                    .active_recovery(fa_id)
                    .await
                    .map(|latest| latest.snapshot)
                    .unwrap_or(validated_fallback);
                snapshot.last_error = Some(error.code());
                self.publish_purchase(snapshot);
            }
            result
        }
        .await;
        if let Err(error) = &result {
            log_mg_failure(&formation_id, error, "returned");
            tracing::warn!(
                formation_id = %formation_id.0,
                phase = ?recovery.snapshot.phase,
                %error,
                "formation drive failed"
            );
        }
        result
    }

    /// Release any reconstructable exact wallet hold before deleting FA state.
    ///
    /// Both explicit abandon and automatic selected-flow reauthorization use
    /// this transition. If reconstruction or release is ambiguous, the
    /// formation is retained so a later invocation can retry; no path may drop
    /// the durable reservation id as a substitute for wallet release.
    async fn release_and_abandon_pre_output(
        &self,
        recovery: &mut PurchaseRecovery,
        fa_id: FaId,
        cleanup: ReservationCleanup,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        if recovery.payment_outputs_started {
            return Err(FaError::AbandonUnavailable(
                crate::AbandonUnavailableReason::PaymentOutputsStarted,
            ));
        }
        if cleanup == ReservationCleanup::DefinitivelyAbsent
            && recovery.payment_reservation_id.is_some()
        {
            return Err(FaError::Storage(
                "wallet reservation cannot be both durable and definitively absent".to_owned(),
            ));
        }
        let requirements = recovery.payment_requirements(fa_id)?;
        match requirements {
            Some(requirements)
                if cleanup == ReservationCleanup::ReconstructIfAuthorized
                    && recovery.payments_authorized(&requirements) =>
            {
                // Reserve may have succeeded immediately before its FA DB
                // checkpoint returned ambiguously. Probe the same id without
                // creating a journal; this remains valid after selection
                // freshness or verifier provenance changes.
                let (reservation_id, recovered) = self
                    .recover_payment_reservation(recovery, &requirements, fa_id, run)
                    .await?;
                match recovered {
                    PaymentReservationRecovery::Existing(reservation) => {
                        // Commit to the release before the wallet call so a
                        // crash between the release and the wipe below makes
                        // wallet absence an expected outcome on the next run
                        // instead of retaining the formation forever.
                        if recovery.payment_reservation_id.is_some() {
                            self.inner
                                .store
                                .record_reservation_release_intent(
                                    &recovery.snapshot.purchase_id,
                                    &reservation_id,
                                )
                                .await?;
                            recovery.payment_reservation_release_intended = true;
                        }
                        run.call("releasing pre-output payment reservation", || {
                            Ok(self
                                .inner
                                .ports
                                .payments
                                .release_payment_reservation(reservation.clone()))
                        })
                        .await?
                        .map_err(|error| FaError::Payment(error.to_string()))?;
                    }
                    PaymentReservationRecovery::Absent => {
                        if recovery.payment_reservation_id.as_ref() == Some(&reservation_id)
                            && !recovery.payment_reservation_release_intended
                        {
                            return Err(FaError::Storage(
                                "durable FA reservation is absent from the payment wallet"
                                    .to_owned(),
                            ));
                        }
                        // Absence under the durable release commitment is the
                        // completed wallet half of an interrupted abandon;
                        // only the local wipe below remains.
                    }
                }
            }
            _ if recovery.payment_reservation_id.is_some() => {
                return Err(FaError::Storage(
                    "cannot wipe FA state while its wallet reservation cannot be reconstructed"
                        .to_owned(),
                ));
            }
            _ => {}
        }

        run.lease
            .abandon_purchase(&recovery.snapshot.purchase_id)
            .await?;
        self.inner.progress.modify(|status| status.purchase = None);
        Ok(())
    }

    async fn drive_purchase_inner(
        &self,
        recovery: &mut PurchaseRecovery,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        if recovery
            .seats
            .iter()
            .all(|seat| seat.progress.seat_id.is_some())
        {
            return self.complete_purchase(recovery).await;
        }

        // A wallet commit may have succeeded immediately before the FA-side
        // reservation checkpoint was interrupted. Recover that deterministic
        // aggregate before consulting expiring admission policy: an expired
        // preview or changed verifier provenance must not make an existing
        // wallet journal unreachable.
        let recovery_requirements = if recovery.payment_reservation_id.is_some() {
            recovery.reserved_payment_requirements(fa_id)?
        } else {
            recovery.payment_requirements(fa_id)?
        };
        if let Some(requirements) = recovery_requirements.as_ref()
            && recovery.payments_authorized(requirements)
        {
            let (reservation_id, recovered) = self
                .recover_payment_reservation(recovery, requirements, fa_id, run)
                .await?;
            if recovery
                .payment_reservation_id
                .as_ref()
                .is_some_and(|stored| stored != &reservation_id)
            {
                return Err(FaError::Storage(
                    "stored wallet reservation does not match exact payment requirements"
                        .to_owned(),
                ));
            }
            match recovered {
                PaymentReservationRecovery::Existing(_) => {
                    self.inner
                        .store
                        .record_payment_reservation(&recovery.snapshot.purchase_id, &reservation_id)
                        .await?;
                    recovery.payment_reservation_id = Some(reservation_id);
                    // Adoption also durably supersedes any interrupted
                    // release commitment.
                    recovery.payment_reservation_release_intended = false;
                }
                PaymentReservationRecovery::Absent if recovery.payment_reservation_id.is_some() => {
                    if !recovery.payment_reservation_release_intended {
                        return Err(FaError::Storage(
                            "durable FA reservation is absent from the payment wallet".to_owned(),
                        ));
                    }
                    // A prior abandon durably committed to this release and
                    // the wallet completed it; only the local half of that
                    // cleanup remains.
                    if recovery.payment_outputs_started {
                        return Err(FaError::Storage(
                            "a released wallet reservation remains after payment outputs started"
                                .to_owned(),
                        ));
                    }
                    run.lease
                        .abandon_purchase(&recovery.snapshot.purchase_id)
                        .await?;
                    self.inner.progress.modify(|status| status.purchase = None);
                    return Ok(());
                }
                PaymentReservationRecovery::Absent => {}
            }
        }

        validate_live_admissions(
            recovery,
            self.inner.peer_badge_verifier.provenance().into(),
            Timestamp(now_secs()?),
        )?;

        self.prepare_quotes(recovery, fa_id, run).await?;
        if let Some(requirements) = recovery.payment_requirements(fa_id)?
            && !recovery.payments_authorized(&requirements)
        {
            // Every new quote is for its seat's previewed plan, so the
            // approved preview already authorizes these exact quotes. The
            // durable record binds the quote ids for recovery and abandon.
            let authorizations = requirements
                .seats
                .iter()
                .map(|requirement| QuoteAuthorization {
                    index: requirement.index,
                    quote_id: requirement.quote_id,
                })
                .collect::<Vec<_>>();
            self.inner
                .store
                .authorize_payments(
                    &recovery.snapshot.purchase_id,
                    &requirements.authorization_id,
                    &authorizations,
                )
                .await?;
            recovery.apply_payment_authorization(requirements.authorization_id, authorizations);
        }

        recovery.snapshot.phase = PurchasePhase::AcquiringSeats;
        recovery.snapshot.freshness = FormationFreshness::Fresh;
        self.publish_purchase(recovery.snapshot.clone());
        self.acquire_seats(recovery, fa_id, run).await?;
        self.complete_purchase(recovery).await
    }

    async fn complete_purchase(&self, recovery: &PurchaseRecovery) -> FaResult<()> {
        self.inner
            .store
            .complete_purchase(&recovery.snapshot.purchase_id)
            .await?;
        self.inner.progress.modify(|status| status.purchase = None);
        self.publish_pool().await;
        Ok(())
    }

    async fn prepare_quotes(
        &self,
        recovery: &mut PurchaseRecovery,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let formation_id = recovery.snapshot.purchase_id.clone();
        let formation_id = &formation_id;
        let intent = recovery.snapshot.request.clone();
        let expected_payer = recovery.payment_federation_id.clone();

        // Free quotes must be replayed exactly: CreateSeat may have succeeded
        // even when its response was lost. Paid quotes are refreshed unless an
        // authorization requires us to first ask the wallet whether payment
        // has already started.
        for position in 0..recovery.seats.len() {
            let seat = &recovery.seats[position];
            let is_free = seat.signed_quote.as_ref().is_some_and(|signed_quote| {
                signed_quote
                    .verify(&seat.progress.locator.service_pubkey)
                    .is_ok_and(|quote| quote.terms.payment.is_none())
            });
            let is_authorized = seat.signed_quote.as_ref().is_some_and(|signed_quote| {
                signed_quote
                    .verify(&seat.progress.locator.service_pubkey)
                    .is_ok_and(|quote| {
                        recovery.quote_is_authorized(seat.progress.index, quote.quote_id())
                    })
            });
            if seat.progress.seat_id.is_some() || is_free || is_authorized {
                continue;
            }
            let Some(signed_quote) = seat.signed_quote.clone() else {
                continue;
            };
            self.verify_quote(
                usize::from(seat.progress.index),
                &signed_quote,
                &seat.progress.locator,
                &intent,
                fa_id,
                expected_payer.as_ref(),
            )?;
            self.inner
                .store
                .clear_quote(formation_id, seat.progress.index, &signed_quote)
                .await?;
            recovery.invalidate_payment_authorization();
            recovery.seats[position].signed_quote = None;
            recovery.seats[position].progress.phase = SeatPhase::Selected;
        }

        let needs_quote = recovery
            .seats
            .iter()
            .any(|seat| seat.progress.seat_id.is_none() && seat.signed_quote.is_none());
        // Resolved on first use and shared by the whole batch, so one
        // authenticated read serves every seat — and a formation that meets
        // no priced seat never reads a setup-payment policy at all.
        let payment_federation =
            (needs_quote && recovery.payment_federation_id.is_some()).then(OnceCell::new);
        let mut pending = FuturesUnordered::new();
        for (position, seat) in recovery.seats.iter().enumerate() {
            if seat.progress.seat_id.is_some() || seat.signed_quote.is_some() {
                continue;
            }
            let locator = seat.progress.locator.clone();
            let intent = &intent;
            let expected_payer = expected_payer.clone();
            let payment_federation = payment_federation.as_ref();
            let policy = quote_attempt_policy(&seat.admission);
            let pinned_plan = seat.admission.plan().cloned();
            pending.push(async move {
                let (signed_quote, _) = self
                    .request_quote_with_retry(
                        formation_id,
                        position,
                        &locator,
                        intent,
                        pinned_plan.as_ref(),
                        fa_id,
                        policy,
                        expected_payer.as_ref(),
                        payment_federation,
                        run,
                    )
                    .await?;
                Ok::<_, FaError>((position, signed_quote))
            });
        }

        let mut first_error = None;
        while let Some(result) = pending.next().await {
            match result {
                Ok((position, signed_quote)) => {
                    let index = recovery.seats[position].progress.index;
                    self.inner
                        .store
                        .store_quote(formation_id, index, signed_quote.clone())
                        .await?;
                    recovery.invalidate_payment_authorization();
                    recovery.seats[position].signed_quote = Some(signed_quote);
                    recovery.seats[position].progress.phase = SeatPhase::QuoteReady;
                    recovery.seats[position].progress.freshness = FormationFreshness::Fresh;
                    recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
                    recovery.snapshot.phase = PurchasePhase::Preparing;
                    recovery.snapshot.freshness = FormationFreshness::Fresh;
                    self.publish_purchase(recovery.snapshot.clone());
                }
                Err(error) => {
                    if first_error.is_some() {
                        log_mg_failure(&formation_id, &error, "discarded");
                    }
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn reserve_payment_requirements(
        &self,
        recovery: &mut PurchaseRecovery,
        requirements: &PaymentRequirements,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<P::PaymentReservation> {
        let (reservation_id, recovered) = self
            .recover_payment_reservation(recovery, requirements, fa_id, run)
            .await?;
        if recovery
            .payment_reservation_id
            .as_ref()
            .is_some_and(|stored| stored != &reservation_id)
        {
            return Err(FaError::Storage(
                "stored wallet reservation does not match exact payment requirements".to_owned(),
            ));
        }
        if let PaymentReservationRecovery::Existing(reservation) = recovered {
            self.inner
                .store
                .record_payment_reservation(&recovery.snapshot.purchase_id, &reservation_id)
                .await?;
            recovery.payment_reservation_id = Some(reservation_id);
            return Ok(reservation);
        }
        if recovery.payment_reservation_id.is_some() {
            return Err(FaError::Storage(
                "durable FA reservation is absent from the payment wallet".to_owned(),
            ));
        }
        validate_live_admissions(
            recovery,
            self.inner.peer_badge_verifier.provenance().into(),
            Timestamp(now_secs()?),
        )?;

        let quotes = self.exact_payment_quotes(recovery, requirements, fa_id)?;
        let preflight = crate::ExactPaymentPreflight::new(requirements, &quotes)?;
        let reservation = run
            .call("reserving exact payment aggregate", || {
                Ok(self
                    .inner
                    .ports
                    .payments
                    .reserve_payment_requirements(&reservation_id, &preflight))
            })
            .await?
            .map_err(|error| {
                if error.proves_insufficient_funds_without_reservation() {
                    FaError::SelectionReauthorizationRequired {
                        reason: SelectionReauthorizationReason::SelectedPayerInsufficientFunds,
                        failure: None,
                    }
                } else {
                    FaError::Payment(error.to_string())
                }
            })?;
        self.inner
            .store
            .record_payment_reservation(&recovery.snapshot.purchase_id, &reservation_id)
            .await?;
        recovery.payment_reservation_id = Some(reservation_id);
        Ok(reservation)
    }

    /// Probe the deterministic aggregate id and exact quote/output binding
    /// without creating wallet state or consulting expiring admission policy.
    async fn recover_payment_reservation(
        &self,
        recovery: &PurchaseRecovery,
        requirements: &PaymentRequirements,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<(
        PaymentReservationId,
        PaymentReservationRecovery<P::PaymentReservation>,
    )> {
        let quotes = self.exact_payment_quotes(recovery, requirements, fa_id)?;
        let preflight = crate::ExactPaymentPreflight::new(requirements, &quotes)?;
        let reservation_id =
            crate::db::payment_reservation_id(&recovery.snapshot.purchase_id, requirements);
        let recovered = run
            .call("recovering exact payment reservation", || {
                Ok(self
                    .inner
                    .ports
                    .payments
                    .recover_payment_reservation(&reservation_id, &preflight))
            })
            .await?
            .map_err(|error| FaError::Payment(error.to_string()))?;
        Ok((reservation_id, recovered))
    }

    fn exact_payment_quotes(
        &self,
        recovery: &PurchaseRecovery,
        requirements: &PaymentRequirements,
        fa_id: FaId,
    ) -> FaResult<Vec<SignatureVerified<GetQuoteResponse>>> {
        let expected_payer = recovery.payment_federation_id.clone();
        let mut quotes = Vec::with_capacity(requirements.seats.len());
        for requirement in &requirements.seats {
            let seat = recovery
                .seats
                .iter()
                .find(|seat| seat.progress.index == requirement.index)
                .ok_or_else(|| {
                    FaError::Storage(format!(
                        "payment requirement names missing FA seat row {}",
                        requirement.index
                    ))
                })?;
            let signed = seat.signed_quote.as_ref().ok_or_else(|| {
                FaError::Storage(format!(
                    "payment requirement has no quote for FA seat row {}",
                    requirement.index
                ))
            })?;
            let quote = self.verify_quote(
                usize::from(requirement.index),
                signed,
                &seat.progress.locator,
                &recovery.snapshot.request,
                fa_id,
                expected_payer.as_ref(),
            )?;
            if quote.quote_id() != requirement.quote_id || quote.terms.payment.is_none() {
                return Err(FaError::Storage(format!(
                    "payment requirement does not match FA seat row {}",
                    requirement.index
                )));
            }
            quotes.push(quote);
        }
        Ok(quotes)
    }

    /// Resolve a selected MG transport while a fresh selection is still
    /// value-safe. A definite connection failure is retried with bounded
    /// exponential backoff for at most two minutes (and never beyond this
    /// driver invocation). After outputs start, the same failure remains an
    /// exact-replay error and must never require a fresh selection.
    async fn connect_with_selected_retry(
        &self,
        locator: &Locator,
        formation_id: &FormationId,
        policy: QuoteAttemptPolicy,
        run: DriverRun<'_>,
    ) -> FaResult<MgClient<F::Client>> {
        let retry_deadline = Instant::now()
            .checked_add(SELECTED_MG_CONNECT_RETRY_BUDGET)
            .unwrap_or(run.deadline)
            .min(run.deadline);
        let mut retry_delay = run
            .options
            .poll_interval
            .max(SELECTED_MG_CONNECT_RETRY_MIN_DELAY)
            .min(SELECTED_MG_CONNECT_RETRY_MAX_DELAY);
        loop {
            let error = match MgClient::connect(&self.inner.ports.mg_connector, locator, run).await
            {
                Ok(client) => return Ok(client),
                Err(error) => error,
            };
            let FaError::Mg(failure) = error else {
                return Err(error);
            };
            if !policy.allows_selection_reauthorization()
                || !matches!(failure.cause, MgCause::Timeout | MgCause::Transport)
            {
                return Err(failure.into());
            }
            if Instant::now() >= retry_deadline
                || sleep_for_retry(retry_deadline, retry_delay).await.is_err()
            {
                return Err(FaError::SelectionReauthorizationRequired {
                    reason: SelectionReauthorizationReason::SelectedMgUnavailable,
                    failure: Some(failure),
                });
            }
            log_mg_failure(formation_id, &FaError::Mg(failure), "retried");
            retry_delay = retry_delay
                .saturating_mul(2)
                .min(SELECTED_MG_CONNECT_RETRY_MAX_DELAY);
        }
    }

    /// Connect, inspect availability, and request one quote as a single
    /// bounded value-free attempt. A selected row whose authorization has not
    /// been consumed may reconnect after definite transport loss; semantic
    /// refusals return immediately and an already value-bound row never
    /// requires a fresh selection after a transport failure.
    #[allow(clippy::too_many_arguments)]
    async fn request_quote_with_retry(
        &self,
        formation_id: &FormationId,
        index: usize,
        locator: &Locator,
        intent: &ResolvedSeatPurchase,
        pinned_plan: Option<&Plan>,
        fa_id: FaId,
        policy: QuoteAttemptPolicy,
        expected_payer: Option<&FederationId>,
        payment_federation: Option<&OnceCell<FederationId>>,
        run: DriverRun<'_>,
    ) -> FaResult<(
        SignedResponse<GetQuoteResponse>,
        SignatureVerified<GetQuoteResponse>,
    )> {
        let retry_deadline = Instant::now()
            .checked_add(SELECTED_MG_CONNECT_RETRY_BUDGET)
            .unwrap_or(run.deadline)
            .min(run.deadline);
        let mut retry_delay = run
            .options
            .poll_interval
            .max(SELECTED_MG_CONNECT_RETRY_MIN_DELAY)
            .min(SELECTED_MG_CONNECT_RETRY_MAX_DELAY);
        loop {
            let result = async {
                // A seat saved before the preview pinned its price has no
                // approved price to request.
                let pinned_plan = pinned_plan.ok_or_else(|| {
                    if policy.allows_selection_reauthorization() {
                        FaError::SelectionReauthorizationRequired {
                            reason: SelectionReauthorizationReason::PreviewExpired,
                            failure: None,
                        }
                    } else {
                        mg_error(index, "seat has no previewed price to quote")
                    }
                })?;
                let client =
                    MgClient::connect(&self.inner.ports.mg_connector, locator, run).await?;
                self.request_new_quote(
                    index,
                    &client,
                    locator,
                    intent,
                    pinned_plan,
                    fa_id,
                    policy,
                    expected_payer,
                    payment_federation,
                    run,
                )
                .await
            }
            .await;
            let error = match result {
                Ok(quote) => return Ok(quote),
                Err(error) => error,
            };
            let FaError::Mg(failure) = error else {
                return Err(error);
            };
            if !policy.allows_selection_reauthorization()
                || !matches!(failure.cause, MgCause::Timeout | MgCause::Transport)
            {
                return Err(failure.into());
            }
            if retry_quote_attempt(retry_deadline, &mut retry_delay)
                .await
                .is_err()
            {
                return Err(FaError::SelectionReauthorizationRequired {
                    reason: SelectionReauthorizationReason::SelectedMgUnavailable,
                    failure: Some(failure),
                });
            }
            log_mg_failure(formation_id, &FaError::Mg(failure), "retried");
        }
    }

    async fn acquire_seats(
        &self,
        recovery: &mut PurchaseRecovery,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<Vec<SeatSession<MgClient<F::Client>>>> {
        let formation_id = recovery.snapshot.purchase_id.clone();
        let formation_id = &formation_id;
        let intent = recovery.snapshot.request.clone();
        let expected_payer = recovery.payment_federation_id.clone();
        let payment_authorized = recovery
            .seats
            .iter()
            .map(|seat| {
                seat.signed_quote.as_ref().is_some_and(|signed_quote| {
                    signed_quote
                        .verify(&seat.progress.locator.service_pubkey)
                        .is_ok_and(|quote| {
                            recovery.quote_is_authorized(seat.progress.index, quote.quote_id())
                        })
                })
            })
            .collect::<Vec<_>>();
        let mut recovered_payments = (0..recovery.seats.len())
            .map(|_| None)
            .collect::<Vec<Option<SeatPaymentRecovery<P::RefundContext, P::TerminalReleaseProof>>>>(
            );
        let mut first_barrier_error = None;
        let mut first_terminal_outcome = None;
        let mut terminal_quotes = Vec::new();
        let mut pending_recoveries = FuturesUnordered::new();
        let recovery_requirements = if recovery.payment_reservation_id.is_some() {
            recovery.reserved_payment_requirements(fa_id)?
        } else {
            recovery.payment_requirements(fa_id)?
        };
        let recovery_reservation_id = recovery_requirements.as_ref().map(|requirements| {
            crate::db::payment_reservation_id(&recovery.snapshot.purchase_id, requirements)
        });
        let mut payment_reservation = if let Some(requirements) = recovery_requirements.as_ref()
            && recovery.payments_authorized(requirements)
        {
            Some(
                self.reserve_payment_requirements(recovery, requirements, fa_id, run)
                    .await?,
            )
        } else {
            None
        };
        for (position, seat) in recovery.seats.iter().enumerate() {
            if seat.progress.seat_id.is_some() || !payment_authorized[position] {
                continue;
            }
            let Some(signed_quote) = seat.signed_quote.as_ref() else {
                continue;
            };
            let quote = self.verify_quote(
                position,
                signed_quote,
                &seat.progress.locator,
                &intent,
                fa_id,
                expected_payer.as_ref(),
            )?;
            if quote.terms.payment.is_none() {
                continue;
            }
            let reservation_id = recovery_reservation_id.clone().ok_or_else(|| {
                FaError::Storage("authorized paid quote has no aggregate reservation id".to_owned())
            })?;
            pending_recoveries.push(async move {
                let recovered = run
                    .call_with_timeout("recovering seat payment", run.options.run_timeout, || {
                        Ok(self
                            .inner
                            .ports
                            .payments
                            .recover_seat_payment(&reservation_id, &quote))
                    })
                    .await?
                    .map_err(|error| FaError::Payment(error.to_string()))?;
                Ok::<_, FaError>((position, recovered))
            });
        }
        while let Some(result) = pending_recoveries.next().await {
            match result {
                Ok((position, SeatPaymentRecovery::Rejected(proof))) => {
                    terminal_quotes.push((position, Some(proof)));
                    first_terminal_outcome
                        .get_or_insert(TerminalSeatOutcome::PaymentRejected { position });
                }
                Ok((position, recovered)) => recovered_payments[position] = Some(recovered),
                Err(error) => {
                    if first_barrier_error.is_some() {
                        log_mg_failure(&formation_id, &error, "discarded");
                    }
                    first_barrier_error.get_or_insert(error);
                }
            }
        }

        let mut pending_replays = FuturesUnordered::new();
        for (position, recovered_payment) in recovered_payments.iter_mut().enumerate() {
            let prepared = match recovered_payment.take() {
                Some(SeatPaymentRecovery::Prepared(prepared)) => prepared,
                recovered => {
                    *recovered_payment = recovered;
                    continue;
                }
            };
            let seat = &recovery.seats[position];
            let locator = seat.progress.locator.clone();
            let signed_quote = seat
                .signed_quote
                .clone()
                .expect("prepared payment requires a stored quote");
            let paid_quote = self.verify_paid_seat_quote(
                position,
                signed_quote,
                &locator,
                &intent,
                fa_id,
                expected_payer.as_ref(),
            )?;
            pending_replays.push(async move {
                let client =
                    MgClient::connect(&self.inner.ports.mg_connector, &locator, run).await?;
                let creation = self
                    .create_or_replay_seat(
                        formation_id,
                        position,
                        &client,
                        &locator,
                        fa_id,
                        SeatAcquisition::ReplayPrepared {
                            quote: paid_quote,
                            prepared,
                        },
                        run,
                    )
                    .await?;
                Ok::<_, FaError>((position, creation))
            });
        }
        while let Some(result) = pending_replays.next().await {
            let (position, creation) = match result {
                Ok(value) => value,
                Err(error) => {
                    if first_barrier_error.is_some() {
                        log_mg_failure(&formation_id, &error, "discarded");
                    }
                    first_barrier_error.get_or_insert(error);
                    continue;
                }
            };
            match creation {
                SeatCreation::Accepted(acceptance) => {
                    if let Err(error) = self
                        .checkpoint_accepted_seat(recovery, position, acceptance)
                        .await
                    {
                        if first_barrier_error.is_some() {
                            log_mg_failure(&formation_id, &error, "discarded");
                        }
                        first_barrier_error.get_or_insert(error);
                    }
                }
                SeatCreation::Refused(refusal) => {
                    terminal_quotes.push((position, refusal.release_proof));
                    first_terminal_outcome
                        .get_or_insert(TerminalSeatOutcome::SeatRefused(refusal.error));
                }
            }
        }
        // Keep every quote and the aggregate authorization intact when any
        // sibling has not reached a durable recovery checkpoint. A later
        // resume must still recognize that sibling's recovery entitlement.
        if let Some(error) = first_barrier_error {
            return Err(error);
        }
        if first_terminal_outcome.as_ref().is_some_and(|outcome| {
            outcome.requires_pre_output_reauthorization(recovery.payment_outputs_started)
        }) {
            // Selected all-free refusal is itself the reauthorization proof.
            // Keep its exact signed quote and consumed admission durable until
            // the outer driver completes durable abandon. A failed abandon can
            // then reopen and replay the same refusal safely.
            return Err(first_terminal_outcome
                .expect("checked terminal outcome exists")
                .into_formation_error(recovery.payment_outputs_started));
        }
        // A terminal member can share its aggregate reservation with a paid
        // sibling that the wallet still reports as Held/NotStarted. Keep the
        // terminal quote, aggregate authorization, and reservation identity
        // until every such sibling reaches an accepted checkpoint or its own
        // terminal proof. Clearing only the terminal subset here would strand
        // the sibling hold and erase the exact replay identity.
        let terminal_positions = terminal_quotes
            .iter()
            .map(|(position, _)| *position)
            .collect::<BTreeSet<_>>();
        // A quote that no payment has started against is refreshed before
        // funding, and the policy it was selected under is refreshed with it:
        // re-quoting under a snapshot the FA never re-read would only be half
        // a refresh. Selection is hoisted here, as it is for initial quoting,
        // so one authenticated read serves the whole batch.
        let needs_funding =
            recovery
                .seats
                .iter()
                .zip(&recovered_payments)
                .any(|(seat, recovered)| {
                    seat.progress.seat_id.is_none()
                        && matches!(recovered, Some(SeatPaymentRecovery::NotStarted))
                });
        let payment_federation_id = if needs_funding {
            let Some(requested) = expected_payer.as_ref() else {
                return Err(FaError::SelectionReauthorizationRequired {
                    reason: SelectionReauthorizationReason::PaymentFederationRequired,
                    failure: None,
                });
            };
            Some(
                self.require_setup_payment_federation(requested, run)
                    .await?,
            )
        } else {
            None
        };
        let mut refreshed_payments = match payment_federation_id {
            Some(payment_federation_id) => {
                self.refresh_unstarted_payments(
                    recovery,
                    &recovered_payments,
                    fa_id,
                    &intent,
                    payment_federation_id,
                    run,
                )
                .await?
            }
            None => (0..recovery.seats.len()).map(|_| None).collect(),
        };
        let will_generate_outputs = refreshed_payments.iter().any(Option::is_some);
        // Resolve every fallible MG connection before crossing the durable
        // wallet-output boundary. Selected creation receives its bounded
        // pre-output retry policy; after outputs start, failures remain exact
        // recovery errors and never require a fresh selection.
        let mut clients = (0..recovery.seats.len())
            .map(|_| None)
            .collect::<Vec<Option<MgClient<F::Client>>>>();
        let mut pending_connections = FuturesUnordered::new();
        for (position, seat) in recovery.seats.iter().enumerate() {
            if terminal_positions.contains(&position) {
                continue;
            }
            let locator = seat.progress.locator.clone();
            let connection_policy = quote_attempt_policy(&seat.admission);
            pending_connections.push(async move {
                let client = self
                    .connect_with_selected_retry(&locator, formation_id, connection_policy, run)
                    .await?;
                Ok::<_, FaError>((position, client))
            });
        }
        let mut first_connection_error = None;
        while let Some(result) = pending_connections.next().await {
            match result {
                Ok((position, client)) => clients[position] = Some(client),
                Err(error) => {
                    first_connection_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first_connection_error {
            return Err(error);
        }

        let payment_reservation = if will_generate_outputs {
            let requirements = if recovery.payment_reservation_id.is_some() {
                recovery.reserved_payment_requirements(fa_id)?
            } else {
                recovery.payment_requirements(fa_id)?
            }
            .ok_or_else(|| {
                FaError::Storage(
                    "paid output generation has no complete payment requirements".to_owned(),
                )
            })?;
            match payment_reservation.take() {
                Some(reservation) => Some(reservation),
                None => Some(
                    self.reserve_payment_requirements(recovery, &requirements, fa_id, run)
                        .await?,
                ),
            }
        } else {
            None
        };

        // Validate every presentation and prepare effect-free payment work
        // before arming outputs. No wallet call is polled here.
        let mut funding = Vec::new();
        let mut presentations = Vec::new();
        for (position, seat) in recovery.seats.iter_mut().enumerate() {
            if terminal_positions.contains(&position) {
                continue;
            }
            let locator = seat.progress.locator.clone();
            let existing = seat.progress.seat_id.clone().map(|seat_id| SeatAcceptance {
                seat_id,
                guardian_fee_account: seat
                    .guardian_fee_account
                    .clone()
                    .expect("validated accepted seat has its signed fee account"),
            });
            let signed_quote = seat.signed_quote.clone();
            let recovered_payment = recovered_payments[position].take();
            let refreshed_payment = refreshed_payments[position].take();
            let client = clients[position].take().ok_or_else(|| {
                FaError::Storage(format!(
                    "missing preflight MG connection for seat {position}"
                ))
            })?;
            if existing.is_none() {
                seat.progress.phase = SeatPhase::Acquiring;
                recovery.snapshot.seats[position] = seat.progress.clone();
            }
            if let Some(existing) = existing {
                presentations.push(PendingSeatPresentation {
                    position,
                    client,
                    locator,
                    source: SeatPresentationSource::Existing(existing),
                });
                continue;
            }
            let signed_quote = signed_quote
                .ok_or_else(|| FaError::Storage(format!("FA seat row {position} has no quote")))?;
            match recovered_payment {
                None => {
                    presentations.push(PendingSeatPresentation {
                        position,
                        client,
                        locator: locator.clone(),
                        source: SeatPresentationSource::Acquire(Box::new(SeatAcquisition::Free(
                            self.verify_free_seat_quote(
                                position,
                                signed_quote,
                                &locator,
                                &intent,
                                fa_id,
                                expected_payer.as_ref(),
                            )?,
                        ))),
                    });
                }
                Some(SeatPaymentRecovery::NotStarted) => {
                    let quote = refreshed_payment.ok_or_else(|| {
                        FaError::Storage(format!(
                            "paid FA seat row {position} escaped the refresh barrier"
                        ))
                    })?;
                    let reservation = payment_reservation.clone().ok_or_else(|| {
                        FaError::Storage(format!(
                            "paid FA seat row {position} has no aggregate reservation"
                        ))
                    })?;
                    funding.push(PendingSeatFunding {
                        position,
                        quote,
                        reservation,
                        timeout_budget: run
                            .prepare_value_call_budget("funding seat payment")
                            .await?,
                        client,
                        locator,
                    });
                }
                Some(SeatPaymentRecovery::Prepared(_)) => {
                    return Err(FaError::Storage(format!(
                        "prepared FA seat row {position} escaped the replay barrier"
                    )));
                }
                Some(SeatPaymentRecovery::Rejected(_)) => {
                    return Err(FaError::Storage(format!(
                        "rejected FA seat row {position} escaped terminal clearing"
                    )));
                }
            }
        }
        self.publish_purchase(recovery.snapshot.clone());

        let free_effects = presentations
            .iter()
            .filter_map(|presentation| {
                let SeatPresentationSource::Acquire(acquisition) = &presentation.source else {
                    return None;
                };
                let SeatAcquisition::Free(quote) = acquisition.as_ref() else {
                    return None;
                };
                Some((
                    presentation.position,
                    recovery.seats[presentation.position].progress.index,
                    quote.verified.quote_id(),
                    AdmissionEffect::FreePresentation,
                ))
            })
            .collect::<Vec<_>>();
        let paid_effects = funding
            .iter()
            .map(|pending_funding| {
                (
                    pending_funding.position,
                    recovery.seats[pending_funding.position].progress.index,
                    pending_funding.quote.verified.quote_id(),
                    AdmissionEffect::PaidOutput,
                )
            })
            .collect::<Vec<_>>();
        let verifier_provenance = self.inner.peer_badge_verifier.provenance().into();
        if !recovery.payment_outputs_started && !funding.is_empty() {
            // Free effects are consumed before arming paid outputs, but no
            // presentation is polled between these durable transactions.
            // A failure therefore remains pre-effect and abandonable.
            if !free_effects.is_empty() {
                let effects = free_effects
                    .iter()
                    .map(|(_, index, quote_id, effect)| (*index, *quote_id, *effect))
                    .collect::<Vec<_>>();
                run.authorize_seat_effects(
                    &recovery.snapshot.purchase_id,
                    &effects,
                    verifier_provenance,
                )
                .await?;
                for &(position, _, quote_id, effect) in &free_effects {
                    apply_authorized_effect_in_memory(recovery, position, quote_id, effect)?;
                }
            }
            run.arm_payment_outputs_started(&recovery.snapshot.purchase_id, verifier_provenance)
                .await?;
            recovery.payment_outputs_started = true;
            recovery.snapshot.payment_outputs_started = true;
            for &(position, _, quote_id, effect) in &paid_effects {
                apply_authorized_effect_in_memory(recovery, position, quote_id, effect)?;
            }
            self.publish_purchase(recovery.snapshot.clone());
        } else {
            // Post-output rows can mix paid and free quotes. Consume the
            // complete exact wave atomically before polling the first wallet
            // or presentation effect so expiry cannot strand only part of
            // the wave.
            let effect_wave = paid_effects
                .iter()
                .chain(&free_effects)
                .copied()
                .collect::<Vec<_>>();
            if !effect_wave.is_empty() {
                let effects = effect_wave
                    .iter()
                    .map(|(_, index, quote_id, effect)| (*index, *quote_id, *effect))
                    .collect::<Vec<_>>();
                run.authorize_seat_effects(
                    &recovery.snapshot.purchase_id,
                    &effects,
                    verifier_provenance,
                )
                .await?;
                for (position, _, quote_id, effect) in effect_wave {
                    apply_authorized_effect_in_memory(recovery, position, quote_id, effect)?;
                }
            }
        }

        let mut sessions = (0..recovery.seats.len())
            .map(|_| None)
            .collect::<Vec<Option<SeatSession<MgClient<F::Client>>>>>();
        let mut first_barrier_error = None;
        // A payment wallet may need the change from one accepted Fedimint
        // transaction to fund the next quote. Drive paid seats in stable seat
        // order and durably checkpoint each acceptance before polling the next
        // wallet future. Recovery enters through this same list, so every
        // newly started value movement has the same boundary.
        for funding in funding {
            let result = self
                .complete_pending_seat_work(
                    formation_id,
                    PendingSeatWork::Funding(Box::new(funding)),
                    fa_id,
                    run,
                )
                .await;
            let CompletedSeatWork {
                position,
                client,
                creation,
                checkpoint,
            } = match result {
                Ok(value) => value,
                Err(error) => {
                    if first_barrier_error.is_some() {
                        log_mg_failure(&formation_id, &error, "discarded");
                    }
                    first_barrier_error.get_or_insert(error);
                    break;
                }
            };
            let acceptance = match creation {
                SeatCreation::Accepted(acceptance) => acceptance,
                SeatCreation::Refused(refund) => {
                    terminal_quotes.push((position, refund.release_proof));
                    first_terminal_outcome
                        .get_or_insert(TerminalSeatOutcome::SeatRefused(refund.error));
                    continue;
                }
            };
            if matches!(checkpoint, SeatCheckpoint::Required)
                && let Err(error) = self
                    .checkpoint_accepted_seat(recovery, position, acceptance)
                    .await
            {
                first_barrier_error.get_or_insert(error);
                break;
            }
            sessions[position] = Some(SeatSession {
                index: recovery.seats[position].progress.index,
                client,
                seat_id: recovery.seats[position]
                    .progress
                    .seat_id
                    .clone()
                    .expect("accepted seat was checkpointed or already recovered"),
            });
        }
        if let Some(error) = first_barrier_error {
            return Err(error);
        }

        // Free and already durable presentations move no wallet value and do
        // not consume change needed by a sibling, so they retain parallel
        // transport progress after every new payment is safely checkpointed.
        let mut pending = FuturesUnordered::new();
        for presentation in presentations {
            pending.push(self.complete_pending_seat_work(
                formation_id,
                PendingSeatWork::Presentation(presentation),
                fa_id,
                run,
            ));
        }
        while let Some(result) = pending.next().await {
            let CompletedSeatWork {
                position,
                client,
                creation,
                checkpoint,
            } = match result {
                Ok(value) => value,
                Err(error) => {
                    if first_barrier_error.is_some() {
                        log_mg_failure(&formation_id, &error, "discarded");
                    }
                    first_barrier_error.get_or_insert(error);
                    continue;
                }
            };
            let acceptance = match creation {
                SeatCreation::Accepted(acceptance) => acceptance,
                SeatCreation::Refused(refund) => {
                    terminal_quotes.push((position, refund.release_proof));
                    first_terminal_outcome
                        .get_or_insert(TerminalSeatOutcome::SeatRefused(refund.error));
                    continue;
                }
            };
            if matches!(checkpoint, SeatCheckpoint::Required)
                && let Err(error) = self
                    .checkpoint_accepted_seat(recovery, position, acceptance)
                    .await
            {
                first_barrier_error.get_or_insert(error);
                continue;
            }
            sessions[position] = Some(SeatSession {
                index: recovery.seats[position].progress.index,
                client,
                seat_id: recovery.seats[position]
                    .progress
                    .seat_id
                    .clone()
                    .expect("accepted seat was checkpointed or already recovered"),
            });
        }
        // A newly funded sibling with an ambiguous error still depends on the
        // aggregate authorization. Preserve every terminal quote until all
        // siblings have reached durable checkpoints.
        if let Some(error) = first_barrier_error {
            return Err(error);
        }
        if first_terminal_outcome.as_ref().is_some_and(|outcome| {
            outcome.requires_pre_output_reauthorization(recovery.payment_outputs_started)
        }) {
            return Err(first_terminal_outcome
                .expect("checked terminal outcome exists")
                .into_formation_error(recovery.payment_outputs_started));
        }
        let terminal_positions = terminal_quotes
            .iter()
            .map(|(position, _)| *position)
            .collect::<Vec<_>>();
        for (position, proof) in terminal_quotes {
            self.release_terminal_quote_reservation(recovery, position, fa_id, proof, run)
                .await?;
        }
        if !terminal_positions.is_empty() {
            self.clear_terminal_quotes(recovery, &terminal_positions)
                .await?;
        }
        if let Some(outcome) = first_terminal_outcome {
            return Err(outcome.into_formation_error(recovery.payment_outputs_started));
        }
        sessions
            .into_iter()
            .enumerate()
            .map(|(position, session)| {
                session.ok_or_else(|| {
                    FaError::Storage(format!("missing live session for seat {position}"))
                })
            })
            .collect()
    }

    async fn refresh_unstarted_payments(
        &self,
        recovery: &mut PurchaseRecovery,
        recovered_payments: &[Option<
            SeatPaymentRecovery<P::RefundContext, P::TerminalReleaseProof>,
        >],
        fa_id: FaId,
        intent: &ResolvedSeatPurchase,
        payment_federation_id: FederationId,
        run: DriverRun<'_>,
    ) -> FaResult<Vec<Option<PaidSeatQuote>>> {
        let formation_id = recovery.snapshot.purchase_id.clone();
        let formation_id = &formation_id;
        let expected_payer = recovery.payment_federation_id.clone();
        if recovery.payment_reservation_id.is_some() {
            let requirements = recovery
                .reserved_payment_requirements(fa_id)?
                .ok_or_else(|| {
                    FaError::Storage(
                        "stored wallet reservation has no exact payment requirements".to_owned(),
                    )
                })?;
            let expected = crate::db::payment_reservation_id(formation_id, &requirements);
            if recovery.payment_reservation_id.as_ref() != Some(&expected) {
                return Err(FaError::Storage(
                    "stored wallet reservation no longer matches journaled quotes".to_owned(),
                ));
            }
            return recovery
                .seats
                .iter()
                .zip(recovered_payments)
                .enumerate()
                .map(|(position, (seat, recovered))| {
                    if !matches!(recovered, Some(SeatPaymentRecovery::NotStarted)) {
                        return Ok(None);
                    }
                    let signed = seat.signed_quote.clone().ok_or_else(|| {
                        FaError::Storage(format!(
                            "reserved FA seat row {position} has no signed quote"
                        ))
                    })?;
                    let locator = seat.progress.locator.clone();
                    self.verify_paid_seat_quote(
                        position,
                        signed,
                        &locator,
                        intent,
                        fa_id,
                        expected_payer.as_ref(),
                    )
                    .map(Some)
                })
                .collect();
        }
        // Already selected: every seat refreshed here is a paid one.
        let payment_federation = OnceCell::new_with(Some(payment_federation_id));
        let mut pending = FuturesUnordered::new();
        for (position, (seat, recovered)) in
            recovery.seats.iter().zip(recovered_payments).enumerate()
        {
            if !matches!(recovered, Some(SeatPaymentRecovery::NotStarted)) {
                continue;
            }
            let locator = seat.progress.locator.clone();
            let old_signed_quote = seat
                .signed_quote
                .clone()
                .expect("not-started payment requires a stored quote");
            let old_quote = self.verify_paid_seat_quote(
                position,
                old_signed_quote,
                &locator,
                intent,
                fa_id,
                expected_payer.as_ref(),
            )?;
            let intent = intent.clone();
            let expected_payer = expected_payer.clone();
            let payment_federation = Some(&payment_federation);
            let policy = quote_attempt_policy(&seat.admission);
            // A seat saved before the preview pinned its price was authorized
            // at its stored quote's plan.
            let pinned_plan = seat
                .admission
                .plan()
                .unwrap_or(&old_quote.verified.terms.request.plan)
                .clone();
            pending.push(async move {
                let (signed, verified) = self
                    .request_quote_with_retry(
                        formation_id,
                        position,
                        &locator,
                        &intent,
                        Some(&pinned_plan),
                        fa_id,
                        policy,
                        expected_payer.as_ref(),
                        payment_federation,
                        run,
                    )
                    .await?;
                Ok::<_, FaError>((position, old_quote, PaidSeatQuote { signed, verified }))
            });
        }

        let mut refreshed = (0..recovery.seats.len()).map(|_| None).collect::<Vec<_>>();
        let mut first_error = None;
        let mut unchanged = Vec::new();
        let mut changed = Vec::new();
        while let Some(result) = pending.next().await {
            match result {
                Ok((position, old, fresh)) => {
                    let same_payment = old
                        .verified
                        .terms
                        .payment
                        .as_ref()
                        .zip(fresh.verified.terms.payment.as_ref())
                        .is_some_and(|(old, fresh)| old.federation_id() == fresh.federation_id());
                    if old.verified.terms.price_msats == fresh.verified.terms.price_msats
                        && same_payment
                    {
                        unchanged.push((position, old, fresh));
                    } else {
                        changed.push((position, old, fresh));
                    }
                }
                Err(error) => {
                    if first_error.is_some() {
                        log_mg_failure(&formation_id, &error, "discarded");
                    }
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }

        for (position, old, fresh) in unchanged {
            let seat_index = recovery.seats[position].progress.index;
            self.inner
                .store
                .refresh_authorized_quote(
                    formation_id,
                    seat_index,
                    &old.signed,
                    fresh.signed.clone(),
                )
                .await?;
            recovery.seats[position].signed_quote = Some(fresh.signed.clone());
            refreshed[position] = Some(fresh);
        }
        let any_changed = !changed.is_empty();
        for (position, old, fresh) in changed {
            let seat_index = recovery.seats[position].progress.index;
            self.inner
                .store
                .clear_quote(formation_id, seat_index, &old.signed)
                .await?;
            self.inner
                .store
                .store_quote(formation_id, seat_index, fresh.signed.clone())
                .await?;
            recovery.invalidate_payment_authorization();
            recovery.seats[position].signed_quote = Some(fresh.signed);
            recovery.seats[position].progress.phase = SeatPhase::QuoteReady;
            recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
        }
        if any_changed {
            return Err(FaError::SelectionReauthorizationRequired {
                reason: crate::SelectionReauthorizationReason::QuoteTermsChanged,
                failure: None,
            });
        }
        Ok(refreshed)
    }

    async fn run_dkg(
        &self,
        recovery: &mut FormationRecovery,
        sessions: &[SeatSession<MgClient<F::Client>>],
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let formation_id = recovery.snapshot.formation_id.clone();
        let formation_id = &formation_id;
        recovery.snapshot.phase = FormationPhase::PreparingDkg;
        self.publish_formation(recovery.snapshot.clone());

        let mut pending_codes = FuturesUnordered::new();
        for (position, session) in sessions.iter().enumerate() {
            let existing = recovery.seats[position].progress.guardian_code.clone();
            let federation_name =
                (position == 0).then(|| recovery.snapshot.federation_name.clone());
            pending_codes.push(async move {
                let code = self
                    .get_dkg_code_with_retry(
                        formation_id,
                        session,
                        fa_id,
                        federation_name,
                        existing.clone(),
                        run,
                    )
                    .await?;
                if let Some(recorded) = existing.as_ref()
                    && recorded != &code
                {
                    return Err(FaError::InvalidManifoldGuardians(format!(
                        "Manifold Guardian {} changed its deterministic guardian code",
                        position + 1
                    )));
                }
                Ok((position, code, existing.is_none()))
            });
        }
        let mut guardian_codes = (0..sessions.len())
            .map(|_| None)
            .collect::<Vec<Option<GuardianCode>>>();
        let mut first_error = None;
        while let Some(result) = pending_codes.next().await {
            let (position, code, newly_generated) = match result {
                Ok(value) => value,
                Err(error) => {
                    if first_error.is_some() {
                        log_mg_failure(&formation_id, &error, "discarded");
                    }
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            if newly_generated {
                recovery.seats[position].progress.guardian_code = Some(code.clone());
                recovery.seats[position].progress.phase = SeatPhase::GuardianCodeReady;
                recovery.seats[position].progress.freshness = FormationFreshness::Fresh;
                self.inner
                    .store
                    .record_guardian_code(
                        formation_id,
                        recovery.seats[position].progress.index,
                        code.clone(),
                    )
                    .await?;
                recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
                self.publish_formation(recovery.snapshot.clone());
            }
            guardian_codes[position] = Some(code);
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        let guardian_codes = guardian_codes
            .into_iter()
            .enumerate()
            .map(|(position, code)| {
                code.ok_or_else(|| {
                    FaError::Storage(format!("missing guardian code for seat {position}"))
                })
            })
            .collect::<FaResult<Vec<_>>>()?;

        let mut pending_starts = FuturesUnordered::new();
        for (position, session) in sessions.iter().enumerate() {
            let timestamp = Timestamp(now_secs()?);
            let completion_callback = recovery.dkg_completion_callback.clone();
            let guardian_codes = guardian_codes.clone();
            let seat_id = session.seat_id.clone();
            pending_starts.push(async move {
                let request = StartDkgRequest {
                    ts: timestamp,
                    fa_id,
                    seat_id,
                    guardian_codes,
                    completion_callback,
                };
                let request = run
                    .construct("signing StartDkg request", || self.sign(&request))
                    .await?;
                match session.client.call(run, MgOperation::StartDkg, |client| client.start_dkg(request)).await {
                    Ok(_) | Err(FaError::Mg(MgFailure {
                        cause: MgCause::WrongState(ServiceStatus::DkgInProcess | ServiceStatus::Running),
                        ..
                    })) => Ok::<_, FaError>((position, false)),
                    Err(error) => Err(error),
                }
            });
        }
        let mut first_error = None;
        while let Some(result) = pending_starts.next().await {
            match result {
                Ok((position, _)) => {
                    recovery.seats[position].progress.phase = SeatPhase::DkgUnderway;
                    recovery.seats[position].progress.freshness = FormationFreshness::Fresh;
                    recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
                }
                Err(error) => {
                    if first_error.is_some() {
                        log_mg_failure(&formation_id, &error, "discarded");
                    }
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        recovery.snapshot.phase = FormationPhase::DkgUnderway;
        self.publish_formation(recovery.snapshot.clone());
        self.poll_until_running(sessions, recovery, fa_id, run)
            .await?;

        let invite = self.fetch_agreed_invite(sessions, fa_id, run).await?;
        recovery.snapshot.invite_code = Some(invite.clone());
        // Save DKG before publication so an interrupted run never repeats it.
        self.inner
            .store
            .record_dkg_complete(formation_id, invite.clone())
            .await?;
        recovery.snapshot.phase = FormationPhase::DkgComplete;
        self.publish_formation(recovery.snapshot.clone());
        self.publish_seat_bindings(sessions, recovery, fa_id, &invite, run)
            .await?;
        self.finish_formation(fa_id).await
    }

    async fn checkpoint_accepted_seat(
        &self,
        recovery: &mut PurchaseRecovery,
        position: usize,
        acceptance: SeatAcceptance,
    ) -> FaResult<()> {
        self.inner
            .store
            .record_seat_accepted(
                &recovery.snapshot.purchase_id,
                recovery.seats[position].progress.index,
                acceptance.seat_id.clone(),
                acceptance.guardian_fee_account.clone(),
            )
            .await?;
        recovery.seats[position].progress.seat_id = Some(acceptance.seat_id);
        recovery.seats[position].guardian_fee_account = Some(acceptance.guardian_fee_account);
        recovery.seats[position].progress.phase = SeatPhase::Created;
        recovery.seats[position].progress.freshness = FormationFreshness::Fresh;
        recovery.snapshot.phase = PurchasePhase::AcquiringSeats;
        recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
        self.publish_purchase(recovery.snapshot.clone());
        Ok(())
    }

    async fn release_terminal_quote_reservation(
        &self,
        recovery: &PurchaseRecovery,
        position: usize,
        fa_id: FaId,
        release_proof: Option<P::TerminalReleaseProof>,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let signed_quote = recovery.seats[position]
            .signed_quote
            .as_ref()
            .expect("terminal quote outcome requires a stored quote");
        let verified = self.verify_quote(
            position,
            signed_quote,
            &recovery.seats[position].progress.locator,
            &recovery.snapshot.request,
            fa_id,
            recovery.payment_federation_id.as_ref(),
        )?;
        if verified.terms.payment.is_some() {
            let release_proof = release_proof.ok_or_else(|| {
                FaError::Payment(format!(
                    "terminal paid FA seat row {position} has no wallet release proof"
                ))
            })?;
            run.call("releasing terminal seat payment reservation", || {
                Ok(self
                    .inner
                    .ports
                    .payments
                    .release_seat_payment_reservation(release_proof))
            })
            .await?
            .map_err(|error| FaError::Payment(error.to_string()))?;
        } else if release_proof.is_some() {
            return Err(FaError::Storage(format!(
                "free FA seat row {position} unexpectedly carried a wallet release proof"
            )));
        }
        Ok(())
    }

    async fn clear_terminal_quotes(
        &self,
        recovery: &mut PurchaseRecovery,
        positions: &[usize],
    ) -> FaResult<()> {
        // After payment outputs started, a terminal seat leaves the purchase.
        // A new purchase buys any seat that is still missing.
        let drop_seats = recovery.payment_outputs_started;
        let expected_quotes = positions
            .iter()
            .map(|&position| {
                (
                    recovery.seats[position].progress.index,
                    recovery.seats[position]
                        .signed_quote
                        .clone()
                        .expect("terminal quote outcome requires a stored quote"),
                )
            })
            .collect::<Vec<_>>();
        self.inner
            .store
            .clear_terminal_quotes(&recovery.snapshot.purchase_id, &expected_quotes, drop_seats)
            .await?;

        recovery.invalidate_payment_authorization();
        recovery.payment_reservation_id = None;
        let mut positions = positions.to_vec();
        positions.sort_unstable();
        for &position in positions.iter().rev() {
            if drop_seats {
                recovery.seats.remove(position);
                recovery.snapshot.seats.remove(position);
            } else {
                recovery.seats[position].signed_quote = None;
                recovery.seats[position].progress.phase = SeatPhase::Selected;
                recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
            }
        }
        Ok(())
    }

    async fn complete_pending_seat_work(
        &self,
        formation_id: &FormationId,
        work: PendingSeatWork<MgClient<F::Client>, P::RefundContext, P::PaymentReservation>,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<CompletedSeatWork<MgClient<F::Client>, P::TerminalReleaseProof>> {
        match work {
            PendingSeatWork::Funding(funding) => {
                let PendingSeatFunding {
                    position,
                    quote,
                    reservation,
                    timeout_budget,
                    client,
                    locator,
                } = *funding;
                // This is intentionally the first await in funded work after
                // the durable boundary: the timeout wrapper immediately polls
                // the wallet call passed to it.
                let prepared = timeout_budget
                    .poll_value_call(
                        run,
                        self.inner
                            .ports
                            .payments
                            .create_seat_payment(&reservation, &quote.verified),
                    )
                    .await?
                    .map_err(|error| FaError::Payment(error.to_string()))?;
                let creation = self
                    .create_or_replay_seat(
                        formation_id,
                        position,
                        &client,
                        &locator,
                        fa_id,
                        SeatAcquisition::ReplayPrepared { quote, prepared },
                        run,
                    )
                    .await?;
                Ok(CompletedSeatWork {
                    position,
                    client,
                    creation,
                    checkpoint: SeatCheckpoint::Required,
                })
            }
            PendingSeatWork::Presentation(PendingSeatPresentation {
                position,
                client,
                locator: _,
                source: SeatPresentationSource::Existing(existing),
            }) => Ok(CompletedSeatWork {
                position,
                client,
                creation: SeatCreation::Accepted(existing),
                checkpoint: SeatCheckpoint::AlreadyDurable,
            }),
            PendingSeatWork::Presentation(PendingSeatPresentation {
                position,
                client,
                locator,
                source: SeatPresentationSource::Acquire(acquisition),
            }) => {
                let creation = self
                    .create_or_replay_seat(
                        formation_id,
                        position,
                        &client,
                        &locator,
                        fa_id,
                        *acquisition,
                        run,
                    )
                    .await?;
                Ok(CompletedSeatWork {
                    position,
                    client,
                    creation,
                    checkpoint: SeatCheckpoint::Required,
                })
            }
        }
    }

    async fn create_or_replay_seat(
        &self,
        formation_id: &FormationId,
        index: usize,
        client: &MgClient<F::Client>,
        locator: &Locator,
        fa_id: FaId,
        acquisition: SeatAcquisition<P::RefundContext>,
        run: DriverRun<'_>,
    ) -> FaResult<SeatCreation<P::TerminalReleaseProof>> {
        // A stored quote is reused only when payment has already started
        // against it. Otherwise obtain and persist a fresh quote before
        // committing funds, so prepared blind nonces can never be stranded.
        let presentation = match acquisition {
            SeatAcquisition::Free(quote) => {
                return self
                    .present_seat(
                        formation_id,
                        index,
                        client,
                        locator,
                        fa_id,
                        SeatPresentation::Free(quote),
                        run,
                    )
                    .await;
            }
            SeatAcquisition::ReplayPrepared { quote, prepared } => {
                SeatPresentation::Paid { quote, prepared }
            }
        };
        self.present_seat(
            formation_id,
            index,
            client,
            locator,
            fa_id,
            presentation,
            run,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn request_new_quote(
        &self,
        index: usize,
        client: &MgClient<F::Client>,
        locator: &Locator,
        intent: &ResolvedSeatPurchase,
        pinned_plan: &Plan,
        fa_id: FaId,
        policy: QuoteAttemptPolicy,
        expected_payer: Option<&FederationId>,
        payment_federation: Option<&OnceCell<FederationId>>,
        run: DriverRun<'_>,
    ) -> FaResult<(
        SignedResponse<GetQuoteResponse>,
        SignatureVerified<GetQuoteResponse>,
    )> {
        let availability = client
            .call(run, MgOperation::GetAvailability, |client| async {
                crate::mg::fold_transport(
                    self.inner
                        .ports
                        .mg_connector
                        .get_availability(client, GetAvailabilityRequest)
                        .await,
                )
            })
            .await?;
        // One shared predicate with the selection walk's live probe
        // (`selection::match_requested_availability`), so a candidate the
        // probing preview seats is exactly a candidate this gate accepts.
        // Requiring the previewed plan gives the quote exactly the previewed
        // price.
        let fedimintd_version = match crate::selection::match_requested_availability(
            &availability,
            intent.federation_size,
            &intent.fedimintd_versions,
            &intent.fedimintd_dkg_version,
            pinned_plan,
        ) {
            Ok(fedimintd_version) => fedimintd_version.clone(),
            Err(mismatch) => {
                let (cause, reason) = match mismatch {
                    AvailabilityMismatch::NotAcceptingSeats => (
                        MgCause::NotAcceptingSeats,
                        SelectionReauthorizationReason::SelectedMgUnavailable,
                    ),
                    AvailabilityMismatch::FederationSize => (
                        MgCause::UnsupportedFederationSize,
                        SelectionReauthorizationReason::SelectedMgUnavailable,
                    ),
                    AvailabilityMismatch::FedimintdVersion => (
                        MgCause::UnsupportedVersion,
                        SelectionReauthorizationReason::SelectedMgUnavailable,
                    ),
                    AvailabilityMismatch::Plan => (
                        MgCause::PlanNotOffered,
                        SelectionReauthorizationReason::PriceChanged,
                    ),
                };
                let failure = client.failure(MgOperation::GetAvailability, cause);
                return Err(if policy.allows_selection_reauthorization() {
                    FaError::SelectionReauthorizationRequired {
                        reason,
                        failure: Some(failure),
                    }
                } else {
                    failure.into()
                });
            }
        };
        let plan = pinned_plan.clone();
        // Free-ness is a property of the price, not of the plan: an MG that
        // gives its seats away is quoted against no payment federation
        // whatever this FA is configured to pay from, and one that charges
        // cannot be quoted at all by an FA with nothing to pay from.
        let priced = match &plan {
            Plan::InfiniteBestEffort { price_msats } => *price_msats > 0,
            Plan::SubscriptionBased { .. } => true,
        };
        let payment_federation_id = if priced {
            let payment_federation = payment_federation.ok_or_else(|| {
                if policy.allows_selection_reauthorization() {
                    FaError::SelectionReauthorizationRequired {
                        reason: SelectionReauthorizationReason::PaymentFederationRequired,
                        failure: None,
                    }
                } else {
                    mg_error(
                        index,
                        "Manifold Guardian charges for a seat, and this FA has no authenticated \
                         payment policy to pay from",
                    )
                }
            })?;
            Some(
                payment_federation
                    .get_or_try_init(|| async {
                        match expected_payer {
                            Some(requested) => {
                                self.require_setup_payment_federation(requested, run).await
                            }
                            None => Err(FaError::SelectionReauthorizationRequired {
                                reason: SelectionReauthorizationReason::PaymentFederationRequired,
                                failure: None,
                            }),
                        }
                    })
                    .await?
                    .clone(),
            )
        } else {
            None
        };
        let refund_issuance = match &payment_federation_id {
            Some(federation_id) => Some(
                run.call("preparing quote refund", || {
                    Ok(self
                        .inner
                        .ports
                        .payments
                        .prepare_quote_refund(federation_id, &plan))
                })
                .await?
                .map_err(|error| FaError::Payment(error.to_string()))?,
            ),
            None => None,
        };
        let quote_request = GetQuoteRequest {
            fa_id,
            fedimintd_version,
            federation_size: intent.federation_size,
            plan,
            payment_federation_id,
            refund_issuance,
        };
        let result = async {
            let signed_quote = client
                .call(run, MgOperation::GetQuote, |client| async {
                    crate::mg::fold_transport(
                        self.inner
                            .ports
                            .mg_connector
                            .get_quote(client, quote_request.clone())
                            .await,
                    )
                })
                .await?;
            let quote = self
                .verify_quote(index, &signed_quote, locator, intent, fa_id, expected_payer)
                .map_err(|_| client.failure(MgOperation::GetQuote, MgCause::InvalidResponse))?;
            // The requested plan is the previewed one, so a quote must charge
            // exactly its price.
            let plan_price_msats = match &quote_request.plan {
                Plan::InfiniteBestEffort { price_msats } => Some(*price_msats),
                Plan::SubscriptionBased { .. } => None,
            };
            if quote.terms.request != quote_request
                || plan_price_msats != Some(quote.terms.price_msats)
            {
                return Err(client
                    .failure(MgOperation::GetQuote, MgCause::InvalidResponse)
                    .into());
            }
            Ok((signed_quote, quote))
        }
        .await;
        result.map_err(|error| match error {
            FaError::Mg(failure)
                if policy.allows_selection_reauthorization()
                    && matches!(
                        failure.cause,
                        MgCause::CapacityExhausted
                            | MgCause::PlanNotOffered
                            | MgCause::PaymentFederationNotAccepted
                            | MgCause::UnsupportedVersion
                            | MgCause::UnsupportedFederationSize
                            | MgCause::InvalidResponse
                    ) =>
            {
                FaError::SelectionReauthorizationRequired {
                    reason: SelectionReauthorizationReason::SelectedMgUnavailable,
                    failure: Some(failure),
                }
            }
            error => error,
        })
    }

    fn verify_quote(
        &self,
        index: usize,
        signed_quote: &SignedResponse<GetQuoteResponse>,
        locator: &Locator,
        intent: &ResolvedSeatPurchase,
        fa_id: FaId,
        expected_payer: Option<&FederationId>,
    ) -> FaResult<SignatureVerified<GetQuoteResponse>> {
        let quote = signed_quote
            .verify(&locator.service_pubkey)
            .map_err(|error| mg_error(index, format!("invalid signed quote: {error}")))?;
        let request = &quote.terms.request;
        if request.fa_id != fa_id
            || request.fedimintd_version.dkg_version() != intent.fedimintd_dkg_version
            || !intent
                .fedimintd_versions
                .contains(&request.fedimintd_version)
            || request.federation_size != intent.federation_size
            || !intent.plan.matches(&request.plan)
        {
            return Err(mg_error(
                index,
                "stored quote does not match the formation intent",
            ));
        }
        if let Some(expected_payer) = expected_payer
            && quote
                .terms
                .payment
                .as_ref()
                .is_some_and(|payment| payment.federation_id() != expected_payer)
        {
            return Err(mg_error(
                index,
                "stored quote does not match the selected payment federation",
            ));
        }
        quote
            .terms
            .check_coherent()
            .map_err(|error| mg_error(index, format!("incoherent quote: {error}")))?;
        Ok(quote)
    }

    fn verify_free_seat_quote(
        &self,
        index: usize,
        signed: SignedResponse<GetQuoteResponse>,
        locator: &Locator,
        intent: &ResolvedSeatPurchase,
        fa_id: FaId,
        expected_payer: Option<&FederationId>,
    ) -> FaResult<FreeSeatQuote> {
        let verified = self.verify_quote(index, &signed, locator, intent, fa_id, expected_payer)?;
        if verified.terms.payment.is_some() {
            return Err(FaError::Storage(format!(
                "paid FA seat row {index} had no authorized payment action"
            )));
        }
        Ok(FreeSeatQuote { signed, verified })
    }

    fn verify_paid_seat_quote(
        &self,
        index: usize,
        signed: SignedResponse<GetQuoteResponse>,
        locator: &Locator,
        intent: &ResolvedSeatPurchase,
        fa_id: FaId,
        expected_payer: Option<&FederationId>,
    ) -> FaResult<PaidSeatQuote> {
        let verified = self.verify_quote(index, &signed, locator, intent, fa_id, expected_payer)?;
        if verified.terms.payment.is_none() {
            return Err(FaError::Storage(format!(
                "free FA seat row {index} had a paid acquisition action"
            )));
        }
        Ok(PaidSeatQuote { signed, verified })
    }

    async fn present_seat(
        &self,
        formation_id: &FormationId,
        index: usize,
        client: &MgClient<F::Client>,
        locator: &Locator,
        fa_id: FaId,
        presentation: SeatPresentation<P::RefundContext>,
        run: DriverRun<'_>,
    ) -> FaResult<SeatCreation<P::TerminalReleaseProof>> {
        let (signed_quote, quote, payment_signatures, refund_context) = match presentation {
            SeatPresentation::Free(FreeSeatQuote { signed, verified }) => {
                (signed, verified, Vec::new(), None)
            }
            SeatPresentation::Paid {
                quote: PaidSeatQuote { signed, verified },
                prepared,
            } => {
                let terms = verified
                    .terms
                    .payment
                    .as_ref()
                    .expect("paid quote type carries payment terms");
                if prepared.settled_under != terms.generation() {
                    return Err(mg_error(
                        index,
                        "wallet payment generation did not match the quote",
                    ));
                }
                (
                    signed,
                    verified,
                    prepared.payment_signatures,
                    Some(prepared.refund_context),
                )
            }
        };
        let quote_id = quote.quote_id();
        let request = CreateSeatRequest {
            ts: Timestamp(now_secs()?),
            fa_id,
            quote: signed_quote,
            payment_signatures,
        };
        let request = run
            .construct("signing CreateSeat request", || self.sign(&request))
            .await?;
        let response = client
            .call(run, MgOperation::CreateSeat, |client| {
                client.create_seat(request)
            })
            .await?
            .verify(&locator.service_pubkey)
            .map_err(|_| client.failure(MgOperation::CreateSeat, MgCause::InvalidResponse))?;
        if response.quote_id != quote_id {
            return Err(client
                .failure(MgOperation::CreateSeat, MgCause::InvalidResponse)
                .into());
        }
        match response.into_inner().outcome {
            CreateSeatOutcome::Accepted {
                seat_id,
                guardian_fee_account,
            } => {
                canonical_guardian_fee_recipient_list(&[GuardianFeeRecipient::new(
                    guardian_fee_account.clone(),
                    GUARDIAN_GUARDIAN_FEE_WEIGHT,
                )])
                .map_err(|_| client.failure(MgOperation::CreateSeat, MgCause::InvalidResponse))?;
                Ok(SeatCreation::Accepted(SeatAcceptance {
                    seat_id,
                    guardian_fee_account: guardian_fee_account.into_account(),
                }))
            }
            CreateSeatOutcome::Refused {
                reason,
                refund_transaction,
            } => {
                let cause = match reason {
                    RefusalReason::OfferChanged => MgCause::OfferChanged,
                };
                let error = client.failure(MgOperation::CreateSeat, cause);
                log_mg_failure(formation_id, &FaError::Mg(error.clone()), "pending_refund");
                let release_proof = match (refund_context, refund_transaction) {
                    (Some(refund_context), Some(refund)) => Some(
                        run.call_with_timeout(
                            "settling refused seat refund",
                            run.options.run_timeout,
                            || {
                                Ok(self
                                    .inner
                                    .ports
                                    .payments
                                    .settle_seat_refund(refund_context, refund))
                            },
                        )
                        .await?
                        .map_err(|error| FaError::Payment(error.to_string()))?
                        .release_proof,
                    ),
                    (None, None) => None,
                    _ => {
                        return Err(mg_error(
                            index,
                            "refusal payment and refund material did not match the quote",
                        ));
                    }
                };
                Ok(SeatCreation::Refused(SeatRefusal {
                    error,
                    release_proof,
                }))
            }
        }
    }

    async fn get_dkg_code_with_retry(
        &self,
        formation_id: &FormationId,
        session: &SeatSession<MgClient<F::Client>>,
        fa_id: FaId,
        federation_name: Option<FederationName>,
        recorded_code: Option<GuardianCode>,
        run: DriverRun<'_>,
    ) -> FaResult<GuardianCode> {
        loop {
            ensure_time_remaining(run.deadline, "waiting for MG child readiness")?;
            let request = GetDkgCodeRequest {
                ts: Timestamp(now_secs()?),
                fa_id,
                seat_id: session.seat_id.clone(),
                federation_name: federation_name.clone(),
            };
            let request = run
                .construct("signing GetDkgCode request", || self.sign(&request))
                .await?;
            match session
                .client
                .call(run, MgOperation::GetDkgCode, |client| {
                    client.get_dkg_code(request)
                })
                .await
            {
                Ok(response) => return Ok(response.guardian_code),
                Err(
                    error @ FaError::Mg(MgFailure {
                        cause: MgCause::SeatUnavailable,
                        ..
                    }),
                ) => {
                    sleep_for_retry(run.deadline, run.options.poll_interval)
                        .await
                        .map_err(|_| {
                            session
                                .client
                                .failure(MgOperation::GetDkgCode, MgCause::Timeout)
                        })?;
                    log_mg_failure(formation_id, &error, "retried");
                }
                Err(FaError::Mg(MgFailure {
                    cause: MgCause::WrongState(ServiceStatus::DkgInProcess | ServiceStatus::Running),
                    ..
                })) if recorded_code.is_some() => {
                    return Ok(recorded_code.expect("matched present recorded code"));
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn poll_until_running(
        &self,
        sessions: &[SeatSession<MgClient<F::Client>>],
        recovery: &mut FormationRecovery,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let formation_id = recovery.snapshot.formation_id.clone();
        let formation_id = &formation_id;
        loop {
            ensure_time_remaining(run.deadline, "waiting for every MG to report running")?;
            let mut waiting = Vec::new();
            let mut running = 0;
            let mut pending_status = FuturesUnordered::new();
            for (position, session) in sessions.iter().enumerate() {
                let request = GetStatusRequest {
                    ts: Timestamp(now_secs()?),
                    fa_id,
                    seat_id: session.seat_id.clone(),
                };
                pending_status.push(async move {
                    let request = run
                        .construct("signing GetStatus request", || self.sign(&request))
                        .await?;
                    let status = session
                        .client
                        .call(run, MgOperation::GetStatus, |client| {
                            client.get_status(request)
                        })
                        .await?;
                    Ok::<_, FaError>((position, status))
                });
            }
            while let Some(result) = pending_status.next().await {
                let (position, status) = result?;
                recovery.seats[position].progress.phase = service_seat_phase(&status.status)
                    .map_err(|cause| {
                        sessions[position]
                            .client
                            .failure(MgOperation::GetStatus, cause)
                    })?;
                recovery.seats[position].progress.freshness = FormationFreshness::Fresh;
                recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
                if status.status == ServiceStatus::Running
                    && status.seat_health == Some(SeatHealth::Healthy)
                {
                    running += 1;
                } else {
                    waiting.push(&sessions[position]);
                }
            }
            recovery.snapshot.freshness = FormationFreshness::Fresh;
            self.publish_formation(recovery.snapshot.clone());
            if running == sessions.len() {
                return Ok(());
            }
            sleep_for_retry(run.deadline, run.options.poll_interval)
                .await
                .inspect_err(|_| {
                    for session in &waiting {
                        log_mg_failure(
                            formation_id,
                            &session
                                .client
                                .failure(MgOperation::WaitForRunning, MgCause::NotRunning)
                                .into(),
                            "returned",
                        );
                    }
                })?;
        }
    }

    async fn resume_after_dkg(
        &self,
        recovery: &mut FormationRecovery,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let stored = recovery.snapshot.invite_code.clone().ok_or_else(|| {
            FaError::Storage("formed FA record contains no persisted invite".to_owned())
        })?;
        let manager_connections = self.seat_sessions(recovery, run).await?;
        self.poll_until_running(&manager_connections, recovery, fa_id, run)
            .await?;
        let invite = self
            .fetch_agreed_invite(&manager_connections, fa_id, run)
            .await?;
        if invite_federation_id(&stored)? != invite_federation_id(&invite)? {
            return Err(FaError::InvalidManifoldGuardians(
                "formed federation identity changed during reconciliation".to_owned(),
            ));
        }
        self.publish_seat_bindings(&manager_connections, recovery, fa_id, &invite, run)
            .await?;
        self.finish_formation(fa_id).await
    }

    /// Publish a formed federation as fresh, in place of its formation.
    async fn finish_formation(&self, fa_id: FaId) -> FaResult<()> {
        let mut federation = self.inner.store.load_federation(fa_id).await?;
        if let Some(federation) = federation.as_mut() {
            federation.freshness = FormationFreshness::Fresh;
        }
        self.inner.progress.modify(|status| {
            status.creation = None;
            status.pool = Vec::new();
            status.formation = None;
            status.federation = federation;
        });
        Ok(())
    }

    /// Recheck the stored federation against consensus.
    pub(crate) async fn reconcile_federation(
        &self,
        snapshot: crate::FederationSnapshot,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let seats = snapshot
            .seats
            .iter()
            .map(|seat| crate::liquidity::FormedMgSeat {
                mg_id: seat.mg_id,
                locator: seat.locator.clone(),
                seat_id: seat.seat_id.clone(),
            })
            .collect();
        let federation_name = self
            .verify_federation(&snapshot.federation_invite, seats, None, run)
            .await?;
        self.inner
            .store
            .record_federation_name(fa_id, federation_name)
            .await?;
        let mut federation = self.inner.store.load_federation(fa_id).await?;
        if let Some(federation) = federation.as_mut() {
            federation.freshness = FormationFreshness::Fresh;
        }
        self.publish_federation(federation);
        Ok(())
    }

    /// Look up the backup of a restored mnemonic and import it.
    pub(crate) async fn recover_backup(&self, fa_id: FaId, run: DriverRun<'_>) -> FaResult<()> {
        let source = self
            .inner
            .backup_source
            .as_ref()
            .expect("only a client with a backup source starts in recovery");
        let root = self.inner.ports.identity.scoped_root();
        let payload = run
            .call_with_timeout("reading FA backup relays", run.options.run_timeout, || {
                Ok(crate::backup_worker::fetch_backup(
                    &root,
                    &source.relays,
                    source.required_reads,
                ))
            })
            .await??;
        self.import_backup(payload, &source.completed, fa_id, run)
            .await
    }

    /// Import a fetched backup once consensus confirms it, and mark the
    /// lookup done. The backup stays in memory until then, so a failed check
    /// leaves nothing behind.
    pub(crate) async fn import_backup(
        &self,
        payload: Option<crate::backup::FaBackupPayload>,
        completed: &crate::db::RecoveryCompletedKey,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        let Some(payload) = payload else {
            self.inner.store.complete_empty_recovery(completed).await?;
            self.inner
                .progress
                .replace(crate::state::FaStatus::default());
            return Ok(());
        };
        let seats = payload
            .seats
            .iter()
            .map(|seat| crate::liquidity::FormedMgSeat {
                mg_id: seat.mg_identity,
                locator: seat.locator.clone(),
                seat_id: seat.seat_id.clone(),
            })
            .collect();
        let federation_name = self
            .verify_federation(
                &payload.federation_invite,
                seats,
                payload.liquidity.as_ref(),
                run,
            )
            .await?;
        self.inner
            .store
            .import_backup(fa_id, payload, federation_name, completed)
            .await?;
        let mut federation = self.inner.store.load_federation(fa_id).await?;
        if let Some(federation) = federation.as_mut() {
            federation.freshness = FormationFreshness::Fresh;
        }
        self.inner.progress.replace(crate::state::FaStatus {
            federation,
            ..crate::state::FaStatus::default()
        });
        Ok(())
    }

    /// Check a federation against consensus: its identity, seat-binding
    /// directory, seats, and liquidity commitment. Return its consensus name.
    async fn verify_federation(
        &self,
        invite: &InviteCode,
        seats: Vec<crate::liquidity::FormedMgSeat>,
        liquidity: Option<
            &fedi_decentralized_service_liquidity_manager::RequestLiquidityDetailsCommitmentV1,
        >,
        run: DriverRun<'_>,
    ) -> FaResult<Option<crate::FederationName>> {
        if seats.is_empty() {
            return Err(FaError::InvalidManifoldGuardians(
                "restored FA backup contains no seats".to_owned(),
            ));
        }
        let consensus = self
            .read_recovery_consensus(invite, &seats, run)
            .await?
            .map_err(|error| {
                FaError::InvalidManifoldGuardians(format!(
                    "reading restored federation consensus failed: {error}"
                ))
            })?;
        verify_consensus_identity(&consensus, invite)?;
        let federation = federation_seats(&consensus.config).map_err(|error| {
            FaError::InvalidManifoldGuardians(format!(
                "restored federation config is not usable: {error}"
            ))
        })?;
        let directory = crate::liquidity::liquidity_seat_bindings_field(&consensus.meta_value)?
            .ok_or_else(|| {
                FaError::InvalidManifoldGuardians(
                    "restored federation has no MG seat-binding directory".to_owned(),
                )
            })?;
        let bindings = MgSeatBindings::parse_canonical(&directory)
            .and_then(|bindings| bindings.verify_for_federation(&federation))
            .map_err(|error| {
                FaError::InvalidManifoldGuardians(format!(
                    "restored federation seat-binding directory is invalid: {error}"
                ))
            })?;
        let federation_name = federation_name(&consensus)?;
        let validation_name = federation_name
            .clone()
            .or_else(|| liquidity.map(|value| value.federation_details.federation_name.clone()))
            .unwrap_or_else(|| crate::FederationName(String::new()));
        let context = crate::liquidity::FormedLiquidityContext {
            federation_name: validation_name,
            invite_code: invite.clone(),
            network: consensus.network,
            federation,
            bindings,
            seats,
        };
        context.fleet_seat_hints().map_err(|error| {
            FaError::InvalidManifoldGuardians(format!(
                "restored seats do not match federation consensus: {error}"
            ))
        })?;
        if let Some(commitment) = liquidity {
            context.matches_commitment(commitment)?;
        }

        self.remember_read_invite(invite, &consensus).await?;
        Ok(federation_name)
    }

    async fn seat_sessions(
        &self,
        recovery: &FormationRecovery,
        run: DriverRun<'_>,
    ) -> FaResult<Vec<SeatSession<MgClient<F::Client>>>> {
        let mut sessions = Vec::with_capacity(recovery.seats.len());
        for (position, seat) in recovery.seats.iter().enumerate() {
            let seat_id = seat.progress.seat_id.clone().ok_or_else(|| {
                FaError::Storage(format!("FA seat row {position} has no seat id"))
            })?;

            let client =
                MgClient::connect(&self.inner.ports.mg_connector, &seat.progress.locator, run)
                    .await?;
            sessions.push(SeatSession {
                index: seat.progress.index,
                client,
                seat_id,
            });
        }
        Ok(sessions)
    }

    /// Publish the directory and initial fee policy as one formation vote.
    async fn publish_seat_bindings(
        &self,
        sessions: &[SeatSession<MgClient<F::Client>>],
        recovery: &mut FormationRecovery,
        fa_id: FaId,
        invite: &InviteCode,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        // A new target must be validated against the final config before it is
        // durable: recovery replays its exact bytes and cannot repair an
        // invalid directory after pinning it. This read validates the signed
        // directory only; recipient derivation no longer uses API keys.
        let formation_config = if recovery.formation_meta_target.is_none() {
            loop {
                ensure_time_remaining(run.deadline, "reading the formation config")?;
                match run
                    .call("reading the formation config", || {
                        Ok(self.inner.ports.consensus_reader.read_consensus(invite))
                    })
                    .await?
                {
                    Ok(snapshot) => break Some(snapshot.config),
                    Err(_) => sleep_for_retry(run.deadline, run.options.poll_interval).await?,
                }
            }
        } else {
            None
        };
        let target = match recovery.formation_meta_target.clone() {
            Some(target) => target,
            None => {
                let (bindings, binding_entries) = self
                    .assemble_seat_bindings(sessions, recovery, fa_id, run)
                    .await?;
                let federation_id = invite_federation_id(invite)?;
                let fa_account = self
                    .inner
                    .ports
                    .fa_fee_account_provider
                    .formed_federation_fee_account(&federation_id)
                    .map_err(|_| {
                        FaError::CapabilityUnavailable(crate::Capability::FeeArrangement)
                    })?;
                let guardian_verification_fee_account =
                    self.inner.guardian_verification_fee_account.clone().ok_or(
                        FaError::CapabilityUnavailable(crate::Capability::FeeArrangement),
                    )?;
                let recipients = canonical_fee_recipients(
                    formation_config
                        .as_ref()
                        .expect("a new formation target has a downloaded config"),
                    &bindings,
                    fa_account.clone(),
                    guardian_verification_fee_account.clone(),
                )?;
                let recipients =
                    canonical_guardian_fee_recipient_list(&recipients).map_err(|error| {
                        FaError::InvalidManifoldGuardians(format!(
                            "guardian-fee recipient accounts cannot form canonical metadata: {error}"
                        ))
                    })?;
                let send_ppm = self
                    .min_guardian_fee_ppm()
                    .await
                    .max(u64::from(GuardianFeePpm::MANIFOLD_DEFAULT.value()));
                let fa_fee_account = GuardianFeeAccount::try_from(fa_account).map_err(|error| {
                    FaError::InvalidIntent(format!("FA guardian-fee account is invalid: {error}"))
                })?;
                let guardian_verification_fee_account = GuardianFeeAccount::try_from(
                    guardian_verification_fee_account,
                )
                .map_err(|_| FaError::CapabilityUnavailable(crate::Capability::FeeArrangement))?;
                let target = FormationMetaTarget {
                    seat_bindings: bindings,
                    binding_entries,
                    fa_fee_account,
                    guardian_verification_fee_account,
                    send_ppm,
                    recipients,
                };
                run.call("recording the formation metadata target", || {
                    Ok(self.inner.store.record_formation_meta_target(
                        &recovery.snapshot.formation_id,
                        target.clone(),
                    ))
                })
                .await??;
                recovery.formation_meta_target = Some(target.clone());
                target
            }
        };

        recovery.snapshot.phase = FormationPhase::PublishingSeatBindings;
        self.publish_formation(recovery.snapshot.clone());
        let formation_id = &recovery.snapshot.formation_id;
        let target = &target;
        let seats = sessions.iter().map(|session| session.index).collect();
        let snapshot = write_consensus_meta(
            run,
            seats,
            || {
                run.call("reading the formation metadata base", || {
                    Ok(self.inner.ports.consensus_reader.read_consensus(invite))
                })
            },
            |snapshot| {
                Ok(self.seat_bindings_match(snapshot, &target.seat_bindings)?
                    && guardian_fee_recipients_match(
                        snapshot.meta_value.as_deref(),
                        &target.recipients,
                    )?
                    && guardian_fee_rate_matches(snapshot.meta_value.as_deref(), target.send_ppm)?)
            },
            |rows, expected_base| async move {
                let mut votes = Vec::with_capacity(rows.len());
                for (index, result) in self
                    .submit_formation_meta_wave(sessions, &rows, fa_id, expected_base, target, run)
                    .await
                {
                    let vote = match result {
                        Ok(_) => SeatVote::Accepted,
                        Err(FaError::Mg(MgFailure {
                            cause: MgCause::MetaConsensusChanged,
                            ..
                        })) => SeatVote::Retry("consensus metadata base changed".to_owned()),
                        Err(
                            error @ FaError::Mg(MgFailure {
                                cause:
                                    MgCause::GuardianVerificationFeeAccountUnavailable
                                    | MgCause::GuardianVerificationFeeAccountMismatch,
                                ..
                            }),
                        ) => return Err(error),
                        Err(error) => {
                            log_mg_failure(formation_id, &error, "pending_consensus");
                            SeatVote::Rejected(error)
                        }
                    };
                    votes.push((index, vote));
                }
                Ok(votes)
            },
        )
        .await
        .map_err(|error| match error {
            MetaWriteError::Fatal(error) | MetaWriteError::Rejected { reason: error, .. } => error,
            MetaWriteError::TooLarge {
                actual_bytes,
                max_bytes,
            } => formation_metadata_too_large(ConsensusMetadataTooLarge {
                actual_bytes,
                max_bytes,
            }),
            MetaWriteError::InvalidBase(reason) => FaError::InvalidManifoldGuardians(reason),
            MetaWriteError::Unconverged { .. } => {
                FaError::Timeout("waiting for formation metadata consensus".to_owned())
            }
        })?;
        run.call("confirming formation metadata consensus", || {
            Ok(self
                .inner
                .store
                .finish_formation(&recovery.snapshot.formation_id))
        })
        .await??;
        self.remember_read_invite(invite, &snapshot).await
    }

    /// Collect one MG attestation and endpoint proof per seat.
    async fn assemble_seat_bindings(
        &self,
        sessions: &[SeatSession<MgClient<F::Client>>],
        recovery: &FormationRecovery,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<(String, Vec<FormationSeatBinding>)> {
        let mut pending = FuturesUnordered::new();
        for (position, session) in sessions.iter().enumerate() {
            let expected_guardian_fee_account =
                recovery.seats[position].guardian_fee_account.clone();

            pending.push(async move {
                let request = GetPeerAttestationRequest {
                    ts: Timestamp(now_secs()?),
                    fa_id,
                    seat_id: session.seat_id.clone(),
                };
                let request = run
                    .construct("signing GetPeerAttestation request", || self.sign(&request))
                    .await?;
                let response = session
                    .client
                    .call(run, MgOperation::GetPeerAttestation, |client| {
                        client.get_peer_attestation(request)
                    })
                    .await?;
                if response
                    .mg_peer_attestation
                    .attestation
                    .guardian_fee_account
                    != expected_guardian_fee_account
                {
                    return Err(session
                        .client
                        .failure(MgOperation::GetPeerAttestation, MgCause::InvalidResponse)
                        .into());
                }
                Ok::<_, FaError>(FormationSeatBinding {
                    attestation: response.mg_peer_attestation,
                    endpoint_proof: response.seat_endpoint_proof,
                })
            });
        }
        let mut entries = Vec::with_capacity(sessions.len());
        while let Some(result) = pending.next().await {
            entries.push(result?);
        }

        let bindings = MgSeatBindings::new(entries.iter().map(|entry| entry.attestation.clone()))
            .and_then(|bindings| bindings.canonical_string())
            .map_err(|error| {
                FaError::InvalidManifoldGuardians(format!(
                    "Manifold Guardian attestations do not form a valid seat-binding directory: {error}"
                ))
            })?;
        Ok((bindings, entries))
    }

    async fn submit_formation_meta_wave(
        &self,
        sessions: &[SeatSession<MgClient<F::Client>>],
        rows: &BTreeSet<u16>,
        fa_id: FaId,
        expected_base: MetaConsensusBase,
        target: &FormationMetaTarget,
        run: DriverRun<'_>,
    ) -> Vec<(u16, FaResult<ProposeFormationMetaResponse>)> {
        let mut pending = FuturesUnordered::new();
        for session in sessions
            .iter()
            .filter(|session| rows.contains(&session.index))
        {
            let vote = async move {
                let request = ProposeFormationMetaRequest {
                    ts: Timestamp(now_secs()?),
                    fa_id,
                    seat_id: session.seat_id.clone(),
                    expected_base,
                    seat_bindings: target.binding_entries.clone(),
                    fa_fee_account: target.fa_fee_account.clone(),
                    guardian_verification_fee_account: target
                        .guardian_verification_fee_account
                        .clone(),
                    send_ppm: target.send_ppm,
                };
                let request = run
                    .construct("signing ProposeFormationMeta request", || {
                        self.sign(&request)
                    })
                    .await?;
                session
                    .client
                    .call(run, MgOperation::ProposeFormationMeta, |client| {
                        client.propose_formation_meta(request)
                    })
                    .await
            };
            pending.push(async move { (session.index, vote.await) });
        }
        let mut results = Vec::with_capacity(rows.len());
        while let Some(result) = pending.next().await {
            results.push(result);
        }
        results
    }

    /// Submit one guarded whole-object metadata wave through the supplied seats.
    ///
    /// The primitive deliberately returns every seat result instead of
    /// short-circuiting. Formation can require all seats while maintenance can
    /// read consensus after a threshold-live partial wave and retry missing
    /// guardians under its deadline.
    pub(crate) async fn submit_meta_field_wave(
        &self,
        sessions: &[SeatSession<F::Client>],
        fa_id: FaId,
        expected_base: MetaConsensusBase,
        key: &MetaFieldKey,
        value: &MetaFieldValue,
        run: DriverRun<'_>,
    ) -> Vec<(u16, Result<MetaFieldSubmission, MetaFieldSubmissionError>)> {
        let mut pending = FuturesUnordered::new();
        for session in sessions {
            pending.push(async move {
                let result = async {
                    let request = SetMetaFieldRequest {
                        ts: Timestamp(now_secs().map_err(MetaFieldSubmissionError::Driver)?),
                        fa_id,
                        seat_id: session.seat_id.clone(),
                        expected_base,
                        key: key.clone(),
                        value: value.clone(),
                    };
                    let request = run
                        .construct("signing SetMetaField request", || self.sign(&request))
                        .await
                        .map_err(MetaFieldSubmissionError::Driver)?;
                    let response = run
                        .call("submitting SetMetaField proposal", || {
                            Ok(session.client.set_meta_field(request))
                        })
                        .await
                        .map_err(MetaFieldSubmissionError::Driver)?;
                    match response {
                        Ok(_) => Ok(MetaFieldSubmission::Accepted),
                        Err(ManifoldGuardianError::MetaConsensusChanged) => {
                            Ok(MetaFieldSubmission::BaseChanged)
                        }
                        Err(error) => Err(MetaFieldSubmissionError::ManifoldGuardian(error)),
                    }
                }
                .await;
                (session.index, result)
            });
        }
        let mut results = Vec::with_capacity(sessions.len());
        while let Some(result) = pending.next().await {
            results.push(result);
        }
        results
    }

    /// Whether consensus carries the expected directory, fully verified.
    fn seat_bindings_match(
        &self,
        snapshot: &FederationConsensusSnapshot,
        expected: &str,
    ) -> FaResult<bool> {
        let Some(value) = seat_bindings_field(&snapshot.meta_value)? else {
            return Ok(false);
        };
        if value != expected {
            return Ok(false);
        }

        // Equality alone would accept whatever this run happened to write.
        // Re-deriving the peer set from the downloaded config and verifying
        // every attestation against it is what makes the readback a check on
        // the federation rather than on our own memory.
        let seats = federation_seats(&snapshot.config).map_err(|error| {
            FaError::InvalidManifoldGuardians(format!(
                "previewed federation config is not usable: {error}"
            ))
        })?;
        MgSeatBindings::parse_canonical(&value)
            .and_then(|bindings| {
                bindings.verify_for_federation(&seats)?;
                Ok(())
            })
            .map_err(|error| {
                FaError::InvalidManifoldGuardians(format!(
                    "consensus seat-binding directory does not match the federation: {error}"
                ))
            })?;
        Ok(true)
    }

    /// Choose remembered guardian addresses for this saved invite, if available.
    /// Otherwise return the saved invite. Only connection information is reused;
    /// callers must still read fresh consensus.
    pub(crate) async fn consensus_invite(&self, saved: &InviteCode) -> InviteCode {
        self.inner
            .read_invite
            .lock()
            .await
            .as_ref()
            .filter(|(original, _)| original == saved)
            .map(|(_, invite)| invite.clone())
            .unwrap_or_else(|| saved.clone())
    }

    /// Remember several guardian addresses from a verified consensus snapshot.
    /// Call after verifying the directory and saved authority. This checks the
    /// federation identity again before storing the connection hint in memory;
    /// the saved invite, backups, and signed liquidity request stay unchanged.
    async fn remember_read_invite(
        &self,
        saved: &InviteCode,
        snapshot: &FederationConsensusSnapshot,
    ) -> FaResult<()> {
        verify_consensus_identity(snapshot, saved)?;
        let peers = snapshot
            .config
            .global
            .api_endpoints
            .iter()
            .map(|(peer, endpoint)| (*peer, endpoint.url.clone()))
            .collect();
        let parsed = FedimintInviteCode::from_str(&saved.0)
            .map_err(|error| FaError::InvalidIntent(error.to_string()))?;
        let invite =
            FedimintInviteCode::from_map(&peers, parsed.federation_id(), parsed.api_secret());
        *self.inner.read_invite.lock().await =
            Some((saved.clone(), InviteCode(invite.to_string())));
        Ok(())
    }

    /// Read fresh consensus, trying the invites of `seats` if the first read
    /// fails. Alternative invites and returned configurations must name the
    /// saved federation. Calls share the driver deadlines, and remaining
    /// attempts are dropped once one succeeds. The caller must still verify the
    /// directory and saved authority before marking recovery fresh.
    async fn read_recovery_consensus(
        &self,
        saved: &InviteCode,
        seats: &[crate::liquidity::FormedMgSeat],
        run: DriverRun<'_>,
    ) -> FaResult<Result<FederationConsensusSnapshot, crate::FederationConsensusError>> {
        let invite = self.consensus_invite(saved).await;
        let first = run
            .call("reading federation recovery consensus", || {
                Ok(self.inner.ports.consensus_reader.read_consensus(&invite))
            })
            .await;
        match &first {
            Ok(Ok(_)) => return first,
            Err(error) if !matches!(error, FaError::Timeout(_)) => return first,
            _ => {}
        }

        // A manager supplies another way to dial, never a trust verdict.
        // Race the saved managers so one unavailable manager cannot hold up the rest.
        let fa_id = self.fa_id()?;
        let expected = invite_federation_id(saved)?;
        let mut pending = FuturesUnordered::new();
        for seat in seats {
            pending.push(async move {
                let client =
                    MgClient::connect(&self.inner.ports.mg_connector, &seat.locator, run).await?;
                let request = run
                    .construct("signing a recovery invite request", || {
                        self.sign(&GetInviteCodeRequest {
                            ts: Timestamp(now_secs()?),
                            fa_id,
                            seat_id: seat.seat_id.clone(),
                        })
                    })
                    .await?;
                let invite = client
                    .call(run, MgOperation::GetInviteCode, |client| {
                        client.get_invite_code(request)
                    })
                    .await?
                    .invite_code;
                if invite_federation_id(&invite)? != expected {
                    return Err(FaError::InvalidManifoldGuardians(
                        "recovery invite names another federation".to_owned(),
                    ));
                }
                let snapshot = run
                    .call("reading alternate recovery consensus", || {
                        Ok(self.inner.ports.consensus_reader.read_consensus(&invite))
                    })
                    .await?
                    .map_err(|error| FaError::InvalidManifoldGuardians(error.to_string()))?;
                verify_consensus_identity(&snapshot, saved)?;
                Ok::<_, FaError>(snapshot)
            });
        }
        while let Some(result) = pending.next().await {
            if let Ok(snapshot) = result {
                return Ok(Ok(snapshot));
            }
        }
        first
    }

    async fn fetch_agreed_invite(
        &self,
        sessions: &[SeatSession<MgClient<F::Client>>],
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<InviteCode> {
        if sessions.is_empty() {
            return Err(FaError::InvalidManifoldGuardians(
                "no Manifold Guardians selected".to_owned(),
            ));
        }
        let mut pending = FuturesUnordered::new();
        for (position, session) in sessions.iter().enumerate() {
            pending.push(async move {
                let request = GetInviteCodeRequest {
                    ts: Timestamp(now_secs()?),
                    fa_id,
                    seat_id: session.seat_id.clone(),
                };
                let request = run
                    .construct("signing GetInviteCode request", || self.sign(&request))
                    .await?;
                let invite = session
                    .client
                    .call(run, MgOperation::GetInviteCode, |client| {
                        client.get_invite_code(request)
                    })
                    .await?
                    .invite_code;
                let federation_id = invite_federation_id(&invite).map_err(|_| {
                    session
                        .client
                        .failure(MgOperation::GetInviteCode, MgCause::InvalidResponse)
                })?;
                Ok::<_, FaError>((position, invite, federation_id))
            });
        }
        let mut invites = (0..sessions.len())
            .map(|_| None)
            .collect::<Vec<Option<(InviteCode, FedimintFederationId)>>>();
        while let Some(result) = pending.next().await {
            let (position, invite, federation_id) = result?;
            invites[position] = Some((invite, federation_id));
        }
        let mut invites = invites.into_iter().enumerate().map(|(position, invite)| {
            invite.ok_or_else(|| {
                FaError::Storage(format!("missing invite response for seat {position}"))
            })
        });
        let (deliverable, expected_id) = invites
            .next()
            .ok_or_else(|| FaError::Storage("formed FA record contains no seats".to_owned()))??;
        for invite in invites {
            let (_, federation_id) = invite?;
            if federation_id != expected_id {
                return Err(FaError::InvalidManifoldGuardians(
                    "selected Manifold Guardians reported different federation identities"
                        .to_owned(),
                ));
            }
        }
        Ok(deliverable)
    }

    pub(crate) async fn active_recovery(&self, fa_id: FaId) -> FaResult<PurchaseRecovery> {
        self.inner
            .store
            .load_purchase(fa_id)
            .await?
            .ok_or(FaError::NoActiveFormation)
    }

    /// The running formation.
    pub(crate) async fn active_formation(&self, fa_id: FaId) -> FaResult<FormationRecovery> {
        self.inner
            .store
            .load_formation(fa_id)
            .await?
            .ok_or(FaError::NoActiveFormation)
    }

    pub(crate) async fn post_formed_authority(&self, fa_id: FaId) -> FaResult<PostFormedAuthority> {
        self.inner.store.post_formed_authority(fa_id).await
    }

    fn publish_purchase(&self, snapshot: SeatPurchaseSnapshot) {
        self.inner
            .progress
            .modify(|status| status.purchase = Some(snapshot));
    }

    fn publish_formation(&self, snapshot: FormationSnapshot) {
        self.inner
            .progress
            .modify(|status| status.formation = Some(snapshot));
    }

    pub(crate) fn publish_federation(&self, federation: Option<crate::FederationSnapshot>) {
        self.inner
            .progress
            .modify(|status| status.federation = federation);
    }

    pub(crate) async fn publish_pool(&self) {
        let pool = self.inner.store.pool_seats().await;
        self.inner.progress.modify(|status| status.pool = pool);
    }

    /// Drive a formation from pool seats to a formed federation.
    pub(crate) async fn drive_formation(
        &self,
        mut recovery: FormationRecovery,
        fa_id: FaId,
        run: DriverRun<'_>,
    ) -> FaResult<()> {
        self.publish_formation(recovery.snapshot.clone());
        let result = if recovery.snapshot.phase.dkg_complete() {
            self.resume_after_dkg(&mut recovery, fa_id, run).await
        } else {
            match self.seat_sessions(&recovery, run).await {
                Ok(sessions) => self.run_dkg(&mut recovery, &sessions, fa_id, run).await,
                Err(error) => Err(error),
            }
        };
        if let Err(error) = &result {
            log_mg_failure(&recovery.snapshot.formation_id, error, "returned");
            recovery.snapshot.last_error = Some(error.code());
            self.publish_formation(recovery.snapshot);
        }
        result
    }

    /// A recheck withdraws the federation's freshness until it succeeds.
    pub(crate) fn begin_recheck(&self) {
        self.inner.progress.modify(|status| {
            if let Some(federation) = &mut status.federation {
                federation.freshness = FormationFreshness::Unsynced;
            }
        });
    }

    pub(crate) fn fail_recheck(&self, error: &FaError) {
        self.inner.progress.modify(|status| {
            if let Some(federation) = &mut status.federation {
                federation.freshness = FormationFreshness::Unsynced;
                federation.last_error = Some(error.code());
            }
        });
    }

    pub(crate) fn fa_id(&self) -> FaResult<FaId> {
        self.inner
            .ports
            .identity
            .public_key()
            .map_err(FaError::Identity)
    }

    pub(crate) fn sign<T: FaSignedRequest>(&self, request: &T) -> FaResult<SignedRequest<T>> {
        SignedRequest::create_with_signer(request, |digest| {
            self.inner.ports.identity.sign_digest(digest)
        })
        .map_err(|error| FaError::Identity(error.to_string()))
    }
}

/// Reject a configuration whose federation id differs from the saved invite.
/// This checks identity only; the caller verifies the guardian directory separately.
fn verify_consensus_identity(
    snapshot: &FederationConsensusSnapshot,
    invite: &InviteCode,
) -> FaResult<()> {
    if snapshot.config.calculate_federation_id() != invite_federation_id(invite)? {
        return Err(FaError::InvalidManifoldGuardians(
            "consensus federation identity differs from the saved invite".to_owned(),
        ));
    }
    Ok(())
}

/// A rename overrides the original name in the client config.
pub(crate) fn federation_name(
    consensus: &FederationConsensusSnapshot,
) -> FaResult<Option<crate::FederationName>> {
    let fields = consensus
        .meta_value
        .as_deref()
        .map(serde_json::from_slice::<BTreeMap<String, serde_json::Value>>)
        .transpose()
        .map_err(|error| {
            FaError::InvalidManifoldGuardians(format!("federation metadata is invalid: {error}"))
        })?
        .unwrap_or_default();
    match fields.get(FEDERATION_NAME_META_FIELD_KEY) {
        None => Ok(consensus
            .config
            .global
            .meta
            .get(FEDERATION_NAME_META_FIELD_KEY)
            .map(|name| crate::FederationName(name.clone()))),
        Some(serde_json::Value::String(value)) => Ok(Some(crate::FederationName(value.clone()))),
        Some(_) => Err(FaError::InvalidManifoldGuardians(
            "federation name metadata is not a string".to_owned(),
        )),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReservationCleanup {
    /// The wallet reserve call returned an error and no capability exists.
    DefinitivelyAbsent,
    /// A wallet commit may have preceded an interrupted FA checkpoint.
    ReconstructIfAuthorized,
}

fn canonical_fee_recipients(
    config: &ClientConfig,
    seat_bindings: &str,
    fa_account: Account,
    guardian_verification_fee_account: Account,
) -> FaResult<Vec<GuardianFeeRecipient>> {
    let fa_fee_account = GuardianFeeAccount::try_from(fa_account.clone()).map_err(|error| {
        FaError::InvalidIntent(format!("FA guardian-fee account is invalid: {error}"))
    })?;
    let guardian_verification_fee_account =
        GuardianFeeAccount::try_from(guardian_verification_fee_account.clone())
            .map_err(|_| FaError::CapabilityUnavailable(crate::Capability::FeeArrangement))?;
    let federation = federation_seats(config).map_err(|error| {
        FaError::InvalidManifoldGuardians(format!("formed federation config is invalid: {error}"))
    })?;
    let bindings = MgSeatBindings::parse_canonical(seat_bindings)
        .and_then(|bindings| bindings.verify_for_federation(&federation))
        .map_err(|error| {
            FaError::InvalidManifoldGuardians(format!(
                "seat-binding directory does not match the formed config: {error}"
            ))
        })?;

    let mut recipients = Vec::with_capacity(bindings.len() + 2);
    for binding in bindings {
        let guardian_account =
            GuardianFeeAccount::try_from(binding.guardian_fee_account).map_err(|error| {
                FaError::InvalidManifoldGuardians(format!(
                    "guardian {} has an invalid signed guardian-fee account: {error}",
                    binding.peer_id.0
                ))
            })?;
        recipients.push(GuardianFeeRecipient::new(
            guardian_account,
            GUARDIAN_GUARDIAN_FEE_WEIGHT,
        ));
    }
    recipients.push(GuardianFeeRecipient::new(
        fa_fee_account,
        FA_GUARDIAN_FEE_WEIGHT,
    ));
    recipients.push(GuardianFeeRecipient::new(
        guardian_verification_fee_account,
        GUARDIAN_VERIFICATION_FEE_WEIGHT,
    ));
    recipients.sort_by_key(|recipient| recipient.account.as_account().id());
    canonical_guardian_fee_recipient_list(&recipients).map_err(|error| {
        FaError::InvalidManifoldGuardians(format!(
            "guardian-fee accounts do not form a canonical recipient set: {error}"
        ))
    })?;
    Ok(recipients)
}

fn service_seat_phase(status: &ServiceStatus) -> Result<SeatPhase, MgCause> {
    match status {
        ServiceStatus::New => Ok(SeatPhase::Created),
        ServiceStatus::DkgInProcess => Ok(SeatPhase::DkgUnderway),
        ServiceStatus::Running => Ok(SeatPhase::Running),
        ServiceStatus::DataLoss | ServiceStatus::Decommissioned => {
            Err(MgCause::TerminalStatus(status.clone()))
        }
    }
}

fn default_federation_name(fa_id: FaId, created_at: u64) -> FederationName {
    const ADJECTIVES: [&str; 16] = [
        "Amber", "Bright", "Calm", "Cedar", "Clear", "Copper", "Gentle", "Golden", "Kind",
        "Lively", "Quiet", "Silver", "Solar", "Steady", "Warm", "Wild",
    ];
    const NOUNS: [&str; 16] = [
        "Badger", "Beacon", "Bison", "Canyon", "Falcon", "Forest", "Harbor", "Juniper", "Meadow",
        "Orchid", "Otter", "Raven", "River", "Summit", "Willow", "Wren",
    ];
    let key = fa_id.0.serialize();
    let adjective = ADJECTIVES[usize::from(key[0] ^ (created_at as u8)) % ADJECTIVES.len()];
    let noun = NOUNS[usize::from(key[31] ^ ((created_at >> 8) as u8)) % NOUNS.len()];
    FederationName(format!("{adjective} {noun}"))
}

pub(crate) fn ensure_time_remaining(deadline: Instant, operation: &'static str) -> FaResult<()> {
    if Instant::now() >= deadline {
        return Err(FaError::Timeout(operation.to_owned()));
    }
    Ok(())
}

fn ensure_effective_time_remaining(deadline: Instant, operation: &'static str) -> FaResult<()> {
    select_timer_duration(
        MIN_RUNTIME_TIMER_DURATION,
        deadline.saturating_duration_since(Instant::now()),
        operation,
    )
    .map(drop)
}

pub(crate) async fn sleep_for_retry(deadline: Instant, interval: Duration) -> FaResult<()> {
    let operation = "waiting to retry Manifold Guardian";
    let duration = select_timer_duration(
        interval,
        deadline.saturating_duration_since(Instant::now()),
        operation,
    )?;
    // Pass the selected relative duration directly. Reconstructing it from an
    // absolute deadline can lose the WASM timer's final millisecond.
    sleep(duration).await;
    ensure_time_remaining(deadline, "waiting to retry Manifold Guardian")
}

async fn retry_quote_attempt(deadline: Instant, delay: &mut Duration) -> Result<(), ()> {
    if Instant::now() >= deadline || sleep_for_retry(deadline, *delay).await.is_err() {
        return Err(());
    }
    *delay = (*delay)
        .saturating_mul(2)
        .min(SELECTED_MG_CONNECT_RETRY_MAX_DELAY);
    Ok(())
}

fn select_timer_duration(
    configured: Duration,
    run_remaining: Duration,
    operation: &'static str,
) -> FaResult<Duration> {
    if run_remaining < MIN_RUNTIME_TIMER_DURATION {
        return Err(FaError::Timeout(operation.to_owned()));
    }
    Ok(configured.min(run_remaining))
}

/// Extract the `fedi:fman_seat_bindings` string from a raw meta object.
///
/// The `meta` module holds one JSON object of fields under its default key,
/// and the directory is one string field within it. `Ok(None)` means the
/// federation carries no directory yet, which is the normal state while
/// consensus is still forming.
fn seat_bindings_field(meta_value: &Option<Vec<u8>>) -> FaResult<Option<String>> {
    let Some(fields) = parse_metadata_object(meta_value.as_deref()).map_err(|error| {
        FaError::InvalidManifoldGuardians(format!(
            "consensus metadata is not a JSON object: {error}"
        ))
    })?
    else {
        return Ok(None);
    };

    match fields.get(MG_SEAT_BINDINGS_META_FIELD_KEY) {
        None => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(FaError::InvalidManifoldGuardians(format!(
            "{MG_SEAT_BINDINGS_META_FIELD_KEY} consensus metadata is not a string"
        ))),
    }
}

pub(crate) fn meta_field_matches(
    meta_value: Option<&[u8]>,
    key: &MetaFieldKey,
    expected: &MetaFieldValue,
) -> FaResult<bool> {
    let Some(fields) = parse_metadata_object(meta_value).map_err(|error| {
        FaError::MaintenanceConsensusInvalid {
            reason: format!("metadata is not a JSON object: {error}"),
        }
    })?
    else {
        return Ok(false);
    };
    match fields.get(&key.0) {
        None => Ok(false),
        Some(serde_json::Value::String(value)) => Ok(value == &expected.0),
        Some(_) => Err(FaError::MaintenanceConsensusInvalid {
            reason: format!("{} consensus metadata is not a string", key.0),
        }),
    }
}

fn parse_metadata_object(
    meta_value: Option<&[u8]>,
) -> Result<Option<BTreeMap<String, serde_json::Value>>, serde_json::Error> {
    meta_value.map(serde_json::from_slice).transpose()
}

/// Pair the snapshot's raw metadata bytes with their consensus revision.
///
/// The port contract requires the reader to report both halves of one meta
/// consensus read together, so a metadata base always commits to one
/// occurrence of the board state. An unpaired combination is a broken
/// adapter; the driver refuses to guess which half to trust.
pub(crate) fn snapshot_meta_consensus(
    snapshot: &FederationConsensusSnapshot,
) -> Result<Option<(u64, &[u8])>, String> {
    match (snapshot.meta_value.as_deref(), snapshot.meta_revision) {
        (Some(value), Some(revision)) => Ok(Some((revision, value))),
        (None, None) => Ok(None),
        (Some(_), None) => Err(
            "consensus read returned metadata bytes without their consensus revision".to_owned(),
        ),
        (None, Some(_)) => {
            Err("consensus read returned a consensus revision without metadata bytes".to_owned())
        }
    }
}

pub(crate) struct ConsensusMetadataTooLarge {
    pub(crate) actual_bytes: usize,
    pub(crate) max_bytes: usize,
}

fn formation_metadata_too_large(error: ConsensusMetadataTooLarge) -> FaError {
    FaError::InvalidManifoldGuardians(format!(
        "consensus metadata is {} bytes; formation permits at most {} bytes",
        error.actual_bytes, error.max_bytes
    ))
}

pub(crate) fn validate_consensus_metadata_size(
    value: Option<&[u8]>,
) -> Result<(), ConsensusMetadataTooLarge> {
    let actual_bytes = value.map_or(0, <[u8]>::len);
    if actual_bytes > FEDERATION_METADATA_OBJECT_MAX_BYTES {
        return Err(ConsensusMetadataTooLarge {
            actual_bytes,
            max_bytes: FEDERATION_METADATA_OBJECT_MAX_BYTES,
        });
    }
    Ok(())
}

fn guardian_fee_rate_matches(meta_value: Option<&[u8]>, send_ppm: u64) -> FaResult<bool> {
    validate_consensus_metadata_size(meta_value).map_err(|error| {
        FaError::MaintenanceConsensusTooLarge {
            actual_bytes: error.actual_bytes,
            max_bytes: error.max_bytes,
        }
    })?;
    let Some(fields) = parse_metadata_object(meta_value).map_err(|error| {
        FaError::InvalidManifoldGuardians(format!(
            "consensus metadata is not a JSON object: {error}"
        ))
    })?
    else {
        return Ok(false);
    };
    match fields.get("fedi:guardian_fee_send_ppm") {
        Some(serde_json::Value::String(rate)) => Ok(rate == &send_ppm.to_string()),
        None => Ok(false),
        Some(_) => Err(FaError::InvalidManifoldGuardians(
            "guardian-fee consensus rate metadata field is not a string".to_owned(),
        )),
    }
}

fn guardian_fee_recipients_match(meta_value: Option<&[u8]>, recipients: &str) -> FaResult<bool> {
    validate_consensus_metadata_size(meta_value).map_err(|error| {
        FaError::MaintenanceConsensusTooLarge {
            actual_bytes: error.actual_bytes,
            max_bytes: error.max_bytes,
        }
    })?;
    let Some(fields) = parse_metadata_object(meta_value).map_err(|error| {
        FaError::InvalidManifoldGuardians(format!(
            "consensus metadata is not a JSON object: {error}"
        ))
    })?
    else {
        return Ok(false);
    };
    match fields.get("fedi:guardian_fee_remittance_account") {
        Some(serde_json::Value::String(value)) => Ok(value == recipients),
        None => Ok(false),
        Some(_) => Err(FaError::InvalidManifoldGuardians(
            "guardian-fee consensus recipients metadata field is not a string".to_owned(),
        )),
    }
}

fn mg_error(index: usize, message: impl Into<String>) -> FaError {
    FaError::ManifoldGuardian {
        index: u16::try_from(index).expect("validated formation size fits u16"),
        message: message.into(),
    }
}

pub(crate) fn now_secs() -> FaResult<u64> {
    Ok(fedimint_core::time::duration_since_epoch().as_secs())
}

fn validate_live_admissions(
    recovery: &PurchaseRecovery,
    expected_provenance: StoredVerifierProvenance,
    now: Timestamp,
) -> FaResult<()> {
    for seat in &recovery.seats {
        seat.admission.validate_if_fresh(expected_provenance, now)?;
    }
    Ok(())
}

pub(crate) fn restored_formation_id(fa_id: FaId, invite: &InviteCode) -> FaResult<FormationId> {
    let federation_id = invite_federation_id(invite)?;
    let mut digest = Sha256::new();
    digest.update(b"fedi-fi-restored-formation-id/v1\0");
    digest.update(fa_id.0.serialize());
    digest.update(federation_id.to_string().as_bytes());
    Ok(FormationId(format!(
        "restored-{}",
        hex::encode(digest.finalize())
    )))
}

fn invite_federation_id(invite: &InviteCode) -> FaResult<FedimintFederationId> {
    let parsed = FedimintInviteCode::from_str(&invite.0).map_err(|error| {
        FaError::InvalidManifoldGuardians(format!(
            "Manifold Guardian returned an invalid federation invite: {error}"
        ))
    })?;
    if parsed.api_secret().is_some() {
        return Err(FaError::InvalidManifoldGuardians(
            "Manifold Guardian returned a bearer-secret federation invite".to_owned(),
        ));
    }
    Ok(parsed.federation_id())
}

/// Create the actual monotonic deadline after the pre-effect check.
pub(crate) fn formation_deadline(options: FormationRunOptions) -> FaResult<Instant> {
    checked_deadline(
        Instant::now(),
        options.run_timeout,
        FormationTimingField::RunTimeout,
    )
}

fn checked_deadline(
    now: Instant,
    duration: Duration,
    field: FormationTimingField,
) -> FaResult<Instant> {
    now.checked_add(duration)
        .ok_or(InvalidFormationRunOptions::DeadlineOverflow { field })
        .map_err(FaError::from)
}

/// Establish the absolute monotonic deadline before awaiting lease acquisition.
pub(crate) async fn start_driver_run(
    store: &FaStore,
    options: FormationRunOptions,
) -> FaResult<(Instant, DriverLease)> {
    let deadline = formation_deadline(options)?;
    let lease = store
        .acquire_driver_lease(options.lease_duration(), options.lease_renewal_duration())
        .await?;
    Ok((deadline, lease))
}

pub(crate) fn finish_driver_run<T>(result: FaResult<T>, release: FaResult<()>) -> FaResult<T> {
    match result {
        Err(error) => Err(error),
        Ok(value) => release.map(|()| value),
    }
}
