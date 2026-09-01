//! Crash-safe Fleet Manager formation and payment-reservation driver.

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "dev-pinned-formation"))]
use std::collections::HashSet;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::str::FromStr as _;
use std::time::Duration;

use fedi_decentralized_domain::{
    FMAN_SEAT_BINDINGS_META_FIELD_KEY, FmanSeatBindings, federation_seats,
};
use fedi_decentralized_nostr_clients::FiNostrClient;
use fedi_decentralized_service_fleet_manager::*;
use fedimint_core::config::{ClientConfig, FederationId as FedimintFederationId};
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::runtime::{Instant, sleep, timeout};
use futures::stream::{FuturesUnordered, StreamExt as _};
use stability_pool_common::Account;
use tokio::sync::OnceCell;

use crate::db::{
    ActiveFormationRecovery, AdmissionEffect, DriverLease, FiRecovery, FiStore, FmanAdmission,
    FormationCreationMode, FormationMetaTarget, InitialSeat, QuoteAuthorization,
    StoredVerifierProvenance,
};
use crate::selection::AvailabilityMismatch;
use crate::{
    FederationConsensusReader, FederationConsensusSnapshot, FiClient, FiError, FiIdentity,
    FiPayments, FiResult, FiStatus, FleetManagerConnector, FmanReplacementApproval,
    FmanReplacementPreview, FmanSelectionApproval, FmanSelectionRequest, FormationActionRequired,
    FormationFreshness, FormationId, FormationIntent, FormationPhase, FormationSnapshot,
    GuardianFeePpm, PaymentAuthorizationId, PaymentRequirements, PaymentReservationId,
    PaymentReservationRecovery, ResolvedFormationIntent, SeatPaymentRecovery, SeatPhase,
    SelectionReauthorizationReason,
};

/// Smallest duration represented without truncation by native and WASM runtime timers.
pub(crate) const MIN_RUNTIME_TIMER_DURATION: Duration = Duration::from_millis(1);
/// Largest duration represented without overflow by native and WASM runtime timers.
pub(crate) const MAX_RUNTIME_TIMER_DURATION: Duration = Duration::from_millis(i32::MAX as u64);
const SELECTED_FMAN_CONNECT_RETRY_BUDGET: Duration = Duration::from_secs(2 * 60);
const SELECTED_FMAN_CONNECT_RETRY_MIN_DELAY: Duration = Duration::from_millis(100);
const SELECTED_FMAN_CONNECT_RETRY_MAX_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuoteAttemptPolicy {
    /// The verified row has not crossed its output/acceptance boundary, so a
    /// definite transport failure may be retried and eventually returned as
    /// a fresh-selection requirement.
    ReplaceableSelection,
    /// The row is pinned or already value-bound; failures remain exact
    /// recovery errors and never manufacture replacement authority.
    ExactRecovery,
}

impl QuoteAttemptPolicy {
    fn allows_selection_reauthorization(self) -> bool {
        self == Self::ReplaceableSelection
    }
}

fn quote_attempt_policy(
    creation_mode: &FormationCreationMode,
    admission: &FmanAdmission,
) -> QuoteAttemptPolicy {
    match creation_mode {
        FormationCreationMode::Selected { .. } if admission.requires_effect_authorization() => {
            QuoteAttemptPolicy::ReplaceableSelection
        }
        FormationCreationMode::Pinned | FormationCreationMode::Selected { .. } => {
            QuoteAttemptPolicy::ExactRecovery
        }
    }
}

enum QuoteAttemptError {
    Transport { index: usize, message: String },
    Other(FiError),
}

impl QuoteAttemptError {
    fn into_fi_error(self) -> FiError {
        match self {
            Self::Transport { index, message } => fman_error(index, message),
            Self::Other(error) => error,
        }
    }
}

impl From<FiError> for QuoteAttemptError {
    fn from(error: FiError) -> Self {
        Self::Other(error)
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
    /// Runtime-representable maximum time for one consumer or network capability call.
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
    /// Maximum time for one consumer or network capability call.
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
    fn poll_interval(self) -> Duration {
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
    pub(crate) fn validate_for_start(self, store: &FiStore) -> FiResult<()> {
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
        make_future: impl FnOnce() -> FiResult<Fut>,
    ) -> FiResult<T>
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
            self.options.request_timeout(),
            self.deadline.saturating_duration_since(Instant::now()),
            operation,
        )?;
        timeout(duration, future)
            .await
            .map_err(|_| FiError::Timeout(operation.to_owned()))
    }

    /// Prepare the timeout budget for a value-moving wallet call.
    ///
    /// This refreshes the coarse run guard and captures the call's time budget.
    /// Constructing the future remains effect-free; the wallet body starts only
    /// when `poll_value_call` polls it.
    async fn prepare_value_call_budget(
        &self,
        operation: &'static str,
    ) -> FiResult<ValueCallTimeoutBudget> {
        ensure_effective_time_remaining(self.deadline, operation)?;
        self.lease.renew().await?;
        ensure_effective_time_remaining(self.deadline, operation)?;
        Ok(ValueCallTimeoutBudget {
            operation,
            deadline: self.deadline,
            request_timeout: self.options.request_timeout(),
        })
    }

    /// Atomically record the durable output-start boundary.
    async fn arm_payment_outputs_started(
        &self,
        formation_id: &FormationId,
        verifier_provenance: StoredVerifierProvenance,
    ) -> FiResult<()> {
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
    ) -> FiResult<()> {
        ensure_effective_time_remaining(self.deadline, "authorizing selected-seat effect wave")?;
        self.lease
            .authorize_seat_effects(formation_id, effects, verifier_provenance)
            .await
    }

    /// Construct a synchronous request under the current run guard.
    pub(crate) async fn construct<T>(
        &self,
        operation: &'static str,
        construct: impl FnOnce() -> FiResult<T>,
    ) -> FiResult<T> {
        ensure_effective_time_remaining(self.deadline, operation)?;
        self.lease.renew().await?;
        ensure_effective_time_remaining(self.deadline, operation)?;
        construct()
    }
}

/// Absolute run deadline and request cap prepared for one wallet-output poll.
struct ValueCallTimeoutBudget {
    operation: &'static str,
    deadline: Instant,
    request_timeout: Duration,
}

impl ValueCallTimeoutBudget {
    async fn poll_value_call<T>(self, future: impl Future<Output = T>) -> FiResult<T> {
        // Preflight can precede the atomic output-start boundary and other
        // per-seat preparation. Recompute the relative timer only when this
        // wallet future is about to be polled, so that intervening work cannot
        // extend the absolute formation run deadline.
        ensure_effective_time_remaining(self.deadline, self.operation)?;
        let duration = select_timer_duration(
            self.request_timeout,
            self.deadline.saturating_duration_since(Instant::now()),
            self.operation,
        )?;
        timeout(duration, future)
            .await
            .map_err(|_| FiError::Timeout(self.operation.to_owned()))
    }
}

pub(crate) struct SeatSession<C> {
    pub(crate) index: u16,
    pub(crate) client: C,
    pub(crate) seat_id: SeatId,
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
    Driver(FiError),
    /// Typed refusal returned by the FMan protocol.
    FleetManager(FleetManagerError),
}

impl MetaFieldSubmissionError {
    fn into_formation_error(self, index: u16) -> FiError {
        match self {
            Self::Driver(error) => error,
            Self::FleetManager(error) => FiError::FleetManager {
                index,
                message: error.to_string(),
            },
        }
    }
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
    reason: RefusalReason,
    release_proof: Option<T>,
}

enum TerminalSeatOutcome {
    PaymentRejected { position: usize },
    SeatRefused { index: u16, reason: String },
}

impl TerminalSeatOutcome {
    fn into_error(self) -> FiError {
        match self {
            Self::PaymentRejected { position } => FiError::Payment(format!(
                "payment for Fleet Manager {} was rejected; its quote was cleared",
                position + 1
            )),
            Self::SeatRefused { index, reason } => FiError::SeatRefused { index, reason },
        }
    }

    /// Classify a proven terminal outcome against the formation's value
    /// boundary. A selected all-free formation has consumed its exact
    /// quote-bound admission but has not armed wallet outputs; re-quoting that
    /// same row would not carry authority for a second presentation. Return
    /// the typed fresh-selection transition so the outer driver abandons to
    /// `Idle`. Pinned or post-output refusals retain their exact recovery
    /// behavior.
    fn into_formation_error(
        self,
        creation_mode: &FormationCreationMode,
        payment_outputs_started: bool,
    ) -> FiError {
        if creation_mode.is_selected()
            && !payment_outputs_started
            && matches!(&self, Self::SeatRefused { .. })
        {
            return FiError::SelectionReauthorizationRequired(
                SelectionReauthorizationReason::SelectedFmanUnavailable,
            );
        }
        self.into_error()
    }

    fn requires_pre_output_reauthorization(
        &self,
        creation_mode: &FormationCreationMode,
        payment_outputs_started: bool,
    ) -> bool {
        creation_mode.is_selected()
            && !payment_outputs_started
            && matches!(self, Self::SeatRefused { .. })
    }
}

fn apply_authorized_effect_in_memory(
    recovery: &mut ActiveFormationRecovery,
    position: usize,
    quote_id: QuoteId,
    effect: AdmissionEffect,
) -> FiResult<()> {
    let seat = recovery.seats.get_mut(position).ok_or_else(|| {
        FiError::Storage(format!(
            "authorized effect names missing FI seat row {position}"
        ))
    })?;
    seat.admission.mark_effect_authorized(quote_id, effect)?;
    if seat.replacement_for.is_some() {
        if !seat.replacement_approved {
            return Err(FiError::Storage(format!(
                "authorized replacement FI seat row {position} lacks its approval"
            )));
        }
        // Mirror the atomic DB transition. Once this exact admission/effect
        // wave commits, the row is pinned to recovery and is no longer a
        // provisional replacement that outer error handling may restore.
        seat.replacement_for = None;
        seat.replacement_previous_locator = None;
        seat.replacement_previous_fman_id = None;
        seat.replacement_approved = false;
    }
    Ok(())
}

impl<I, P, N, F, C> FiClient<I, P, N, F, C>
where
    I: FiIdentity,
    P: FiPayments,
    N: FiNostrClient,
    F: FleetManagerConnector,
    C: FederationConsensusReader,
{
    /// Form a federation through explicitly pinned FMan locators.
    ///
    /// The returned future owns no background task. Dropping it cancels
    /// in-flight work, while every completed durable checkpoint remains
    /// resumable through [`FiClient::resume`]. If paid quotes are selected,
    /// this method returns `Ok(())` at
    /// [`FormationPhase::AwaitingPaymentReadiness`] without spending; the
    /// consumer must inspect the aggregate requirements and explicitly call
    /// [`FiClient::authorize_payments`].
    #[cfg(any(test, feature = "dev-pinned-formation"))]
    pub async fn create_with_pinned_fmans(
        &self,
        intent: FormationIntent,
        locators: Vec<Locator>,
        options: FormationRunOptions,
    ) -> FiResult<()> {
        Self::preflight_create_with_pinned_fmans(&intent, &locators)?;
        let fedimintd_dkg_version = pinned_dkg_version(&intent)?;
        let seats = locators
            .into_iter()
            .enumerate()
            .map(|(index, locator)| InitialSeat::new(index, locator, FmanAdmission::Pinned))
            .collect();
        self.create_with_seats_and_callback(
            intent,
            seats,
            FormationCreationMode::Pinned,
            fedimintd_dkg_version,
            None,
            options,
        )
        .await
    }

    /// Form through pinned FMan locators and durably attach one installation
    /// callback to every guardian's DKG attempt.
    ///
    /// This has the same cancellation, payment-readiness, and explicit payment
    /// authorization behavior as [`Self::create_with_pinned_fmans`]. FI
    /// persists the callback before quotes, payments, seat creation, or DKG,
    /// retains it through every pre-`Formed` recovery, and sends the same value
    /// to every guardian. Public formation
    /// snapshots never expose the bearer. FI clears its copy atomically with the
    /// `Formed` checkpoint after every FMan has accepted durable retry ownership.
    #[cfg(any(test, feature = "dev-pinned-formation"))]
    pub async fn create_with_pinned_fmans_and_callback(
        &self,
        intent: FormationIntent,
        locators: Vec<Locator>,
        completion_callback: DkgCompletionCallback,
        options: FormationRunOptions,
    ) -> FiResult<()> {
        Self::preflight_create_with_pinned_fmans(&intent, &locators)?;
        let fedimintd_dkg_version = pinned_dkg_version(&intent)?;
        let seats = locators
            .into_iter()
            .enumerate()
            .map(|(index, locator)| InitialSeat::new(index, locator, FmanAdmission::Pinned))
            .collect();
        self.create_with_seats_and_callback(
            intent,
            seats,
            FormationCreationMode::Pinned,
            fedimintd_dkg_version,
            Some(completion_callback),
            options,
        )
        .await
    }

    /// Execute the Pay-and-create action for one fresh, verified selection.
    ///
    /// The approval comes only from [`crate::FmanSelectionPreview::approve`] and is
    /// valid for two minutes. No quote is requested during preview: this
    /// method first validates the sealed set and explicit ready payer, then
    /// obtains exact quotes and proceeds automatically only while their total
    /// remains within the approved limit. Any stale preview, unavailable
    /// selected FMan, unavailable payer, or changed/over-limit quote returns a
    /// typed reauthorization error before wallet output generation. A
    /// deployment bootstrapping exclusively from zero-price seats uses
    /// [`Self::create_without_payer`] instead.
    pub async fn pay_and_create(
        &self,
        intent: FormationIntent,
        approval: FmanSelectionApproval,
        payment_federation_id: FederationId,
        options: FormationRunOptions,
    ) -> FiResult<()> {
        self.create_from_selection(intent, approval, Some(payment_federation_id), None, options)
            .await
    }

    /// Execute Pay-and-create while atomically binding one installation-scoped
    /// DKG completion callback before quotes, payments, or remote seat work.
    ///
    /// FI persists the callback before any remote work, retains it through
    /// every pre-`Formed` recovery, sends the same value to every guardian,
    /// never exposes the bearer in public formation snapshots, and clears its
    /// copy atomically with the `Formed` checkpoint once every FMan has accepted
    /// durable retry ownership.
    pub async fn pay_and_create_with_callback(
        &self,
        intent: FormationIntent,
        approval: FmanSelectionApproval,
        payment_federation_id: FederationId,
        completion_callback: DkgCompletionCallback,
        options: FormationRunOptions,
    ) -> FiResult<()> {
        self.create_from_selection(
            intent,
            approval,
            Some(payment_federation_id),
            Some(completion_callback),
            options,
        )
        .await
    }

    /// Execute selected formation without a setup-payment federation.
    ///
    /// This is the deployment-bootstrap counterpart to [`Self::pay_and_create`]:
    /// it consumes the same fresh, fully PeerBadge-verified approval, but
    /// requests quotes without consulting setup-payment policy or the wallet.
    /// Formation proceeds only if every selected Fleet Manager's live offer
    /// and exact signed quote are free. A priced live offer returns
    /// [`SelectionReauthorizationReason::PaymentFederationRequired`] before a
    /// quote, wallet call, seat presentation, or durable output boundary; the
    /// consumer may retry through [`Self::pay_and_create`] after choosing an
    /// authenticated Ready payer and approving a positive paid envelope.
    pub async fn create_without_payer(
        &self,
        intent: FormationIntent,
        approval: FmanSelectionApproval,
        options: FormationRunOptions,
    ) -> FiResult<()> {
        self.create_from_selection(intent, approval, None, None, options)
            .await
    }

    async fn create_from_selection(
        &self,
        intent: FormationIntent,
        approval: FmanSelectionApproval,
        payment_federation_id: Option<FederationId>,
        completion_callback: Option<DkgCompletionCallback>,
        options: FormationRunOptions,
    ) -> FiResult<()> {
        let max_total_msats = approval.max_total_msats;
        let expected_size = usize::from(intent.federation_size().0);
        if approval.seats.len() != expected_size {
            return Err(FiError::InvalidFleetManagers(format!(
                "selection contains {} seats but intent requires {expected_size}",
                approval.seats.len()
            )));
        }
        if approval.request.federation_size() != intent.federation_size()
            || intent.fedimintd_versions() != approval.request.fedimintd_versions()
            || approval.request.plan() != intent.plan()
        {
            return Err(FiError::InvalidIntent(
                "selection approval belongs to a different formation request".to_owned(),
            ));
        }
        if approval.verifier_provenance != self.inner.peer_badge_verifier.provenance() {
            return Err(FiError::InvalidIntent(
                "selection approval belongs to a different verifier environment".to_owned(),
            ));
        }
        let approval_valid_until = approval.valid_until;
        let fedimintd_dkg_version = approval.fedimintd_dkg_version.clone();
        let verifier_provenance = approval.verifier_provenance.into();
        let approved_seats = approval.into_seats_at(Timestamp(now_secs()?))?;
        let seats = approved_seats
            .into_iter()
            .enumerate()
            .map(|(index, seat)| {
                InitialSeat::new(
                    index,
                    seat.locator,
                    FmanAdmission::fresh_peer_badge(
                        seat.fman_id,
                        verifier_provenance,
                        approval_valid_until,
                    ),
                )
            })
            .collect();
        let intent = if payment_federation_id.is_some() {
            intent.with_max_total_msats(max_total_msats)?
        } else {
            // No payment can be requested or authorized in this mode, so a
            // spending cap is neither needed nor persisted. In particular,
            // this keeps the natural zero-price approval representable without
            // weakening FormationIntent's public nonzero paid-cap invariant.
            intent
        };
        self.create_with_seats_and_callback(
            intent,
            seats,
            FormationCreationMode::Selected {
                payment_federation_id,
            },
            fedimintd_dkg_version,
            completion_callback,
            options,
        )
        .await
    }

    /// Fetch a fresh verified guardian subset for rows whose exact payment
    /// was proven safe to replace after outputs started.
    pub async fn preview_fman_replacements(
        &self,
        options: crate::FmanDiscoveryOptions,
    ) -> FiResult<FmanReplacementPreview> {
        let fi_id = self.fi_id()?;
        let recovery = self.active_recovery(fi_id).await?;
        let requirements =
            crate::db::replacement_requirements(&recovery.snapshot.formation_id, &recovery.seats)
                .ok_or_else(|| {
                FiError::InvalidIntent("no guardian replacement is required".to_owned())
            })?;
        let request = FmanSelectionRequest::new(
            recovery.snapshot.intent.federation_size,
            recovery.snapshot.intent.fedimintd_versions.clone(),
            recovery.snapshot.intent.plan,
        )?;
        let excluded = recovery
            .seats
            .iter()
            .map(|seat| {
                seat.admission.fman_id().ok_or_else(|| {
                    FiError::Storage(
                        "selected formation contains a pinned FMan admission".to_owned(),
                    )
                })
            })
            .collect::<FiResult<BTreeSet<_>>>()?;
        let replacement_indices = requirements
            .seats
            .iter()
            .map(|seat| seat.index)
            .collect::<BTreeSet<_>>();
        let mut retained_service_pubkeys = BTreeMap::new();
        for seat in &recovery.seats {
            if replacement_indices.contains(&seat.progress.index) {
                continue;
            }
            let fman_id = seat.admission.fman_id().ok_or_else(|| {
                FiError::Storage("selected formation contains a pinned FMan admission".to_owned())
            })?;
            if retained_service_pubkeys
                .insert(seat.progress.locator.service_pubkey, fman_id)
                .is_some()
            {
                return Err(FiError::Storage(
                    "selected formation contains duplicate retained service signing keys"
                        .to_owned(),
                ));
            }
        }
        let now = fedimint_core::time::duration_since_epoch().as_secs();
        let deadline = Instant::now()
            .checked_add(options.timeout())
            .expect("clamped discovery timeout fits the monotonic deadline domain");
        crate::selection::preview_fman_replacements_with(
            &self.inner.ports.registry,
            &self.inner.peer_badge_verifier,
            &crate::selection::LiveAvailabilityProber {
                connector: Some(&self.inner.ports.fman_connector),
            },
            self.inner.peer_badge_verifier.provenance(),
            &request,
            &recovery.snapshot.intent.fedimintd_dkg_version,
            requirements,
            excluded,
            retained_service_pubkeys,
            deadline,
            now,
            || fedimint_core::time::duration_since_epoch().as_secs(),
        )
        .await
    }

    /// Atomically apply one renewed, verified replacement approval and
    /// continue through exact replacement quoting and automatic aggregate
    /// authorization only while the renewed sealed cap covers the total.
    pub async fn apply_fman_replacements(
        &self,
        approval: FmanReplacementApproval,
        options: FormationRunOptions,
    ) -> FiResult<()> {
        let _guard = self.inner.run_guard.try_lock().map_err(|_| FiError::Busy)?;
        options.validate_for_start(&self.inner.store)?;
        let fi_id = self.fi_id()?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = async {
            let recovery = self.active_recovery(fi_id).await?;
            let current = crate::db::replacement_requirements(
                &recovery.snapshot.formation_id,
                &recovery.seats,
            )
            .ok_or_else(|| {
                FiError::InvalidIntent("no guardian replacement is required".to_owned())
            })?;
            if current != approval.requirements {
                return Err(FiError::InvalidIntent(
                    "guardian replacement requirements changed after preview".to_owned(),
                ));
            }
            if approval.verifier_provenance != self.inner.peer_badge_verifier.provenance() {
                return Err(FiError::SelectionReauthorizationRequired(
                    SelectionReauthorizationReason::VerifierEnvironmentChanged,
                ));
            }
            let valid_until = approval.valid_until;
            let verifier_provenance = approval.verifier_provenance.into();
            let max_total_msats = approval.max_total_msats;
            let requirements = approval.requirements.clone();
            let approved_seats = approval.into_seats_at(Timestamp(now_secs()?))?;
            let mut fman_ids = recovery
                .seats
                .iter()
                .filter_map(|seat| seat.admission.fman_id())
                .collect::<BTreeSet<_>>();
            if approved_seats
                .iter()
                .any(|seat| !fman_ids.insert(seat.fman_id))
            {
                return Err(FiError::InvalidFleetManagers(
                    "replacement guardian duplicates an existing or replacement FMan".to_owned(),
                ));
            }
            let replacement_indices = requirements
                .seats
                .iter()
                .map(|seat| seat.index)
                .collect::<BTreeSet<_>>();
            let mut service_pubkeys = BTreeMap::new();
            for seat in &recovery.seats {
                if replacement_indices.contains(&seat.progress.index) {
                    continue;
                }
                let fman_id = seat.admission.fman_id().ok_or_else(|| {
                    FiError::Storage(
                        "selected formation contains a pinned FMan admission".to_owned(),
                    )
                })?;
                if service_pubkeys
                    .insert(seat.progress.locator.service_pubkey, fman_id)
                    .is_some()
                {
                    return Err(FiError::Storage(
                        "selected formation contains duplicate retained service signing keys"
                            .to_owned(),
                    ));
                }
            }
            for seat in &approved_seats {
                if service_pubkeys
                    .insert(seat.locator.service_pubkey, seat.fman_id)
                    .is_some()
                {
                    return Err(FiError::InvalidFleetManagers(
                        "replacement guardian duplicates a retained or replacement service \
                         signing key"
                            .to_owned(),
                    ));
                }
            }
            let replacements = approved_seats
                .into_iter()
                .map(|seat| {
                    (
                        seat.locator,
                        FmanAdmission::fresh_peer_badge(
                            seat.fman_id,
                            verifier_provenance,
                            valid_until,
                        ),
                    )
                })
                .collect::<Vec<_>>();
            self.inner
                .store
                .replace_guardians(
                    &recovery.snapshot.formation_id,
                    &requirements,
                    &replacements,
                    max_total_msats,
                )
                .await?;
            let recovery = self.active_recovery(fi_id).await?;
            self.publish_snapshot(recovery.snapshot.clone());
            self.drive_pinned(recovery, fi_id, run).await
        }
        .await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    async fn create_with_seats_and_callback(
        &self,
        intent: FormationIntent,
        seats: Vec<InitialSeat>,
        creation_mode: FormationCreationMode,
        fedimintd_dkg_version: crate::FedimintdDkgVersion,
        completion_callback: Option<DkgCompletionCallback>,
        options: FormationRunOptions,
    ) -> FiResult<()> {
        let _run = self.inner.run_guard.try_lock().map_err(|_| FiError::Busy)?;
        options.validate_for_start(&self.inner.store)?;
        let fi_id = self.fi_id()?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = async {
            if !matches!(self.inner.store.load_status(fi_id).await?, FiStatus::Idle) {
                return Err(FiError::InvalidIntent(
                    "an active formation already exists; resume it instead".to_owned(),
                ));
            }
            if let Some(requested) = creation_mode.selected_payment_federation() {
                // Pay-and-create must not obtain even its first exact quote
                // until the explicitly selected payer is authenticated and
                // currently Ready. Exact balance plus fee sufficiency is
                // checked once the complete verified aggregate exists.
                self.require_setup_payment_federation(requested, run)
                    .await?;
            }
            let created_at = now_secs()?;
            let formation_id = FormationId(format!("{}-{created_at}", fi_id.0));
            let default_name = default_federation_name(fi_id, created_at);
            let intent = intent.resolve_for_dkg(default_name, fedimintd_dkg_version)?;
            self.inner
                .store
                .initialize(
                    fi_id,
                    formation_id,
                    intent,
                    seats,
                    creation_mode,
                    completion_callback,
                )
                .await?;
            let recovery = self.active_recovery(fi_id).await?;
            self.publish_snapshot(recovery.snapshot.clone());
            self.drive_pinned(recovery, fi_id, run).await
        }
        .await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    /// Validate pinned locator inputs without accessing identity, storage,
    /// wallets, or the network.
    #[cfg(any(test, feature = "dev-pinned-formation"))]
    pub fn preflight_create_with_pinned_fmans(
        intent: &FormationIntent,
        locators: &[Locator],
    ) -> FiResult<()> {
        validate_locators(intent, locators)?;
        pinned_dkg_version(intent).map(|_| ())
    }

    /// Explicitly authorize the paid terms currently exposed by the aggregate
    /// action and continue formation.
    ///
    /// Authorization is durably recorded before the first wallet call and the
    /// exact displayed quote aggregate is then reserved in the payer wallet.
    /// Dropping this future after either checkpoint is safe:
    /// [`FiClient::resume`] reconstructs the same reservation and recovers any
    /// wallet operation that already started.
    ///
    /// The initial selected Pay-and-create aggregate cannot use this entry
    /// point: its sealed cap is the only authorization. A post-output guardian
    /// replacement may expose a fresh exact aggregate here when its real quote
    /// total exceeds the renewed replacement cap.
    pub async fn authorize_payments(
        &self,
        authorization_id: PaymentAuthorizationId,
        options: FormationRunOptions,
    ) -> FiResult<()> {
        let _run = self.inner.run_guard.try_lock().map_err(|_| FiError::Busy)?;
        options.validate_for_start(&self.inner.store)?;
        let fi_id = self.fi_id()?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = async {
            let recovery = self.active_recovery(fi_id).await?;
            if recovery.creation_mode.is_selected() && !recovery.payment_outputs_started {
                return Err(FiError::InvalidIntent(
                    "selected Pay-and-create uses only its sealed cap authorization".to_owned(),
                ));
            }
            let FormationActionRequired::AuthorizePayments(requirements) =
                recovery.snapshot.action_required.clone().ok_or_else(|| {
                    FiError::InvalidIntent(
                        "formation is not awaiting payment authorization".to_owned(),
                    )
                })?
            else {
                return Err(FiError::InvalidIntent(
                    "formation is awaiting guardian replacement, not payment authorization"
                        .to_owned(),
                ));
            };
            if requirements.authorization_id != authorization_id {
                return Err(FiError::InvalidIntent(
                    "payment requirements changed after they were displayed; review the new set"
                        .to_owned(),
                ));
            }
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
                    &recovery.snapshot.formation_id,
                    &authorization_id,
                    &authorizations,
                )
                .await?;
            let recovery = self.active_recovery(fi_id).await?;
            self.publish_snapshot(recovery.snapshot.clone());
            self.drive_pinned(recovery, fi_id, run).await
        }
        .await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    /// Abandon the active formation while it is still value-safe.
    ///
    /// Abandoning is allowed until wallet output generation is durably armed
    /// and before the federation is `Formed`. Commercial quote authorization
    /// alone does not close this window. Zero-price seats a Fleet Manager
    /// already accepted server-side are **forfeited**, not released: abandon
    /// wipes the FI's durable formation state back to [`FiStatus::Idle`]
    /// without contacting any FMan. Outside the window this returns the typed
    /// [`FiError::AbandonUnavailable`]; abandoning after output generation is
    /// armed is deferred until a refund-safe teardown exists.
    ///
    /// The wipe runs under the driver run-guard and lease, so it cannot race
    /// a concurrent driver. The authenticated setup-payment policy retention
    /// is deliberately preserved; it is deployment policy, not formation
    /// state.
    pub async fn abandon_formation(&self, options: FormationRunOptions) -> FiResult<()> {
        let _run = self.inner.run_guard.try_lock().map_err(|_| FiError::Busy)?;
        options.validate_for_start(&self.inner.store)?;
        let fi_id = self.fi_id()?;
        // The same bounded run fences reconstruction and explicit release of
        // any exact pre-output wallet reservation before the local wipe.
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let result = async {
            let mut recovery = self.active_recovery(fi_id).await?;
            self.release_and_abandon_pre_output(
                &mut recovery,
                fi_id,
                ReservationCleanup::ReconstructIfAuthorized,
                run,
            )
            .await
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
    /// The resolved FI account must be a single-signature `BtcDepositor`
    /// account and the rate must be within the pinned payer's inclusive
    /// `0..=210_000`-ppm ceiling and at or above the published minimum
    /// carried by the admitted setup-payment publication (1,500 ppm when none
    /// is admitted).
    /// Both bounds are refused here, before any guardian is contacted, so a
    /// rate every FMan would vote down reports its own reason instead of
    /// retrying to a timeout. FMan remains
    /// the authoritative split validator and may reject a proposal before
    /// casting its guardian vote. Transport, stale-base, or consensus-read
    /// failures may yield a timeout; cancellation stores no partial policy
    /// record, and retrying the same call safely rereads, rebases, and confirms
    /// the live consensus value.
    pub async fn propose_guardian_fees(
        &self,
        send_ppm: GuardianFeePpm,
        options: FormationRunOptions,
    ) -> FiResult<()> {
        let _run = self.inner.run_guard.try_lock().map_err(|_| FiError::Busy)?;
        options.validate_for_start(&self.inner.store)?;
        if send_ppm.value() > crate::MAX_GUARDIAN_FEE_PPM {
            return Err(FiError::InvalidIntent(format!(
                "guardian fee ppm exceeds the payer ceiling of {}",
                crate::MAX_GUARDIAN_FEE_PPM
            )));
        }
        let min_send_ppm = self.min_guardian_fee_ppm().await;
        if u64::from(send_ppm.value()) < min_send_ppm {
            return Err(FiError::InvalidIntent(format!(
                "guardian fee ppm is below the published minimum of {min_send_ppm}"
            )));
        }
        let fi_id = self.fi_id()?;
        let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
        let run = DriverRun::new(options, deadline, &lease);
        let key = MetaFieldKey(
            fedi_decentralized_service_fleet_manager::GUARDIAN_FEE_SEND_PPM_META_FIELD_KEY
                .to_owned(),
        );
        let value = MetaFieldValue(send_ppm.value().to_string());
        let result = self.update_meta_field_pinned(key, value, fi_id, run).await;
        finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    pub(crate) async fn resume_pinned(
        &self,
        recovery: ActiveFormationRecovery,
        options: FormationRunOptions,
        deadline: Instant,
        lease: &DriverLease,
    ) -> FiResult<()> {
        let fi_id = self.fi_id()?;
        let run = DriverRun::new(options, deadline, lease);
        self.drive_pinned(recovery, fi_id, run).await
    }

    async fn drive_pinned(
        &self,
        mut recovery: ActiveFormationRecovery,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<()> {
        let formation_id = recovery.snapshot.formation_id.clone();
        let result = async {
            let validated_fallback = recovery.snapshot.clone();
            let result = self.drive_pinned_inner(&mut recovery, fi_id, run).await;
            if matches!(&result, Err(FiError::SelectionReauthorizationRequired(_)))
                && recovery.creation_mode.is_selected()
                && recovery.payment_outputs_started
                && recovery.seats.iter().any(|seat| seat.replacement_approved)
            {
                // The replacement approval is still value-free: its exact effect
                // was never authorized and no wallet reservation may be erased by
                // this transition. Return the whole provisional wave to a fresh
                // preview boundary under the same driver lease.
                self.restore_provisional_replacements_after_reauthorization(
                    &mut recovery,
                    fi_id,
                    run,
                )
                .await?;
            }
            if matches!(&result, Err(FiError::SelectionReauthorizationRequired(_)))
                && recovery.creation_mode.is_selected()
                && !recovery.payment_outputs_started
            {
                // A reserve-port error is the one reauthorization outcome that
                // proves this invocation obtained no capability. Other outcomes
                // may follow a wallet commit whose FI checkpoint was interrupted,
                // so they must reconstruct the deterministic reservation before
                // deleting the formation.
                let cleanup = if matches!(
                    &result,
                    Err(FiError::SelectionReauthorizationRequired(
                        SelectionReauthorizationReason::SelectedPayerInsufficientFunds
                    ))
                ) && recovery.payment_reservation_id.is_none()
                {
                    ReservationCleanup::DefinitivelyAbsent
                } else {
                    ReservationCleanup::ReconstructIfAuthorized
                };
                match self
                    .release_and_abandon_pre_output(&mut recovery, fi_id, cleanup, run)
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
                    .active_recovery(fi_id)
                    .await
                    .map(|latest| latest.snapshot)
                    .unwrap_or(validated_fallback);
                snapshot.last_error = Some(error.code());
                self.publish_snapshot(snapshot);
            }
            result
        }
        .await;
        if let Err(error) = &result {
            tracing::warn!(
                formation_id = %formation_id.0,
                phase = ?recovery.snapshot.phase,
                %error,
                "formation drive failed"
            );
        }
        result
    }

    /// Release any reconstructable exact wallet hold before deleting FI state.
    ///
    /// Both explicit abandon and automatic selected-flow reauthorization use
    /// this transition. If reconstruction or release is ambiguous, the
    /// formation is retained so a later invocation can retry; no path may drop
    /// the durable reservation id as a substitute for wallet release.
    async fn release_and_abandon_pre_output(
        &self,
        recovery: &mut ActiveFormationRecovery,
        fi_id: FiId,
        cleanup: ReservationCleanup,
        run: DriverRun<'_>,
    ) -> FiResult<()> {
        if recovery.payment_outputs_started {
            return Err(FiError::AbandonUnavailable(
                crate::AbandonUnavailableReason::PaymentOutputsStarted,
            ));
        }
        if recovery.snapshot.phase == FormationPhase::Formed {
            return Err(FiError::AbandonUnavailable(
                crate::AbandonUnavailableReason::AlreadyFormed,
            ));
        }

        if cleanup == ReservationCleanup::DefinitivelyAbsent
            && recovery.payment_reservation_id.is_some()
        {
            return Err(FiError::Storage(
                "wallet reservation cannot be both durable and definitively absent".to_owned(),
            ));
        }
        let requirements = recovery.payment_requirements(fi_id)?;
        match requirements {
            Some(requirements)
                if cleanup == ReservationCleanup::ReconstructIfAuthorized
                    && recovery.payments_authorized(&requirements) =>
            {
                // Reserve may have succeeded immediately before its FI DB
                // checkpoint returned ambiguously. Probe the same id without
                // creating a journal; this remains valid after selection
                // freshness or verifier provenance changes.
                let (reservation_id, recovered) = self
                    .recover_payment_reservation(recovery, &requirements, fi_id, run)
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
                                    &recovery.snapshot.formation_id,
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
                        .map_err(|error| FiError::Payment(error.to_string()))?;
                    }
                    PaymentReservationRecovery::Absent => {
                        if recovery.payment_reservation_id.as_ref() == Some(&reservation_id)
                            && !recovery.payment_reservation_release_intended
                        {
                            return Err(FiError::Storage(
                                "durable FI reservation is absent from the payment wallet"
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
                return Err(FiError::Storage(
                    "cannot wipe FI state while its wallet reservation cannot be reconstructed"
                        .to_owned(),
                ));
            }
            _ => {}
        }

        run.lease
            .abandon_formation(&recovery.snapshot.formation_id)
            .await?;
        self.inner.progress.send_replace(FiStatus::Idle);
        Ok(())
    }

    /// Release an exact unstarted replacement hold, then atomically return
    /// its still-unconsumed approval wave to the fresh-preview boundary.
    async fn restore_provisional_replacements_after_reauthorization(
        &self,
        recovery: &mut ActiveFormationRecovery,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<()> {
        let released_reservation = if let Some(reservation_id) =
            recovery.payment_reservation_id.clone()
        {
            let requirements = recovery
                .reserved_payment_requirements(fi_id)?
                .ok_or_else(|| {
                    FiError::Storage(
                        "replacement reservation has no exact payment requirements".to_owned(),
                    )
                })?;
            if !recovery.payments_authorized(&requirements)
                || crate::db::payment_reservation_id(&recovery.snapshot.formation_id, &requirements)
                    != reservation_id
            {
                return Err(FiError::Storage(
                    "replacement reservation no longer matches its exact authorization".to_owned(),
                ));
            }
            let reservation = self
                .reserve_payment_requirements(recovery, &requirements, fi_id, run)
                .await?;
            // Commit to the release before the wallet call so a crash between
            // the release and the atomic restore below makes wallet absence
            // an expected outcome on the next run instead of retaining the
            // formation forever.
            self.inner
                .store
                .record_reservation_release_intent(&recovery.snapshot.formation_id, &reservation_id)
                .await?;
            recovery.payment_reservation_release_intended = true;
            run.call("releasing unstarted replacement reservation", || {
                Ok(self
                    .inner
                    .ports
                    .payments
                    .release_payment_reservation(reservation.clone()))
            })
            .await?
            .map_err(|error| FiError::Payment(error.to_string()))?;
            Some(ReleasedReplacementReservation::after_wallet_release(
                reservation_id,
            ))
        } else {
            None
        };
        run.lease
            .restore_provisional_replacements(
                &recovery.snapshot.formation_id,
                released_reservation.as_ref(),
            )
            .await
    }

    async fn drive_pinned_inner(
        &self,
        recovery: &mut ActiveFormationRecovery,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<()> {
        if recovery.snapshot.phase == FormationPhase::Formed {
            return self.reconcile_formed(recovery, fi_id, run).await;
        }

        // A wallet commit may have succeeded immediately before the FI-side
        // reservation checkpoint was interrupted. Recover that deterministic
        // aggregate before consulting expiring admission policy: an expired
        // preview or changed verifier provenance must not make an existing
        // wallet journal unreachable.
        let recovery_requirements = if recovery.payment_reservation_id.is_some() {
            recovery.reserved_payment_requirements(fi_id)?
        } else {
            recovery.payment_requirements(fi_id)?
        };
        if let Some(requirements) = recovery_requirements.as_ref()
            && recovery.payments_authorized(requirements)
        {
            let (reservation_id, recovered) = self
                .recover_payment_reservation(recovery, requirements, fi_id, run)
                .await?;
            if recovery
                .payment_reservation_id
                .as_ref()
                .is_some_and(|stored| stored != &reservation_id)
            {
                return Err(FiError::Storage(
                    "stored wallet reservation does not match exact payment requirements"
                        .to_owned(),
                ));
            }
            match recovered {
                PaymentReservationRecovery::Existing(_) => {
                    self.inner
                        .store
                        .record_payment_reservation(
                            &recovery.snapshot.formation_id,
                            &reservation_id,
                        )
                        .await?;
                    recovery.payment_reservation_id = Some(reservation_id);
                    // Adoption also durably supersedes any interrupted
                    // release commitment.
                    recovery.payment_reservation_release_intended = false;
                }
                PaymentReservationRecovery::Absent if recovery.payment_reservation_id.is_some() => {
                    if !recovery.payment_reservation_release_intended {
                        return Err(FiError::Storage(
                            "durable FI reservation is absent from the payment wallet".to_owned(),
                        ));
                    }
                    // A prior run durably committed to this release and the
                    // wallet completed it; only the local half of that
                    // cleanup remains. Which half is identified by the
                    // value-safety boundary: pre-output commitments come
                    // from abandon, post-output commitments from a
                    // provisional-replacement restore.
                    if recovery.payment_outputs_started {
                        let witness =
                            ReleasedReplacementReservation::after_absence_under_release_intent(
                                reservation_id,
                            );
                        run.lease
                            .restore_provisional_replacements(
                                &recovery.snapshot.formation_id,
                                Some(&witness),
                            )
                            .await?;
                        *recovery = self.active_recovery(fi_id).await?;
                    } else {
                        run.lease
                            .abandon_formation(&recovery.snapshot.formation_id)
                            .await?;
                        self.inner.progress.send_replace(FiStatus::Idle);
                        return Ok(());
                    }
                }
                PaymentReservationRecovery::Absent => {}
            }
        }

        validate_live_admissions(
            recovery,
            self.inner.peer_badge_verifier.provenance().into(),
            Timestamp(now_secs()?),
        )?;

        if let Some(requirements) =
            crate::db::replacement_requirements(&recovery.snapshot.formation_id, &recovery.seats)
        {
            recovery.snapshot.phase = FormationPhase::AwaitingPaymentReadiness;
            recovery.snapshot.action_required =
                Some(FormationActionRequired::ReplaceGuardians(requirements));
            recovery.snapshot.freshness = FormationFreshness::Fresh;
            self.publish_snapshot(recovery.snapshot.clone());
            return Ok(());
        }

        self.prepare_quotes(recovery, fi_id, run).await?;
        if let Some(requirements) = recovery.payment_requirements(fi_id)?
            && !recovery.payments_authorized(&requirements)
        {
            // An intent spending cap that covers the checked aggregate total
            // self-authorizes exactly once: the same durable quote-bound
            // authorization is recorded as an explicit `authorize_payments`
            // call would write, so recovery and abandon semantics are
            // identical. The cap is the consumer's approval of the *initial*
            // aggregate only, so self-authorization is gated on the durable
            // `payment_authorization_recorded` tombstone: once any aggregate
            // authorization was ever recorded, a replaced quote set (for
            // example re-quoted after a verified refusal cleared a member)
            // parks for explicit authorization even when the fresh total is
            // still under the cap. A cap that is absent or exceeded likewise
            // parks the formation with the aggregate action carrying both
            // numbers.
            let under_cap = !recovery.payment_authorization_recorded
                && recovery
                    .snapshot
                    .intent
                    .max_total_msats
                    .is_some_and(|cap| requirements.total_msats <= cap);
            if under_cap {
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
                        &recovery.snapshot.formation_id,
                        &requirements.authorization_id,
                        &authorizations,
                    )
                    .await?;
                recovery.apply_payment_authorization(
                    requirements.authorization_id.clone(),
                    authorizations,
                );
            } else if recovery.creation_mode.is_selected() && !recovery.payment_outputs_started {
                return Err(FiError::SelectionReauthorizationRequired(
                    SelectionReauthorizationReason::QuoteTotalExceedsLimit,
                ));
            } else {
                recovery.snapshot.phase = FormationPhase::AwaitingPaymentReadiness;
                recovery.snapshot.action_required =
                    Some(FormationActionRequired::AuthorizePayments(requirements));
                recovery.snapshot.freshness = FormationFreshness::Fresh;
                self.publish_snapshot(recovery.snapshot.clone());
                return Ok(());
            }
        }

        recovery.snapshot.action_required = None;
        recovery.snapshot.phase = FormationPhase::AcquiringSeats;
        recovery.snapshot.freshness = FormationFreshness::Fresh;
        self.publish_snapshot(recovery.snapshot.clone());
        let sessions = self.acquire_seats(recovery, fi_id, run).await?;
        self.run_dkg(recovery, &sessions, fi_id, run).await
    }

    async fn prepare_quotes(
        &self,
        recovery: &mut ActiveFormationRecovery,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<()> {
        let formation_id = recovery.snapshot.formation_id.clone();
        let intent = recovery.snapshot.intent.clone();
        let expected_payer = recovery
            .creation_mode
            .selected_payment_federation()
            .cloned();

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
                fi_id,
                expected_payer.as_ref(),
            )?;
            self.inner
                .store
                .clear_quote(&formation_id, seat.progress.index, &signed_quote)
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
        let can_pay_for_formation = match &recovery.creation_mode {
            FormationCreationMode::Pinned => self.can_pay(),
            FormationCreationMode::Selected {
                payment_federation_id,
            } => payment_federation_id.is_some(),
        };
        let payment_federation = (needs_quote && can_pay_for_formation).then(OnceCell::new);
        let mut pending = FuturesUnordered::new();
        for (position, seat) in recovery.seats.iter().enumerate() {
            if seat.progress.seat_id.is_some() || seat.signed_quote.is_some() {
                continue;
            }
            let locator = seat.progress.locator.clone();
            let intent = &intent;
            let expected_payer = expected_payer.clone();
            let payment_federation = payment_federation.as_ref();
            let policy = quote_attempt_policy(&recovery.creation_mode, &seat.admission);
            pending.push(async move {
                let (signed_quote, _) = self
                    .request_quote_with_retry(
                        position,
                        &locator,
                        intent,
                        fi_id,
                        policy,
                        expected_payer.as_ref(),
                        payment_federation,
                        run,
                    )
                    .await?;
                Ok::<_, FiError>((position, signed_quote))
            });
        }

        let mut first_error = None;
        while let Some(result) = pending.next().await {
            match result {
                Ok((position, signed_quote)) => {
                    let index = recovery.seats[position].progress.index;
                    self.inner
                        .store
                        .store_quote(&formation_id, index, signed_quote.clone())
                        .await?;
                    recovery.invalidate_payment_authorization();
                    recovery.seats[position].signed_quote = Some(signed_quote);
                    recovery.seats[position].progress.phase = SeatPhase::QuoteReady;
                    recovery.seats[position].progress.freshness = FormationFreshness::Fresh;
                    recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
                    recovery.snapshot.phase = FormationPhase::Preparing;
                    recovery.snapshot.freshness = FormationFreshness::Fresh;
                    self.publish_snapshot(recovery.snapshot.clone());
                }
                Err(error) => {
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
        recovery: &mut ActiveFormationRecovery,
        requirements: &PaymentRequirements,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<P::PaymentReservation> {
        let (reservation_id, recovered) = self
            .recover_payment_reservation(recovery, requirements, fi_id, run)
            .await?;
        if recovery
            .payment_reservation_id
            .as_ref()
            .is_some_and(|stored| stored != &reservation_id)
        {
            return Err(FiError::Storage(
                "stored wallet reservation does not match exact payment requirements".to_owned(),
            ));
        }
        if let PaymentReservationRecovery::Existing(reservation) = recovered {
            self.inner
                .store
                .record_payment_reservation(&recovery.snapshot.formation_id, &reservation_id)
                .await?;
            recovery.payment_reservation_id = Some(reservation_id);
            return Ok(reservation);
        }
        if recovery.payment_reservation_id.is_some() {
            return Err(FiError::Storage(
                "durable FI reservation is absent from the payment wallet".to_owned(),
            ));
        }
        if recovery.creation_mode.is_selected() {
            validate_live_admissions(
                recovery,
                self.inner.peer_badge_verifier.provenance().into(),
                Timestamp(now_secs()?),
            )?;
        }

        let quotes = self.exact_payment_quotes(recovery, requirements, fi_id)?;
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
            .map_err(|error| match &recovery.creation_mode {
                FormationCreationMode::Selected { .. }
                    if error.proves_insufficient_funds_without_reservation() =>
                {
                    FiError::SelectionReauthorizationRequired(
                        SelectionReauthorizationReason::SelectedPayerInsufficientFunds,
                    )
                }
                FormationCreationMode::Selected { .. } | FormationCreationMode::Pinned => {
                    FiError::Payment(error.to_string())
                }
            })?;
        self.inner
            .store
            .record_payment_reservation(&recovery.snapshot.formation_id, &reservation_id)
            .await?;
        recovery.payment_reservation_id = Some(reservation_id);
        Ok(reservation)
    }

    /// Probe the deterministic aggregate id and exact quote/output binding
    /// without creating wallet state or consulting expiring admission policy.
    async fn recover_payment_reservation(
        &self,
        recovery: &ActiveFormationRecovery,
        requirements: &PaymentRequirements,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<(
        PaymentReservationId,
        PaymentReservationRecovery<P::PaymentReservation>,
    )> {
        let quotes = self.exact_payment_quotes(recovery, requirements, fi_id)?;
        let preflight = crate::ExactPaymentPreflight::new(requirements, &quotes)?;
        let reservation_id =
            crate::db::payment_reservation_id(&recovery.snapshot.formation_id, requirements);
        let recovered = run
            .call("recovering exact payment reservation", || {
                Ok(self
                    .inner
                    .ports
                    .payments
                    .recover_payment_reservation(&reservation_id, &preflight))
            })
            .await?
            .map_err(|error| FiError::Payment(error.to_string()))?;
        Ok((reservation_id, recovered))
    }

    fn exact_payment_quotes(
        &self,
        recovery: &ActiveFormationRecovery,
        requirements: &PaymentRequirements,
        fi_id: FiId,
    ) -> FiResult<Vec<SignatureVerified<GetQuoteResponse>>> {
        let expected_payer = recovery
            .creation_mode
            .selected_payment_federation()
            .cloned();
        let mut quotes = Vec::with_capacity(requirements.seats.len());
        for requirement in &requirements.seats {
            let seat = recovery
                .seats
                .iter()
                .find(|seat| seat.progress.index == requirement.index)
                .ok_or_else(|| {
                    FiError::Storage(format!(
                        "payment requirement names missing FI seat row {}",
                        requirement.index
                    ))
                })?;
            let signed = seat.signed_quote.as_ref().ok_or_else(|| {
                FiError::Storage(format!(
                    "payment requirement has no quote for FI seat row {}",
                    requirement.index
                ))
            })?;
            let quote = self.verify_quote(
                usize::from(requirement.index),
                signed,
                &seat.progress.locator,
                &recovery.snapshot.intent,
                fi_id,
                expected_payer.as_ref(),
            )?;
            if quote.quote_id() != requirement.quote_id || quote.terms.payment.is_none() {
                return Err(FiError::Storage(format!(
                    "payment requirement does not match FI seat row {}",
                    requirement.index
                )));
            }
            quotes.push(quote);
        }
        Ok(quotes)
    }

    /// Resolve a selected FMan transport while replacement is still
    /// value-safe. A definite connection failure is retried with bounded
    /// exponential backoff for at most two minutes (and never beyond this
    /// driver invocation). After outputs start, the same failure remains an
    /// exact-replay error and must never be projected as replacement advice.
    async fn connect_with_selected_retry(
        &self,
        locator: &Locator,
        position: usize,
        policy: QuoteAttemptPolicy,
        run: DriverRun<'_>,
    ) -> FiResult<F::Client> {
        let retry_deadline = Instant::now()
            .checked_add(SELECTED_FMAN_CONNECT_RETRY_BUDGET)
            .unwrap_or(run.deadline)
            .min(run.deadline);
        let mut retry_delay = run
            .options
            .poll_interval
            .max(SELECTED_FMAN_CONNECT_RETRY_MIN_DELAY)
            .min(SELECTED_FMAN_CONNECT_RETRY_MAX_DELAY);

        loop {
            let attempt = run
                .call("connecting to Fleet Manager", || {
                    Ok(self.inner.ports.fman_connector.connect(locator))
                })
                .await;
            match attempt {
                Ok(Ok(client)) => return Ok(client),
                Ok(Err(error)) if !policy.allows_selection_reauthorization() => {
                    return Err(fman_error(position, error.to_string()));
                }
                Err(error) if !policy.allows_selection_reauthorization() => return Err(error),
                Ok(Err(_)) | Err(FiError::Timeout(_)) => {
                    if Instant::now() >= retry_deadline {
                        return Err(FiError::SelectionReauthorizationRequired(
                            SelectionReauthorizationReason::SelectedFmanUnavailable,
                        ));
                    }
                    if sleep_for_retry(retry_deadline, retry_delay).await.is_err() {
                        return Err(FiError::SelectionReauthorizationRequired(
                            SelectionReauthorizationReason::SelectedFmanUnavailable,
                        ));
                    }
                    retry_delay = retry_delay
                        .saturating_mul(2)
                        .min(SELECTED_FMAN_CONNECT_RETRY_MAX_DELAY);
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Connect, inspect availability, and request one quote as a single
    /// bounded value-free attempt. A selected row whose authorization has not
    /// been consumed may reconnect after definite transport loss; semantic
    /// refusals return immediately and an already value-bound row never gains
    /// replacement authority from a transport failure.
    #[allow(clippy::too_many_arguments)]
    async fn request_quote_with_retry(
        &self,
        index: usize,
        locator: &Locator,
        intent: &ResolvedFormationIntent,
        fi_id: FiId,
        policy: QuoteAttemptPolicy,
        expected_payer: Option<&FederationId>,
        payment_federation: Option<&OnceCell<FederationId>>,
        run: DriverRun<'_>,
    ) -> FiResult<(
        SignedResponse<GetQuoteResponse>,
        SignatureVerified<GetQuoteResponse>,
    )> {
        let retry_deadline = Instant::now()
            .checked_add(SELECTED_FMAN_CONNECT_RETRY_BUDGET)
            .unwrap_or(run.deadline)
            .min(run.deadline);
        let mut retry_delay = run
            .options
            .poll_interval
            .max(SELECTED_FMAN_CONNECT_RETRY_MIN_DELAY)
            .min(SELECTED_FMAN_CONNECT_RETRY_MAX_DELAY);

        loop {
            let client = match run
                .call("connecting to Fleet Manager", || {
                    Ok(self.inner.ports.fman_connector.connect(locator))
                })
                .await
            {
                Ok(Ok(client)) => client,
                Ok(Err(error)) if !policy.allows_selection_reauthorization() => {
                    return Err(fman_error(index, error.to_string()));
                }
                Err(error) if !policy.allows_selection_reauthorization() => return Err(error),
                Ok(Err(_)) | Err(FiError::Timeout(_)) => {
                    if retry_quote_attempt(retry_deadline, &mut retry_delay)
                        .await
                        .is_err()
                    {
                        return Err(FiError::SelectionReauthorizationRequired(
                            SelectionReauthorizationReason::SelectedFmanUnavailable,
                        ));
                    }
                    continue;
                }
                Err(error) => return Err(error),
            };

            match self
                .request_new_quote(
                    index,
                    &client,
                    locator,
                    intent,
                    fi_id,
                    policy,
                    expected_payer,
                    payment_federation,
                    run,
                )
                .await
            {
                Ok(quote) => return Ok(quote),
                Err(QuoteAttemptError::Transport { .. })
                | Err(QuoteAttemptError::Other(FiError::Timeout(_)))
                    if policy.allows_selection_reauthorization() =>
                {
                    if retry_quote_attempt(retry_deadline, &mut retry_delay)
                        .await
                        .is_err()
                    {
                        return Err(FiError::SelectionReauthorizationRequired(
                            SelectionReauthorizationReason::SelectedFmanUnavailable,
                        ));
                    }
                }
                Err(error) => return Err(error.into_fi_error()),
            }
        }
    }

    async fn acquire_seats(
        &self,
        recovery: &mut ActiveFormationRecovery,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<Vec<SeatSession<F::Client>>> {
        let intent = recovery.snapshot.intent.clone();
        let expected_payer = recovery
            .creation_mode
            .selected_payment_federation()
            .cloned();
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
            recovery.reserved_payment_requirements(fi_id)?
        } else {
            recovery.payment_requirements(fi_id)?
        };
        let recovery_reservation_id = recovery_requirements.as_ref().map(|requirements| {
            crate::db::payment_reservation_id(&recovery.snapshot.formation_id, requirements)
        });
        let mut payment_reservation = if let Some(requirements) = recovery_requirements.as_ref()
            && recovery.payments_authorized(requirements)
        {
            Some(
                self.reserve_payment_requirements(recovery, requirements, fi_id, run)
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
                fi_id,
                expected_payer.as_ref(),
            )?;
            if quote.terms.payment.is_none() {
                continue;
            }
            let reservation_id = recovery_reservation_id.clone().ok_or_else(|| {
                FiError::Storage("authorized paid quote has no aggregate reservation id".to_owned())
            })?;
            pending_recoveries.push(async move {
                let recovered = run
                    .call("recovering seat payment", || {
                        Ok(self
                            .inner
                            .ports
                            .payments
                            .recover_seat_payment(&reservation_id, &quote))
                    })
                    .await?
                    .map_err(|error| FiError::Payment(error.to_string()))?;
                Ok::<_, FiError>((position, recovered))
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
                fi_id,
                expected_payer.as_ref(),
            )?;
            pending_replays.push(async move {
                let client = run
                    .call("connecting to Fleet Manager", || {
                        Ok(self.inner.ports.fman_connector.connect(&locator))
                    })
                    .await?
                    .map_err(|error| fman_error(position, error.to_string()))?;
                let creation = self
                    .create_or_replay_seat(
                        position,
                        &client,
                        &locator,
                        fi_id,
                        SeatAcquisition::ReplayPrepared {
                            quote: paid_quote,
                            prepared,
                        },
                        run,
                    )
                    .await?;
                Ok::<_, FiError>((position, creation))
            });
        }
        while let Some(result) = pending_replays.next().await {
            let (position, creation) = match result {
                Ok(value) => value,
                Err(error) => {
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
                        first_barrier_error.get_or_insert(error);
                    }
                }
                SeatCreation::Refused(refusal) => {
                    terminal_quotes.push((position, refusal.release_proof));
                    first_terminal_outcome.get_or_insert(TerminalSeatOutcome::SeatRefused {
                        index: recovery.seats[position].progress.index,
                        reason: format!("{:?}", refusal.reason),
                    });
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
            outcome.requires_pre_output_reauthorization(
                &recovery.creation_mode,
                recovery.payment_outputs_started,
            )
        }) {
            // Selected all-free refusal is itself the reauthorization proof.
            // Keep its exact signed quote and consumed admission durable until
            // the outer driver completes durable abandon. A failed abandon can
            // then reopen and replay the same refusal safely.
            return Err(first_terminal_outcome
                .expect("checked terminal outcome exists")
                .into_formation_error(&recovery.creation_mode, recovery.payment_outputs_started));
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
        // re-quoting under a snapshot the FI never re-read would only be half
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
            match (&recovery.creation_mode, expected_payer.as_ref()) {
                (FormationCreationMode::Selected { .. }, Some(requested)) => Some(
                    self.require_setup_payment_federation(requested, run)
                        .await?,
                ),
                (FormationCreationMode::Selected { .. }, None) => {
                    return Err(FiError::SelectionReauthorizationRequired(
                        SelectionReauthorizationReason::PaymentFederationRequired,
                    ));
                }
                (FormationCreationMode::Pinned, _) if self.can_pay() => {
                    Some(self.select_setup_payment_federation(run).await?)
                }
                (FormationCreationMode::Pinned, _) => None,
            }
        } else {
            None
        };
        let mut refreshed_payments = match payment_federation_id {
            Some(payment_federation_id) => {
                self.refresh_unstarted_payments(
                    recovery,
                    &recovered_payments,
                    fi_id,
                    &intent,
                    payment_federation_id,
                    run,
                )
                .await?
            }
            None => (0..recovery.seats.len()).map(|_| None).collect(),
        };
        let will_generate_outputs = refreshed_payments.iter().any(Option::is_some);
        // Resolve every fallible FMan connection before crossing the durable
        // wallet-output boundary. Selected creation receives its bounded
        // pre-output retry policy; after outputs start, failures remain exact
        // recovery errors and never become replacement advice.
        let mut clients = (0..recovery.seats.len())
            .map(|_| None)
            .collect::<Vec<Option<F::Client>>>();
        let mut pending_connections = FuturesUnordered::new();
        for (position, seat) in recovery.seats.iter().enumerate() {
            if terminal_positions.contains(&position) {
                continue;
            }
            let locator = seat.progress.locator.clone();
            let connection_policy = quote_attempt_policy(&recovery.creation_mode, &seat.admission);
            pending_connections.push(async move {
                let client = self
                    .connect_with_selected_retry(&locator, position, connection_policy, run)
                    .await?;
                Ok::<_, FiError>((position, client))
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
                recovery.reserved_payment_requirements(fi_id)?
            } else {
                recovery.payment_requirements(fi_id)?
            }
            .ok_or_else(|| {
                FiError::Storage(
                    "paid output generation has no complete payment requirements".to_owned(),
                )
            })?;
            match payment_reservation.take() {
                Some(reservation) => Some(reservation),
                None => Some(
                    self.reserve_payment_requirements(recovery, &requirements, fi_id, run)
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
                FiError::Storage(format!(
                    "missing preflight FMan connection for seat {position}"
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
                .ok_or_else(|| FiError::Storage(format!("FI seat row {position} has no quote")))?;
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
                                fi_id,
                                expected_payer.as_ref(),
                            )?,
                        ))),
                    });
                }
                Some(SeatPaymentRecovery::NotStarted) => {
                    let quote = refreshed_payment.ok_or_else(|| {
                        FiError::Storage(format!(
                            "paid FI seat row {position} escaped the refresh barrier"
                        ))
                    })?;
                    let reservation = payment_reservation.clone().ok_or_else(|| {
                        FiError::Storage(format!(
                            "paid FI seat row {position} has no aggregate reservation"
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
                    return Err(FiError::Storage(format!(
                        "prepared FI seat row {position} escaped the replay barrier"
                    )));
                }
                Some(SeatPaymentRecovery::Rejected(_)) => {
                    return Err(FiError::Storage(format!(
                        "rejected FI seat row {position} escaped terminal clearing"
                    )));
                }
            }
        }
        self.publish_snapshot(recovery.snapshot.clone());

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
        if recovery.creation_mode.is_selected() {
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
                        &recovery.snapshot.formation_id,
                        &effects,
                        verifier_provenance,
                    )
                    .await?;
                    for &(position, _, quote_id, effect) in &free_effects {
                        apply_authorized_effect_in_memory(recovery, position, quote_id, effect)?;
                    }
                }
                run.arm_payment_outputs_started(
                    &recovery.snapshot.formation_id,
                    verifier_provenance,
                )
                .await?;
                recovery.payment_outputs_started = true;
                recovery.snapshot.payment_outputs_started = true;
                for &(position, _, quote_id, effect) in &paid_effects {
                    apply_authorized_effect_in_memory(recovery, position, quote_id, effect)?;
                }
                self.publish_snapshot(recovery.snapshot.clone());
            } else {
                // Post-output replacement can mix paid and free rows. Consume
                // the complete exact wave atomically before polling the first
                // wallet or presentation effect so expiry cannot strand only
                // the still-provisional half of the wave.
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
                        &recovery.snapshot.formation_id,
                        &effects,
                        verifier_provenance,
                    )
                    .await?;
                    for (position, _, quote_id, effect) in effect_wave {
                        apply_authorized_effect_in_memory(recovery, position, quote_id, effect)?;
                    }
                }
            }
        }
        if !funding.is_empty() {
            if !recovery.payment_outputs_started {
                // Pinned formation carries no selected admission wave, but it
                // still needs the aggregate output tombstone before polling.
                run.arm_payment_outputs_started(
                    &recovery.snapshot.formation_id,
                    self.inner.peer_badge_verifier.provenance().into(),
                )
                .await?;
                recovery.payment_outputs_started = true;
                recovery.snapshot.payment_outputs_started = true;
                self.publish_snapshot(recovery.snapshot.clone());
            }
        }

        let mut sessions = (0..recovery.seats.len())
            .map(|_| None)
            .collect::<Vec<Option<SeatSession<F::Client>>>>();
        let mut first_barrier_error = None;
        // A payment wallet may need the change from one accepted Fedimint
        // transaction to fund the next quote. Drive paid seats in stable seat
        // order and durably checkpoint each acceptance before polling the next
        // wallet future. Recovery and replacements enter through this same
        // list, so every newly started value movement has the same boundary.
        for funding in funding {
            let result = self
                .complete_pending_seat_work(PendingSeatWork::Funding(Box::new(funding)), fi_id, run)
                .await;
            let CompletedSeatWork {
                position,
                client,
                creation,
                checkpoint,
            } = match result {
                Ok(value) => value,
                Err(error) => {
                    first_barrier_error.get_or_insert(error);
                    break;
                }
            };
            let acceptance = match creation {
                SeatCreation::Accepted(acceptance) => acceptance,
                SeatCreation::Refused(refund) => {
                    terminal_quotes.push((position, refund.release_proof));
                    first_terminal_outcome.get_or_insert(TerminalSeatOutcome::SeatRefused {
                        index: recovery.seats[position].progress.index,
                        reason: format!("{:?}", refund.reason),
                    });
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
                PendingSeatWork::Presentation(presentation),
                fi_id,
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
                    first_barrier_error.get_or_insert(error);
                    continue;
                }
            };
            let acceptance = match creation {
                SeatCreation::Accepted(acceptance) => acceptance,
                SeatCreation::Refused(refund) => {
                    terminal_quotes.push((position, refund.release_proof));
                    first_terminal_outcome.get_or_insert(TerminalSeatOutcome::SeatRefused {
                        index: recovery.seats[position].progress.index,
                        reason: format!("{:?}", refund.reason),
                    });
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
            outcome.requires_pre_output_reauthorization(
                &recovery.creation_mode,
                recovery.payment_outputs_started,
            )
        }) {
            return Err(first_terminal_outcome
                .expect("checked terminal outcome exists")
                .into_formation_error(&recovery.creation_mode, recovery.payment_outputs_started));
        }
        let terminal_positions = terminal_quotes
            .iter()
            .map(|(position, _)| *position)
            .collect::<Vec<_>>();
        for (position, proof) in terminal_quotes {
            self.release_terminal_quote_reservation(recovery, position, fi_id, proof, run)
                .await?;
        }
        if !terminal_positions.is_empty() {
            self.clear_terminal_quotes(recovery, &terminal_positions)
                .await?;
        }
        if let Some(outcome) = first_terminal_outcome {
            return Err(outcome
                .into_formation_error(&recovery.creation_mode, recovery.payment_outputs_started));
        }
        sessions
            .into_iter()
            .enumerate()
            .map(|(position, session)| {
                session.ok_or_else(|| {
                    FiError::Storage(format!("missing live session for seat {position}"))
                })
            })
            .collect()
    }

    async fn refresh_unstarted_payments(
        &self,
        recovery: &mut ActiveFormationRecovery,
        recovered_payments: &[Option<
            SeatPaymentRecovery<P::RefundContext, P::TerminalReleaseProof>,
        >],
        fi_id: FiId,
        intent: &ResolvedFormationIntent,
        payment_federation_id: FederationId,
        run: DriverRun<'_>,
    ) -> FiResult<Vec<Option<PaidSeatQuote>>> {
        let formation_id = recovery.snapshot.formation_id.clone();
        let expected_payer = recovery
            .creation_mode
            .selected_payment_federation()
            .cloned();
        if recovery.payment_reservation_id.is_some() {
            let requirements = recovery
                .reserved_payment_requirements(fi_id)?
                .ok_or_else(|| {
                    FiError::Storage(
                        "stored wallet reservation has no exact payment requirements".to_owned(),
                    )
                })?;
            let expected = crate::db::payment_reservation_id(&formation_id, &requirements);
            if recovery.payment_reservation_id.as_ref() != Some(&expected) {
                return Err(FiError::Storage(
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
                        FiError::Storage(format!(
                            "reserved FI seat row {position} has no signed quote"
                        ))
                    })?;
                    let locator = seat.progress.locator.clone();
                    self.verify_paid_seat_quote(
                        position,
                        signed,
                        &locator,
                        intent,
                        fi_id,
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
                fi_id,
                expected_payer.as_ref(),
            )?;
            let intent = intent.clone();
            let expected_payer = expected_payer.clone();
            let payment_federation = Some(&payment_federation);
            let policy = quote_attempt_policy(&recovery.creation_mode, &seat.admission);
            pending.push(async move {
                let (signed, verified) = self
                    .request_quote_with_retry(
                        position,
                        &locator,
                        &intent,
                        fi_id,
                        policy,
                        expected_payer.as_ref(),
                        payment_federation,
                        run,
                    )
                    .await?;
                Ok::<_, FiError>((position, old_quote, PaidSeatQuote { signed, verified }))
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
                    &formation_id,
                    seat_index,
                    &old.signed,
                    fresh.signed.clone(),
                )
                .await?;
            recovery.seats[position].signed_quote = Some(fresh.signed.clone());
            refreshed[position] = Some(fresh);
        }
        let first_changed_position = changed.first().map(|(position, _, _)| *position);
        for (position, old, fresh) in changed {
            let seat_index = recovery.seats[position].progress.index;
            self.inner
                .store
                .clear_quote(&formation_id, seat_index, &old.signed)
                .await?;
            self.inner
                .store
                .store_quote(&formation_id, seat_index, fresh.signed.clone())
                .await?;
            recovery.invalidate_payment_authorization();
            recovery.seats[position].signed_quote = Some(fresh.signed);
            recovery.seats[position].progress.phase = SeatPhase::QuoteReady;
            recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
        }
        if let Some(position) = first_changed_position {
            if recovery.creation_mode.is_selected() {
                return Err(FiError::SelectionReauthorizationRequired(
                    crate::SelectionReauthorizationReason::QuoteTermsChanged,
                ));
            }
            return Err(FiError::Payment(format!(
                "payment terms for Fleet Manager {} changed; review the fresh quote",
                position + 1
            )));
        }
        Ok(refreshed)
    }

    async fn run_dkg(
        &self,
        recovery: &mut ActiveFormationRecovery,
        sessions: &[SeatSession<F::Client>],
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<()> {
        let formation_id = recovery.snapshot.formation_id.clone();
        recovery.snapshot.phase = FormationPhase::PreparingDkg;
        self.publish_snapshot(recovery.snapshot.clone());

        let mut pending_codes = FuturesUnordered::new();
        for (position, session) in sessions.iter().enumerate() {
            let existing = recovery.seats[position].progress.guardian_code.clone();
            let federation_name =
                (position == 0).then(|| recovery.snapshot.intent.federation_name.clone());
            pending_codes.push(async move {
                let code = self
                    .get_dkg_code_with_retry(session, fi_id, federation_name, existing.clone(), run)
                    .await?;
                if let Some(recorded) = existing.as_ref()
                    && recorded != &code
                {
                    return Err(FiError::InvalidFleetManagers(format!(
                        "Fleet Manager {} changed its deterministic guardian code",
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
                        &formation_id,
                        recovery.seats[position].progress.index,
                        code.clone(),
                    )
                    .await?;
                recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
                self.publish_snapshot(recovery.snapshot.clone());
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
                    FiError::Storage(format!("missing guardian code for seat {position}"))
                })
            })
            .collect::<FiResult<Vec<_>>>()?;

        let mut pending_starts = FuturesUnordered::new();
        for (position, session) in sessions.iter().enumerate() {
            let timestamp = Timestamp(now_secs()?);
            let completion_callback = recovery.dkg_completion_callback.clone();
            let guardian_codes = guardian_codes.clone();
            let seat_id = session.seat_id.clone();
            pending_starts.push(async move {
                let request = StartDkgRequest {
                    ts: timestamp,
                    fi_id,
                    seat_id,
                    guardian_codes,
                    completion_callback,
                };
                let request = run
                    .construct("signing StartDkg request", || self.sign(&request))
                    .await?;
                let result = run
                    .call("starting DKG", || Ok(session.client.start_dkg(request)))
                    .await?;
                match result {
                    Ok(_)
                    | Err(FleetManagerError::WrongState {
                        status: ServiceStatus::DkgInProcess,
                    }) => Ok::<_, FiError>((position, false)),
                    Err(FleetManagerError::WrongState {
                        status: ServiceStatus::Running,
                    }) => Ok((position, false)),
                    Err(error) => Err(fman_error(position, error.to_string())),
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
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        recovery.snapshot.phase = FormationPhase::DkgUnderway;
        self.publish_snapshot(recovery.snapshot.clone());
        self.poll_until_running(sessions, recovery, fi_id, run)
            .await?;

        let invite = self.fetch_agreed_invite(sessions, fi_id, run).await?;
        recovery.snapshot.invite_code = Some(invite.clone());
        recovery.snapshot.action_required = None;
        // The federation is formed before its directory exists: record that
        // first, so a directory publish interrupted below resumes through
        // `reconcile_formed` instead of re-running DKG.
        self.inner
            .store
            .record_formed(&formation_id, invite.clone())
            .await?;
        self.publish_seat_bindings(sessions, recovery, fi_id, &invite, run)
            .await?;
        recovery.snapshot.phase = FormationPhase::Formed;
        recovery.snapshot.freshness = FormationFreshness::Fresh;
        recovery.snapshot.last_error = None;
        self.publish_snapshot(recovery.snapshot.clone());
        Ok(())
    }

    async fn checkpoint_accepted_seat(
        &self,
        recovery: &mut ActiveFormationRecovery,
        position: usize,
        acceptance: SeatAcceptance,
    ) -> FiResult<()> {
        let all_created = recovery
            .seats
            .iter()
            .enumerate()
            .all(|(index, seat)| index == position || seat.progress.seat_id.is_some());
        self.inner
            .store
            .record_seat_accepted(
                &recovery.snapshot.formation_id,
                recovery.seats[position].progress.index,
                acceptance.seat_id.clone(),
                acceptance.guardian_fee_account.clone(),
            )
            .await?;
        recovery.seats[position].progress.seat_id = Some(acceptance.seat_id);
        recovery.seats[position].guardian_fee_account = Some(acceptance.guardian_fee_account);
        recovery.seats[position].progress.phase = SeatPhase::Created;
        recovery.seats[position].progress.freshness = FormationFreshness::Fresh;
        recovery.snapshot.phase = if all_created {
            FormationPhase::PreparingDkg
        } else {
            FormationPhase::AcquiringSeats
        };
        recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
        self.publish_snapshot(recovery.snapshot.clone());
        Ok(())
    }

    async fn release_terminal_quote_reservation(
        &self,
        recovery: &ActiveFormationRecovery,
        position: usize,
        fi_id: FiId,
        release_proof: Option<P::TerminalReleaseProof>,
        run: DriverRun<'_>,
    ) -> FiResult<()> {
        let signed_quote = recovery.seats[position]
            .signed_quote
            .as_ref()
            .expect("terminal quote outcome requires a stored quote");
        let verified = self.verify_quote(
            position,
            signed_quote,
            &recovery.seats[position].progress.locator,
            &recovery.snapshot.intent,
            fi_id,
            recovery.creation_mode.selected_payment_federation(),
        )?;
        if verified.terms.payment.is_some() {
            let release_proof = release_proof.ok_or_else(|| {
                FiError::Payment(format!(
                    "terminal paid FI seat row {position} has no wallet release proof"
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
            .map_err(|error| FiError::Payment(error.to_string()))?;
        } else if release_proof.is_some() {
            return Err(FiError::Storage(format!(
                "free FI seat row {position} unexpectedly carried a wallet release proof"
            )));
        }
        Ok(())
    }

    async fn clear_terminal_quotes(
        &self,
        recovery: &mut ActiveFormationRecovery,
        positions: &[usize],
    ) -> FiResult<()> {
        let mark_replacements =
            recovery.payment_outputs_started && recovery.creation_mode.is_selected();
        let mut cleared = Vec::with_capacity(positions.len());
        for &position in positions {
            let signed_quote = recovery.seats[position]
                .signed_quote
                .as_ref()
                .expect("terminal quote outcome requires a stored quote")
                .clone();
            let replacement_for = mark_replacements
                .then(|| {
                    signed_quote
                        .verify(&recovery.seats[position].progress.locator.service_pubkey)
                        .map(|quote| quote.quote_id())
                        .map_err(|error| {
                            FiError::Storage(format!(
                                "invalid terminal quote for replacement row {position}: {error}"
                            ))
                        })
                })
                .transpose()?;
            cleared.push((position, signed_quote, replacement_for));
        }

        let expected_quotes = cleared
            .iter()
            .map(|(position, signed_quote, _)| {
                (
                    recovery.seats[*position].progress.index,
                    signed_quote.clone(),
                )
            })
            .collect::<Vec<_>>();
        self.inner
            .store
            .clear_terminal_quotes(
                &recovery.snapshot.formation_id,
                &expected_quotes,
                mark_replacements,
            )
            .await?;

        recovery.invalidate_payment_authorization();
        recovery.payment_reservation_id = None;
        for (position, _, replacement_for) in cleared {
            recovery.seats[position].signed_quote = None;
            recovery.seats[position].replacement_for = replacement_for;
            recovery.seats[position].replacement_previous_locator =
                replacement_for.map(|_| recovery.seats[position].progress.locator.clone());
            recovery.seats[position].replacement_previous_fman_id =
                replacement_for.and_then(|_| recovery.seats[position].admission.fman_id());
            recovery.seats[position].replacement_approved = false;
            recovery.seats[position].progress.phase = if replacement_for.is_some() {
                SeatPhase::ReplacementRequired
            } else {
                SeatPhase::Selected
            };
            recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
        }
        if let Some(requirements) =
            crate::db::replacement_requirements(&recovery.snapshot.formation_id, &recovery.seats)
        {
            recovery.snapshot.action_required =
                Some(FormationActionRequired::ReplaceGuardians(requirements));
        }
        Ok(())
    }

    async fn complete_pending_seat_work(
        &self,
        work: PendingSeatWork<F::Client, P::RefundContext, P::PaymentReservation>,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<CompletedSeatWork<F::Client, P::TerminalReleaseProof>> {
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
                        self.inner
                            .ports
                            .payments
                            .create_seat_payment(&reservation, &quote.verified),
                    )
                    .await?
                    .map_err(|error| FiError::Payment(error.to_string()))?;
                let creation = self
                    .create_or_replay_seat(
                        position,
                        &client,
                        &locator,
                        fi_id,
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
                    .create_or_replay_seat(position, &client, &locator, fi_id, *acquisition, run)
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
        index: usize,
        client: &F::Client,
        locator: &Locator,
        fi_id: FiId,
        acquisition: SeatAcquisition<P::RefundContext>,
        run: DriverRun<'_>,
    ) -> FiResult<SeatCreation<P::TerminalReleaseProof>> {
        // A stored quote is reused only when payment has already started
        // against it. Otherwise obtain and persist a fresh quote before
        // committing funds, so prepared blind nonces can never be stranded.
        let presentation = match acquisition {
            SeatAcquisition::Free(quote) => {
                return self
                    .present_seat(
                        index,
                        client,
                        locator,
                        fi_id,
                        SeatPresentation::Free(quote),
                        run,
                    )
                    .await;
            }
            SeatAcquisition::ReplayPrepared { quote, prepared } => {
                SeatPresentation::Paid { quote, prepared }
            }
        };
        self.present_seat(index, client, locator, fi_id, presentation, run)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn request_new_quote(
        &self,
        index: usize,
        client: &F::Client,
        locator: &Locator,
        intent: &ResolvedFormationIntent,
        fi_id: FiId,
        policy: QuoteAttemptPolicy,
        expected_payer: Option<&FederationId>,
        payment_federation: Option<&OnceCell<FederationId>>,
        run: DriverRun<'_>,
    ) -> Result<
        (
            SignedResponse<GetQuoteResponse>,
            SignatureVerified<GetQuoteResponse>,
        ),
        QuoteAttemptError,
    > {
        let availability = run
            .call("checking Fleet Manager availability", || {
                Ok(self
                    .inner
                    .ports
                    .fman_connector
                    .get_availability(client, GetAvailabilityRequest))
            })
            .await?
            .map_err(|error| QuoteAttemptError::Transport {
                index,
                message: error.to_string(),
            })?
            .map_err(|error| {
                quote_attempt_error(index, error, QuoteAttemptPolicy::ExactRecovery)
            })?;
        // One shared predicate with the selection walk's live probe
        // (`selection::match_requested_availability`), so a candidate the
        // probing preview seats is exactly a candidate this gate accepts.
        let matched = match crate::selection::match_requested_availability(
            &availability,
            intent.federation_size,
            &intent.fedimintd_versions,
            &intent.fedimintd_dkg_version,
            intent.plan,
        ) {
            Ok(matched) => matched,
            Err(mismatch @ AvailabilityMismatch::NotAcceptingSeats) => {
                return Err((if policy.allows_selection_reauthorization() {
                    FiError::SelectionReauthorizationRequired(
                        crate::SelectionReauthorizationReason::SelectedFmanUnavailable,
                    )
                } else {
                    fman_error(index, mismatch.message())
                })
                .into());
            }
            Err(mismatch) => {
                return Err(selected_availability_error(policy, index, mismatch.message()).into());
            }
        };
        let fedimintd_version = matched.fedimintd_version.clone();
        let plan = matched.plan.clone();
        // Free-ness is a property of the price, not of the plan: an FMan that
        // gives its seats away is quoted against no payment federation
        // whatever this FI is configured to pay from, and one that charges
        // cannot be quoted at all by an FI with nothing to pay from.
        let priced = match &plan {
            Plan::InfiniteBestEffort { price_msats } => *price_msats > 0,
            Plan::SubscriptionBased { .. } => true,
        };
        let payment_federation_id = if priced {
            let payment_federation = payment_federation.ok_or_else(|| {
                if policy.allows_selection_reauthorization() {
                    FiError::SelectionReauthorizationRequired(
                        SelectionReauthorizationReason::PaymentFederationRequired,
                    )
                } else {
                    fman_error(
                        index,
                        "Fleet Manager charges for a seat, and this FI has no authenticated \
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
                            None => self.select_setup_payment_federation(run).await,
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
                .map_err(|error| FiError::Payment(error.to_string()))?,
            ),
            None => None,
        };
        let quote_request = GetQuoteRequest {
            fi_id,
            fedimintd_version,
            federation_size: intent.federation_size,
            plan,
            payment_federation_id,
            refund_issuance,
        };
        let signed_quote = run
            .call("requesting Fleet Manager quote", || {
                Ok(self
                    .inner
                    .ports
                    .fman_connector
                    .get_quote(client, quote_request.clone()))
            })
            .await?
            .map_err(|error| QuoteAttemptError::Transport {
                index,
                message: error.to_string(),
            })?
            .map_err(|error| quote_attempt_error(index, error, policy))?;
        let quote =
            self.verify_quote(index, &signed_quote, locator, intent, fi_id, expected_payer)?;
        if quote.terms.request != quote_request {
            return Err(fman_error(index, "quote echoed a different request").into());
        }
        Ok((signed_quote, quote))
    }

    fn verify_quote(
        &self,
        index: usize,
        signed_quote: &SignedResponse<GetQuoteResponse>,
        locator: &Locator,
        intent: &ResolvedFormationIntent,
        fi_id: FiId,
        expected_payer: Option<&FederationId>,
    ) -> FiResult<SignatureVerified<GetQuoteResponse>> {
        let quote = signed_quote
            .verify(&locator.service_pubkey)
            .map_err(|error| fman_error(index, format!("invalid signed quote: {error}")))?;
        let request = &quote.terms.request;
        if request.fi_id != fi_id
            || request.fedimintd_version.dkg_version() != intent.fedimintd_dkg_version
            || !intent
                .fedimintd_versions
                .contains(&request.fedimintd_version)
            || request.federation_size != intent.federation_size
            || !intent.plan.matches(&request.plan)
        {
            return Err(fman_error(
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
            return Err(fman_error(
                index,
                "stored quote does not match the selected payment federation",
            ));
        }
        quote
            .terms
            .check_coherent()
            .map_err(|error| fman_error(index, format!("incoherent quote: {error}")))?;
        Ok(quote)
    }

    fn verify_free_seat_quote(
        &self,
        index: usize,
        signed: SignedResponse<GetQuoteResponse>,
        locator: &Locator,
        intent: &ResolvedFormationIntent,
        fi_id: FiId,
        expected_payer: Option<&FederationId>,
    ) -> FiResult<FreeSeatQuote> {
        let verified = self.verify_quote(index, &signed, locator, intent, fi_id, expected_payer)?;
        if verified.terms.payment.is_some() {
            return Err(FiError::Storage(format!(
                "paid FI seat row {index} had no authorized payment action"
            )));
        }
        Ok(FreeSeatQuote { signed, verified })
    }

    fn verify_paid_seat_quote(
        &self,
        index: usize,
        signed: SignedResponse<GetQuoteResponse>,
        locator: &Locator,
        intent: &ResolvedFormationIntent,
        fi_id: FiId,
        expected_payer: Option<&FederationId>,
    ) -> FiResult<PaidSeatQuote> {
        let verified = self.verify_quote(index, &signed, locator, intent, fi_id, expected_payer)?;
        if verified.terms.payment.is_none() {
            return Err(FiError::Storage(format!(
                "free FI seat row {index} had a paid acquisition action"
            )));
        }
        Ok(PaidSeatQuote { signed, verified })
    }

    async fn present_seat(
        &self,
        index: usize,
        client: &F::Client,
        locator: &Locator,
        fi_id: FiId,
        presentation: SeatPresentation<P::RefundContext>,
        run: DriverRun<'_>,
    ) -> FiResult<SeatCreation<P::TerminalReleaseProof>> {
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
                    return Err(fman_error(
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
            fi_id,
            quote: signed_quote,
            payment_signatures,
        };
        let request = run
            .construct("signing CreateSeat request", || self.sign(&request))
            .await?;
        let response = run
            .call("creating Fleet Manager seat", || {
                Ok(client.create_seat(request))
            })
            .await?
            .map_err(|error| fman_error(index, error.to_string()))?
            .verify(&locator.service_pubkey)
            .map_err(|error| {
                fman_error(
                    index,
                    format!("invalid signed CreateSeat response: {error}"),
                )
            })?;
        if response.quote_id != quote_id {
            return Err(fman_error(index, "CreateSeat answered a different quote"));
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
                .map_err(|error| {
                    fman_error(
                        index,
                        format!("CreateSeat committed an invalid guardian-fee account: {error}"),
                    )
                })?;
                Ok(SeatCreation::Accepted(SeatAcceptance {
                    seat_id,
                    guardian_fee_account: guardian_fee_account.into_account(),
                }))
            }
            CreateSeatOutcome::Refused {
                reason,
                refund_transaction,
            } => {
                let release_proof = match (refund_context, refund_transaction) {
                    (Some(refund_context), Some(refund)) => Some(
                        run.call("settling refused seat refund", || {
                            Ok(self
                                .inner
                                .ports
                                .payments
                                .settle_seat_refund(refund_context, refund))
                        })
                        .await?
                        .map_err(|error| FiError::Payment(error.to_string()))?
                        .release_proof,
                    ),
                    (None, None) => None,
                    _ => {
                        return Err(fman_error(
                            index,
                            "refusal payment and refund material did not match the quote",
                        ));
                    }
                };
                Ok(SeatCreation::Refused(SeatRefusal {
                    reason,
                    release_proof,
                }))
            }
        }
    }

    async fn get_dkg_code_with_retry(
        &self,
        session: &SeatSession<F::Client>,
        fi_id: FiId,
        federation_name: Option<FederationName>,
        recorded_code: Option<GuardianCode>,
        run: DriverRun<'_>,
    ) -> FiResult<GuardianCode> {
        loop {
            ensure_time_remaining(run.deadline, "waiting for FMan child readiness")?;
            let request = GetDkgCodeRequest {
                ts: Timestamp(now_secs()?),
                fi_id,
                seat_id: session.seat_id.clone(),
                federation_name: federation_name.clone(),
            };
            let request = run
                .construct("signing GetDkgCode request", || self.sign(&request))
                .await?;
            match run
                .call("requesting guardian code", || {
                    Ok(session.client.get_dkg_code(request))
                })
                .await?
            {
                Ok(response) => return Ok(response.guardian_code),
                Err(FleetManagerError::SeatUnavailable) => {
                    sleep_for_retry(run.deadline, run.options.poll_interval).await?;
                }
                Err(FleetManagerError::WrongState {
                    status: ServiceStatus::DkgInProcess | ServiceStatus::Running,
                }) if recorded_code.is_some() => {
                    return Ok(recorded_code.expect("matched present recorded code"));
                }
                Err(error) => {
                    return Err(fman_error(usize::from(session.index), error.to_string()));
                }
            }
        }
    }

    async fn poll_until_running(
        &self,
        sessions: &[SeatSession<F::Client>],
        recovery: &mut ActiveFormationRecovery,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<()> {
        loop {
            ensure_time_remaining(run.deadline, "waiting for every FMan to report running")?;
            let mut running = 0;
            let mut pending_status = FuturesUnordered::new();
            for (position, session) in sessions.iter().enumerate() {
                let request = GetStatusRequest {
                    ts: Timestamp(now_secs()?),
                    fi_id,
                    seat_id: session.seat_id.clone(),
                };
                pending_status.push(async move {
                    let request = run
                        .construct("signing GetStatus request", || self.sign(&request))
                        .await?;
                    let status = run
                        .call("checking Fleet Manager status", || {
                            Ok(session.client.get_status(request))
                        })
                        .await?
                        .map_err(|error| fman_error(position, error.to_string()))?;
                    Ok::<_, FiError>((position, status))
                });
            }
            while let Some(result) = pending_status.next().await {
                let (position, status) = result?;
                recovery.seats[position].progress.phase =
                    service_seat_phase(position, &status.status)?;
                recovery.seats[position].progress.freshness = FormationFreshness::Fresh;
                recovery.snapshot.seats[position] = recovery.seats[position].progress.clone();
                if status.status == ServiceStatus::Running
                    && status.seat_health == Some(SeatHealth::Healthy)
                {
                    running += 1;
                }
            }
            recovery.snapshot.freshness = FormationFreshness::Fresh;
            self.publish_snapshot(recovery.snapshot.clone());
            if running == sessions.len() {
                return Ok(());
            }
            sleep_for_retry(run.deadline, run.options.poll_interval).await?;
        }
    }

    async fn reconcile_formed(
        &self,
        recovery: &mut ActiveFormationRecovery,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<()> {
        let sessions = self.formed_sessions(recovery, run).await?;
        self.poll_until_running(&sessions, recovery, fi_id, run)
            .await?;
        let invite = self.fetch_agreed_invite(&sessions, fi_id, run).await?;
        let stored = recovery.snapshot.invite_code.as_ref().ok_or_else(|| {
            FiError::Storage("formed FI record contains no persisted invite".to_owned())
        })?;
        if invite_federation_id(stored)? != invite_federation_id(&invite)? {
            return Err(FiError::InvalidFleetManagers(
                "formed federation identity changed during reconciliation".to_owned(),
            ));
        }
        self.publish_seat_bindings(&sessions, recovery, fi_id, &invite, run)
            .await?;
        recovery.snapshot.phase = FormationPhase::Formed;
        recovery.snapshot.freshness = FormationFreshness::Fresh;
        recovery.snapshot.last_error = None;
        self.publish_snapshot(recovery.snapshot.clone());
        Ok(())
    }

    async fn formed_sessions(
        &self,
        recovery: &ActiveFormationRecovery,
        run: DriverRun<'_>,
    ) -> FiResult<Vec<SeatSession<F::Client>>> {
        let mut sessions = Vec::with_capacity(recovery.seats.len());
        for (position, seat) in recovery.seats.iter().enumerate() {
            let seat_id = seat.progress.seat_id.clone().ok_or_else(|| {
                FiError::Storage(format!("formed FI seat row {position} has no seat id"))
            })?;
            let client = run
                .call("reconnecting to formed Fleet Manager", || {
                    Ok(self
                        .inner
                        .ports
                        .fman_connector
                        .connect(&seat.progress.locator))
                })
                .await?
                .map_err(|error| fman_error(position, error.to_string()))?;
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
        sessions: &[SeatSession<F::Client>],
        recovery: &mut ActiveFormationRecovery,
        fi_id: FiId,
        invite: &InviteCode,
        run: DriverRun<'_>,
    ) -> FiResult<()> {
        recovery.snapshot.phase = FormationPhase::PublishingSeatBindings;
        self.publish_snapshot(recovery.snapshot.clone());

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
        let mut target = match recovery.formation_meta_target.clone() {
            Some(target) => target,
            None => {
                let (bindings, binding_entries) = self
                    .assemble_seat_bindings(sessions, recovery, fi_id, run)
                    .await?;
                let federation_id = invite_federation_id(invite)?;
                let fi_account = self
                    .inner
                    .ports
                    .fi_fee_account_provider
                    .formed_federation_fee_account(&federation_id)
                    .map_err(|_| {
                        FiError::CapabilityUnavailable(crate::Capability::FeeArrangement)
                    })?;
                let guardian_verification_fee_account =
                    self.inner.guardian_verification_fee_account.clone().ok_or(
                        FiError::CapabilityUnavailable(crate::Capability::FeeArrangement),
                    )?;
                let recipients = canonical_fee_recipients(
                    formation_config
                        .as_ref()
                        .expect("a new formation target has a downloaded config"),
                    &bindings,
                    fi_account.clone(),
                    guardian_verification_fee_account.clone(),
                )?;
                let recipients =
                    canonical_guardian_fee_recipient_list(&recipients).map_err(|error| {
                        FiError::InvalidFleetManagers(format!(
                            "guardian-fee recipient accounts cannot form canonical metadata: {error}"
                        ))
                    })?;
                let send_ppm = self
                    .min_guardian_fee_ppm()
                    .await
                    .max(u64::from(GuardianFeePpm::MANIFOLD_DEFAULT.value()));
                let fi_fee_account = GuardianFeeAccount::try_from(fi_account).map_err(|error| {
                    FiError::InvalidIntent(format!("FI guardian-fee account is invalid: {error}"))
                })?;
                let guardian_verification_fee_account = GuardianFeeAccount::try_from(
                    guardian_verification_fee_account,
                )
                .map_err(|_| FiError::CapabilityUnavailable(crate::Capability::FeeArrangement))?;
                let target = FormationMetaTarget {
                    seat_bindings: bindings,
                    binding_entries,
                    fi_fee_account,
                    guardian_verification_fee_account,
                    send_ppm,
                    recipients,
                    confirmed: false,
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

        let mut pending_error = None;

        loop {
            ensure_time_remaining(run.deadline, "reading the formation metadata base")?;
            let snapshot = run
                .call("reading the formation metadata base", || {
                    Ok(self.inner.ports.consensus_reader.read_consensus(invite))
                })
                .await?;
            let Ok(snapshot) = snapshot else {
                sleep_for_retry(run.deadline, run.options.poll_interval).await?;
                continue;
            };
            validate_consensus_metadata_size(snapshot.meta_value.as_deref()).map_err(|error| {
                FiError::InvalidFleetManagers(format!(
                    "consensus metadata is {} bytes; formation permits at most {} bytes",
                    error.actual_bytes, error.max_bytes
                ))
            })?;
            let immutable_matches = self.seat_bindings_match(&snapshot, &target.seat_bindings)?
                && guardian_fee_recipients_match(
                    snapshot.meta_value.as_deref(),
                    &target.recipients,
                )?;
            let exact_matches = immutable_matches
                && guardian_fee_rate_matches(snapshot.meta_value.as_deref(), target.send_ppm)?;
            if (target.confirmed && immutable_matches) || exact_matches {
                if !target.confirmed {
                    run.call("confirming formation metadata consensus", || {
                        Ok(self
                            .inner
                            .store
                            .confirm_formation_meta_target(&recovery.snapshot.formation_id))
                    })
                    .await??;
                    target.confirmed = true;
                    recovery.formation_meta_target = Some(target.clone());
                }
                return Ok(());
            }
            if target.confirmed {
                return Err(FiError::InvalidFleetManagers(
                    "formed federation changed its immutable directory or fee recipients"
                        .to_owned(),
                ));
            }
            if let Some(error) = pending_error.take() {
                return Err(error);
            }

            let expected_base = MetaConsensusBase::from_consensus(
                snapshot_meta_consensus(&snapshot).map_err(FiError::InvalidFleetManagers)?,
            );
            let results = self
                .submit_formation_meta_wave(
                    sessions,
                    fi_id,
                    expected_base,
                    &target.binding_entries,
                    &target.fi_fee_account,
                    &target.guardian_verification_fee_account,
                    target.send_ppm,
                    run,
                )
                .await;
            for (index, result) in results {
                match result {
                    Ok(_)
                    | Err(MetaFieldSubmissionError::FleetManager(
                        FleetManagerError::MetaConsensusChanged,
                    )) => {}
                    Err(MetaFieldSubmissionError::FleetManager(
                        FleetManagerError::FormationMetaAlreadyPublished,
                    )) => {
                        pending_error.get_or_insert_with(|| {
                            FiError::InvalidFleetManagers(
                                "federation already published different formation metadata"
                                    .to_owned(),
                            )
                        });
                    }
                    Err(MetaFieldSubmissionError::FleetManager(
                        error @ (FleetManagerError::GuardianVerificationFeeAccountUnavailable
                        | FleetManagerError::GuardianVerificationFeeAccountMismatch),
                    )) => {
                        return Err(FiError::FleetManager {
                            index,
                            message: error.to_string(),
                        });
                    }
                    Err(error) => {
                        pending_error.get_or_insert_with(|| error.into_formation_error(index));
                    }
                }
            }
            sleep_for_retry(run.deadline, run.options.poll_interval).await?;
        }
    }

    /// Collect one FMan attestation and endpoint proof per seat.
    async fn assemble_seat_bindings(
        &self,
        sessions: &[SeatSession<F::Client>],
        recovery: &ActiveFormationRecovery,
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<(String, Vec<FormationSeatBinding>)> {
        let mut pending = FuturesUnordered::new();
        for (position, session) in sessions.iter().enumerate() {
            let expected_guardian_fee_account = recovery.seats[position]
                .guardian_fee_account
                .clone()
                .ok_or_else(|| {
                    FiError::InvalidFleetManagers(format!(
                        "accepted seat {position} has no persisted guardian-fee account"
                    ))
                })?;
            pending.push(async move {
                let request = GetPeerAttestationRequest {
                    ts: Timestamp(now_secs()?),
                    fi_id,
                    seat_id: session.seat_id.clone(),
                };
                let request = run
                    .construct("signing GetPeerAttestation request", || self.sign(&request))
                    .await?;
                let response = run
                    .call("fetching the FMan peer attestation", || {
                        Ok(session.client.get_peer_attestation(request))
                    })
                    .await?
                    .map_err(|error| fman_error(position, error.to_string()))?;
                if response.fman_peer_attestation.attestation.guardian_fee_account
                    != expected_guardian_fee_account
                {
                    return Err(FiError::InvalidFleetManagers(format!(
                        "Fleet Manager {position} attested a guardian-fee account that differs from its signed seat acceptance"
                    )));
                }
                Ok::<_, FiError>(FormationSeatBinding {
                    attestation: response.fman_peer_attestation,
                    endpoint_proof: response.seat_endpoint_proof,
                })
            });
        }
        let mut entries = Vec::with_capacity(sessions.len());
        while let Some(result) = pending.next().await {
            entries.push(result?);
        }

        let bindings = FmanSeatBindings::new(entries.iter().map(|entry| entry.attestation.clone()))
            .and_then(|bindings| bindings.canonical_string())
            .map_err(|error| {
                FiError::InvalidFleetManagers(format!(
                    "Fleet Manager attestations do not form a valid seat-binding directory: {error}"
                ))
            })?;
        Ok((bindings, entries))
    }

    async fn submit_formation_meta_wave(
        &self,
        sessions: &[SeatSession<F::Client>],
        fi_id: FiId,
        expected_base: MetaConsensusBase,
        seat_bindings: &[FormationSeatBinding],
        fi_fee_account: &GuardianFeeAccount,
        guardian_verification_fee_account: &GuardianFeeAccount,
        send_ppm: u64,
        run: DriverRun<'_>,
    ) -> Vec<(
        u16,
        Result<ProposeFormationMetaResponse, MetaFieldSubmissionError>,
    )> {
        let mut pending = FuturesUnordered::new();
        for session in sessions {
            pending.push(async move {
                let result = async {
                    let request = ProposeFormationMetaRequest {
                        ts: Timestamp(now_secs().map_err(MetaFieldSubmissionError::Driver)?),
                        fi_id,
                        seat_id: session.seat_id.clone(),
                        expected_base,
                        seat_bindings: seat_bindings.to_vec(),
                        fi_fee_account: fi_fee_account.clone(),
                        guardian_verification_fee_account: guardian_verification_fee_account
                            .clone(),
                        send_ppm,
                    };
                    let request = run
                        .construct("signing ProposeFormationMeta request", || {
                            self.sign(&request)
                        })
                        .await
                        .map_err(MetaFieldSubmissionError::Driver)?;
                    run.call("proposing formation metadata", || {
                        Ok(session.client.propose_formation_meta(request))
                    })
                    .await
                    .map_err(MetaFieldSubmissionError::Driver)?
                    .map_err(MetaFieldSubmissionError::FleetManager)
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

    /// Submit one guarded whole-object metadata wave through the supplied seats.
    ///
    /// The primitive deliberately returns every seat result instead of
    /// short-circuiting. Formation can require all seats while maintenance can
    /// read consensus after a threshold-live partial wave and retry missing
    /// guardians under its deadline.
    pub(crate) async fn submit_meta_field_wave(
        &self,
        sessions: &[SeatSession<F::Client>],
        fi_id: FiId,
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
                        fi_id,
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
                        Err(FleetManagerError::MetaConsensusChanged) => {
                            Ok(MetaFieldSubmission::BaseChanged)
                        }
                        Err(error) => Err(MetaFieldSubmissionError::FleetManager(error)),
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
    ) -> FiResult<bool> {
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
            FiError::InvalidFleetManagers(format!(
                "previewed federation config is not usable: {error}"
            ))
        })?;
        FmanSeatBindings::parse_canonical(&value)
            .and_then(|bindings| {
                bindings.verify_for_federation(&seats)?;
                Ok(())
            })
            .map_err(|error| {
                FiError::InvalidFleetManagers(format!(
                    "consensus seat-binding directory does not match the federation: {error}"
                ))
            })?;
        Ok(true)
    }

    async fn fetch_agreed_invite(
        &self,
        sessions: &[SeatSession<F::Client>],
        fi_id: FiId,
        run: DriverRun<'_>,
    ) -> FiResult<InviteCode> {
        if sessions.is_empty() {
            return Err(FiError::InvalidFleetManagers(
                "no Fleet Managers selected".to_owned(),
            ));
        }
        let mut pending = FuturesUnordered::new();
        for (position, session) in sessions.iter().enumerate() {
            pending.push(async move {
                let request = GetInviteCodeRequest {
                    ts: Timestamp(now_secs()?),
                    fi_id,
                    seat_id: session.seat_id.clone(),
                };
                let request = run
                    .construct("signing GetInviteCode request", || self.sign(&request))
                    .await?;
                let invite = run
                    .call("fetching federation invite code", || {
                        Ok(session.client.get_invite_code(request))
                    })
                    .await?
                    .map_err(|error| fman_error(position, error.to_string()))?
                    .invite_code;
                let federation_id = invite_federation_id(&invite)
                    .map_err(|error| fman_error(position, error.to_string()))?;
                Ok::<_, FiError>((position, invite, federation_id))
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
                FiError::Storage(format!("missing invite response for seat {position}"))
            })
        });
        let (deliverable, expected_id) = invites
            .next()
            .ok_or_else(|| FiError::Storage("formed FI record contains no seats".to_owned()))??;
        for invite in invites {
            let (_, federation_id) = invite?;
            if federation_id != expected_id {
                return Err(FiError::InvalidFleetManagers(
                    "selected Fleet Managers reported different federation identities".to_owned(),
                ));
            }
        }
        Ok(deliverable)
    }

    pub(crate) async fn active_recovery(&self, fi_id: FiId) -> FiResult<ActiveFormationRecovery> {
        match self.inner.store.load_recovery(fi_id).await? {
            FiRecovery::Idle => Err(FiError::NoActiveFormation),
            FiRecovery::Formation(recovery) => Ok(*recovery),
        }
    }

    fn publish_snapshot(&self, snapshot: FormationSnapshot) {
        self.inner
            .progress
            .send_replace(FiStatus::Formation(snapshot));
    }

    pub(crate) fn fi_id(&self) -> FiResult<FiId> {
        self.inner
            .ports
            .identity
            .public_key()
            .map_err(FiError::Identity)
    }

    pub(crate) fn sign<T: FiSignedRequest>(&self, request: &T) -> FiResult<SignedRequest<T>> {
        SignedRequest::create_with_signer(request, |digest| {
            self.inner.ports.identity.sign_digest(digest)
        })
        .map_err(|error| FiError::Identity(error.to_string()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReservationCleanup {
    /// The wallet reserve call returned an error and no capability exists.
    DefinitivelyAbsent,
    /// A wallet commit may have preceded an interrupted FI checkpoint.
    ReconstructIfAuthorized,
}

/// Non-serializable witness created only by the replacement cleanup paths:
/// either the consumer wallet authoritatively reported that the exact
/// reconstructed reservation was released in this run, or it reported the
/// reservation authoritatively absent after a durable release commitment
/// from an interrupted earlier run.
pub(crate) struct ReleasedReplacementReservation {
    reservation_id: PaymentReservationId,
    witnesses_absence: bool,
}

impl ReleasedReplacementReservation {
    fn after_wallet_release(reservation_id: PaymentReservationId) -> Self {
        Self {
            reservation_id,
            witnesses_absence: false,
        }
    }

    fn after_absence_under_release_intent(reservation_id: PaymentReservationId) -> Self {
        Self {
            reservation_id,
            witnesses_absence: true,
        }
    }

    pub(crate) fn reservation_id(&self) -> &PaymentReservationId {
        &self.reservation_id
    }

    /// Whether this witness proves current absence rather than a release
    /// performed in this run; such a witness is honored only under the
    /// durable release commitment persisted before that release began.
    pub(crate) fn witnesses_absence(&self) -> bool {
        self.witnesses_absence
    }
}

fn canonical_fee_recipients(
    config: &ClientConfig,
    seat_bindings: &str,
    fi_account: Account,
    guardian_verification_fee_account: Account,
) -> FiResult<Vec<GuardianFeeRecipient>> {
    let fi_fee_account = GuardianFeeAccount::try_from(fi_account.clone()).map_err(|error| {
        FiError::InvalidIntent(format!("FI guardian-fee account is invalid: {error}"))
    })?;
    let guardian_verification_fee_account =
        GuardianFeeAccount::try_from(guardian_verification_fee_account.clone())
            .map_err(|_| FiError::CapabilityUnavailable(crate::Capability::FeeArrangement))?;
    let federation = federation_seats(config).map_err(|error| {
        FiError::InvalidFleetManagers(format!("formed federation config is invalid: {error}"))
    })?;
    let bindings = FmanSeatBindings::parse_canonical(seat_bindings)
        .and_then(|bindings| bindings.verify_for_federation(&federation))
        .map_err(|error| {
            FiError::InvalidFleetManagers(format!(
                "seat-binding directory does not match the formed config: {error}"
            ))
        })?;

    let mut recipients = Vec::with_capacity(bindings.len() + 2);
    for binding in bindings {
        let guardian_account =
            GuardianFeeAccount::try_from(binding.guardian_fee_account).map_err(|error| {
                FiError::InvalidFleetManagers(format!(
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
        fi_fee_account,
        FI_GUARDIAN_FEE_WEIGHT,
    ));
    recipients.push(GuardianFeeRecipient::new(
        guardian_verification_fee_account,
        GUARDIAN_VERIFICATION_FEE_WEIGHT,
    ));
    recipients.sort_by_key(|recipient| recipient.account.as_account().id());
    canonical_guardian_fee_recipient_list(&recipients).map_err(|error| {
        FiError::InvalidFleetManagers(format!(
            "guardian-fee accounts do not form a canonical recipient set: {error}"
        ))
    })?;
    Ok(recipients)
}

#[cfg(any(test, feature = "dev-pinned-formation"))]
fn pinned_dkg_version(intent: &FormationIntent) -> FiResult<FedimintdDkgVersion> {
    let core = intent.fedimintd_versions().only_core().ok_or_else(|| {
        FiError::InvalidIntent("pinned formation requires one fedimintd patch release".to_owned())
    })?;
    Ok(format!("{core}+fedi")
        .parse::<FedimintdVersion>()
        .expect("a release core with the fixed Fedi vendor is valid SemVer")
        .dkg_version())
}

#[cfg(any(test, feature = "dev-pinned-formation"))]
fn validate_locators(intent: &FormationIntent, locators: &[Locator]) -> FiResult<()> {
    if locators.len() != usize::from(intent.federation_size().0) {
        return Err(FiError::InvalidFleetManagers(format!(
            "expected {} pinned locators, got {}",
            intent.federation_size().0,
            locators.len()
        )));
    }
    let mut keys = HashSet::new();
    if let Some(duplicate) = locators
        .iter()
        .map(|locator| locator.service_pubkey)
        .find(|key| !keys.insert(*key))
    {
        return Err(FiError::InvalidFleetManagers(format!(
            "duplicate FMan service key {duplicate}"
        )));
    }
    Ok(())
}

fn service_seat_phase(index: usize, status: &ServiceStatus) -> FiResult<SeatPhase> {
    match status {
        ServiceStatus::New => Ok(SeatPhase::Created),
        ServiceStatus::DkgInProcess => Ok(SeatPhase::DkgUnderway),
        ServiceStatus::DataLoss => Err(FiError::FleetManager {
            index: u16::try_from(index).expect("validated formation size fits u16"),
            message: "guardian data loss; decommission and replace this seat".to_owned(),
        }),
        ServiceStatus::Running => Ok(SeatPhase::Running),
        ServiceStatus::Decommissioned => Err(FiError::FleetManager {
            index: u16::try_from(index).expect("validated formation size fits u16"),
            message: format!("seat entered terminal/non-running status {status}"),
        }),
    }
}

fn default_federation_name(fi_id: FiId, created_at: u64) -> FederationName {
    const ADJECTIVES: [&str; 16] = [
        "Amber", "Bright", "Calm", "Cedar", "Clear", "Copper", "Gentle", "Golden", "Kind",
        "Lively", "Quiet", "Silver", "Solar", "Steady", "Warm", "Wild",
    ];
    const NOUNS: [&str; 16] = [
        "Badger", "Beacon", "Bison", "Canyon", "Falcon", "Forest", "Harbor", "Juniper", "Meadow",
        "Orchid", "Otter", "Raven", "River", "Summit", "Willow", "Wren",
    ];
    let key = fi_id.0.serialize();
    let adjective = ADJECTIVES[usize::from(key[0] ^ (created_at as u8)) % ADJECTIVES.len()];
    let noun = NOUNS[usize::from(key[31] ^ ((created_at >> 8) as u8)) % NOUNS.len()];
    FederationName(format!("{adjective} {noun}"))
}

pub(crate) fn ensure_time_remaining(deadline: Instant, operation: &'static str) -> FiResult<()> {
    if Instant::now() >= deadline {
        return Err(FiError::Timeout(operation.to_owned()));
    }
    Ok(())
}

fn ensure_effective_time_remaining(deadline: Instant, operation: &'static str) -> FiResult<()> {
    select_timer_duration(
        MIN_RUNTIME_TIMER_DURATION,
        deadline.saturating_duration_since(Instant::now()),
        operation,
    )
    .map(drop)
}

pub(crate) async fn sleep_for_retry(deadline: Instant, interval: Duration) -> FiResult<()> {
    let operation = "waiting to retry Fleet Manager";
    let duration = select_timer_duration(
        interval,
        deadline.saturating_duration_since(Instant::now()),
        operation,
    )?;
    // Pass the selected relative duration directly. Reconstructing it from an
    // absolute deadline can lose the WASM timer's final millisecond.
    sleep(duration).await;
    ensure_time_remaining(deadline, "waiting to retry Fleet Manager")
}

async fn retry_quote_attempt(deadline: Instant, delay: &mut Duration) -> Result<(), ()> {
    if Instant::now() >= deadline || sleep_for_retry(deadline, *delay).await.is_err() {
        return Err(());
    }
    *delay = (*delay)
        .saturating_mul(2)
        .min(SELECTED_FMAN_CONNECT_RETRY_MAX_DELAY);
    Ok(())
}

fn select_timer_duration(
    configured: Duration,
    run_remaining: Duration,
    operation: &'static str,
) -> FiResult<Duration> {
    if run_remaining < MIN_RUNTIME_TIMER_DURATION {
        return Err(FiError::Timeout(operation.to_owned()));
    }
    Ok(configured.min(run_remaining))
}

/// Extract the `fedi:fman_seat_bindings` string from a raw meta object.
///
/// The `meta` module holds one JSON object of fields under its default key,
/// and the directory is one string field within it. `Ok(None)` means the
/// federation carries no directory yet, which is the normal state while
/// consensus is still forming.
fn seat_bindings_field(meta_value: &Option<Vec<u8>>) -> FiResult<Option<String>> {
    let Some(fields) = parse_metadata_object(meta_value.as_deref()).map_err(|error| {
        FiError::InvalidFleetManagers(format!("consensus metadata is not a JSON object: {error}"))
    })?
    else {
        return Ok(None);
    };

    match fields.get(FMAN_SEAT_BINDINGS_META_FIELD_KEY) {
        None => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(FiError::InvalidFleetManagers(format!(
            "{FMAN_SEAT_BINDINGS_META_FIELD_KEY} consensus metadata is not a string"
        ))),
    }
}

pub(crate) fn meta_field_matches(
    meta_value: Option<&[u8]>,
    key: &MetaFieldKey,
    expected: &MetaFieldValue,
) -> FiResult<bool> {
    let Some(fields) = parse_metadata_object(meta_value).map_err(|error| {
        FiError::MaintenanceConsensusInvalid {
            reason: format!("metadata is not a JSON object: {error}"),
        }
    })?
    else {
        return Ok(false);
    };
    match fields.get(&key.0) {
        None => Ok(false),
        Some(serde_json::Value::String(value)) => Ok(value == &expected.0),
        Some(_) => Err(FiError::MaintenanceConsensusInvalid {
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

fn guardian_fee_rate_matches(meta_value: Option<&[u8]>, send_ppm: u64) -> FiResult<bool> {
    validate_consensus_metadata_size(meta_value).map_err(|error| {
        FiError::MaintenanceConsensusTooLarge {
            actual_bytes: error.actual_bytes,
            max_bytes: error.max_bytes,
        }
    })?;
    let Some(fields) = parse_metadata_object(meta_value).map_err(|error| {
        FiError::InvalidFleetManagers(format!("consensus metadata is not a JSON object: {error}"))
    })?
    else {
        return Ok(false);
    };
    match fields.get("fedi:guardian_fee_send_ppm") {
        Some(serde_json::Value::String(rate)) => Ok(rate == &send_ppm.to_string()),
        None => Ok(false),
        Some(_) => Err(FiError::InvalidFleetManagers(
            "guardian-fee consensus rate metadata field is not a string".to_owned(),
        )),
    }
}

fn guardian_fee_recipients_match(meta_value: Option<&[u8]>, recipients: &str) -> FiResult<bool> {
    validate_consensus_metadata_size(meta_value).map_err(|error| {
        FiError::MaintenanceConsensusTooLarge {
            actual_bytes: error.actual_bytes,
            max_bytes: error.max_bytes,
        }
    })?;
    let Some(fields) = parse_metadata_object(meta_value).map_err(|error| {
        FiError::InvalidFleetManagers(format!("consensus metadata is not a JSON object: {error}"))
    })?
    else {
        return Ok(false);
    };
    match fields.get("fedi:guardian_fee_remittance_account") {
        Some(serde_json::Value::String(value)) => Ok(value == recipients),
        None => Ok(false),
        Some(_) => Err(FiError::InvalidFleetManagers(
            "guardian-fee consensus recipients metadata field is not a string".to_owned(),
        )),
    }
}

fn fman_error(index: usize, message: impl Into<String>) -> FiError {
    FiError::FleetManager {
        index: u16::try_from(index).expect("validated formation size fits u16"),
        message: message.into(),
    }
}

fn selected_availability_error(
    policy: QuoteAttemptPolicy,
    index: usize,
    message: impl Into<String>,
) -> FiError {
    if policy.allows_selection_reauthorization() {
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::SelectedFmanUnavailable,
        )
    } else {
        fman_error(index, message)
    }
}

fn quote_attempt_error(
    index: usize,
    error: FleetManagerError,
    policy: QuoteAttemptPolicy,
) -> QuoteAttemptError {
    match error {
        error
            if policy.allows_selection_reauthorization()
                && matches!(
                    error,
                    FleetManagerError::CapacityExhausted
                        | FleetManagerError::PlanNotOffered
                        | FleetManagerError::PaymentFederationNotAccepted
                        | FleetManagerError::UnsupportedVersion
                        | FleetManagerError::UnsupportedFederationSize
                ) =>
        {
            QuoteAttemptError::Other(FiError::SelectionReauthorizationRequired(
                SelectionReauthorizationReason::SelectedFmanUnavailable,
            ))
        }
        error => QuoteAttemptError::Other(fman_error(index, error.to_string())),
    }
}

pub(crate) fn now_secs() -> FiResult<u64> {
    Ok(fedimint_core::time::duration_since_epoch().as_secs())
}

fn validate_live_admissions(
    recovery: &ActiveFormationRecovery,
    expected_provenance: StoredVerifierProvenance,
    now: Timestamp,
) -> FiResult<()> {
    for seat in &recovery.seats {
        seat.admission.validate_if_fresh(expected_provenance, now)?;
    }
    Ok(())
}

fn invite_federation_id(invite: &InviteCode) -> FiResult<FedimintFederationId> {
    let parsed = FedimintInviteCode::from_str(&invite.0).map_err(|error| {
        FiError::InvalidFleetManagers(format!(
            "Fleet Manager returned an invalid federation invite: {error}"
        ))
    })?;
    if parsed.api_secret().is_some() {
        return Err(FiError::InvalidFleetManagers(
            "Fleet Manager returned a bearer-secret federation invite".to_owned(),
        ));
    }
    Ok(parsed.federation_id())
}

/// Create the actual monotonic deadline after the pre-effect check.
pub(crate) fn formation_deadline(options: FormationRunOptions) -> FiResult<Instant> {
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
) -> FiResult<Instant> {
    now.checked_add(duration)
        .ok_or(InvalidFormationRunOptions::DeadlineOverflow { field })
        .map_err(FiError::from)
}

/// Establish the absolute monotonic deadline before awaiting lease acquisition.
pub(crate) async fn start_driver_run(
    store: &FiStore,
    options: FormationRunOptions,
) -> FiResult<(Instant, DriverLease)> {
    let deadline = formation_deadline(options)?;
    let lease = store
        .acquire_driver_lease(options.lease_duration(), options.lease_renewal_duration())
        .await?;
    Ok((deadline, lease))
}

pub(crate) fn finish_driver_run<T>(result: FiResult<T>, release: FiResult<()>) -> FiResult<T> {
    match result {
        Err(error) => Err(error),
        Ok(value) => release.map(|()| value),
    }
}
