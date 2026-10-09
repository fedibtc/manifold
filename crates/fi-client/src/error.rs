//! FI client errors.

/// FI client result.
pub type FiResult<T> = Result<T, FiError>;

/// A runtime capability not yet connected to the engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Registry discovery and trust-material refresh.
    Registry,
    /// FMan RPC transport.
    FleetManagerTransport,
    /// Consumer wallet payment.
    Payments,
    /// FLIP RPC and independent completion verification.
    Liquidity,
    /// Post-formation guardian fee metadata arrangement.
    FeeArrangement,
}

/// Typed reason the active formation is past its value-safe abandon window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AbandonUnavailableReason {
    /// The wallet output-generation call was durably armed. The tombstone
    /// survives quote or authorization replacement, and teardown now requires
    /// exact payment recovery.
    PaymentOutputsStarted,
    /// DKG finished; the saved invite and guardian state must be retained.
    DkgComplete,
}

impl std::fmt::Display for AbandonUnavailableReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PaymentOutputsStarted => {
                "payment output generation started; exact recovery must finish before teardown"
            }
            Self::DkgComplete => "federation key generation is complete",
        })
    }
}

/// Typed reason a Pay-and-create attempt must return to fresh user approval.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionReauthorizationReason {
    /// The advertisement preview's five-minute approval window elapsed.
    PreviewExpired,
    /// The consumer's limit is below the advertisement estimate it displayed.
    AdvertisementEstimateExceedsLimit,
    /// A selected FMan no longer advertises compatible live availability.
    SelectedFmanUnavailable,
    /// The exact signed quote total exceeds the approved spending limit.
    QuoteTotalExceedsLimit,
    /// An exact quote changed before output generation could begin.
    QuoteTermsChanged,
    /// The explicitly selected payer is no longer admitted and ready.
    SelectedPayerUnavailable,
    /// The selected live offer requires payment, but this Pay-and-create
    /// attempt deliberately supplied no payer.
    PaymentFederationRequired,
    /// The selected payer cannot cover the exact quote aggregate plus wallet
    /// fees and required reserve without starting output generation.
    SelectedPayerInsufficientFunds,
    /// The active verifier profile differs from the one that admitted the
    /// approved guardian set.
    VerifierEnvironmentChanged,
}

impl std::fmt::Display for SelectionReauthorizationReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PreviewExpired => "the verified selection preview expired",
            Self::AdvertisementEstimateExceedsLimit => {
                "the approved limit is below the displayed advertisement estimate"
            }
            Self::SelectedFmanUnavailable => {
                "a selected Fleet Manager is no longer available under the approved offer"
            }
            Self::QuoteTotalExceedsLimit => {
                "the exact setup quote total exceeds the approved limit"
            }
            Self::QuoteTermsChanged => {
                "an exact setup quote changed before payment output generation"
            }
            Self::SelectedPayerUnavailable => {
                "the selected payment federation is no longer admitted and ready"
            }
            Self::PaymentFederationRequired => {
                "a selected Fleet Manager requires a payment federation"
            }
            Self::SelectedPayerInsufficientFunds => {
                "the selected payment federation cannot fund the exact aggregate and fees"
            }
            Self::VerifierEnvironmentChanged => {
                "the PeerBadge verifier environment changed since guardian approval"
            }
        })
    }
}

/// Stable progress-facing error category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FiErrorCode {
    /// Consumer intent failed validation.
    InvalidIntent,
    /// Formation run options failed validation.
    InvalidOptions,
    /// Durable state could not be read or written.
    Storage,
    /// The consumer-provided identity could not be used.
    Identity,
    /// Another driver is already active.
    Busy,
    /// No active formation exists.
    NoActiveFormation,
    /// The active formation can no longer be abandoned value-safely.
    AbandonUnavailable,
    /// A registry relay operation failed.
    Registry,
    /// Verified seat selection or its advertised estimate failed.
    Selection,
    /// A fresh preview or explicit approval is required before payment.
    SelectionReauthorizationRequired,
    /// A later-stage capability is unavailable.
    CapabilityUnavailable,
    /// Pinned Fleet Manager inputs were invalid.
    InvalidFleetManagers,
    /// A local verdict about a seat; no remote request is attributed.
    FleetManager,
    /// Consumer payment or refund settlement failed.
    Payment,
    /// FLIP discovery, trust admission, request, or recovery failed.
    Liquidity,
    /// A maintenance verb was attempted outside the formed lifecycle state.
    MaintenanceWrongState,
    /// A guardian terminally rejected an otherwise valid maintenance request.
    MaintenanceRejected,
    /// The federation's complete consensus metadata object exceeded policy.
    MaintenanceConsensusTooLarge,
    /// The federation's consensus metadata was not a usable JSON object.
    MaintenanceConsensusInvalid,
    /// Maintenance could not reach consensus before the caller's deadline.
    MaintenanceConvergence,
    /// An operation, including formation or selection preview, missed its
    /// caller-visible deadline.
    Timeout,
}

/// Whether an unchanged formation should resume without a user command.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureDisposition {
    /// A later attempt can succeed without new authorization or state repair.
    Retryable,
    /// Stop automatic retries until an explicit command or a fresh launch.
    ///
    /// This does not authorize abandonment and does not mean funds are lost.
    Terminal,
}

/// Runtime-only formation failure. Durable recovery remains the source of truth
/// and a fresh launch rechecks it rather than persisting a failure verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FormationFailure {
    /// Stable progress-facing category.
    pub code: FiErrorCode,
    /// Manifold-owned automatic retry policy.
    pub disposition: FailureDisposition,
}

impl From<&FiError> for FormationFailure {
    fn from(error: &FiError) -> Self {
        Self {
            code: error.code(),
            disposition: error.disposition(),
        }
    }
}

/// A failure produced by one identity-bound Fleet Manager operation.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("FMan {fman} {operation}: {cause}")]
pub struct FmanFailure {
    pub fman: secp256k1::XOnlyPublicKey,
    pub operation: FmanOperation,
    pub cause: FmanCause,
}

/// Closed request names; never supplied by a remote peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FmanOperation {
    Connect,
    GetAvailability,
    GetQuote,
    CreateSeat,
    GetDkgCode,
    StartDkg,
    RestartDkg,
    GetStatus,
    GetInviteCode,
    GetPeerAttestation,
    ProposeFormationMeta,
    WaitForRunning,
}
impl FmanOperation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::GetAvailability => "get_availability",
            Self::GetQuote => "get_quote",
            Self::CreateSeat => "create_seat",
            Self::GetDkgCode => "get_dkg_code",
            Self::StartDkg => "start_dkg",
            Self::RestartDkg => "restart_dkg",
            Self::GetStatus => "get_status",
            Self::GetInviteCode => "get_invite_code",
            Self::GetPeerAttestation => "get_peer_attestation",
            Self::ProposeFormationMeta => "propose_formation_meta",
            Self::WaitForRunning => "wait_for_running",
        }
    }
}
impl std::fmt::Display for FmanOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Request causes, independent of the formation policy that handles them.
/// Remote free text is deliberately absent.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FmanCause {
    #[error("timeout")]
    Timeout,
    #[error("transport")]
    Transport,
    #[error("invalid_response")]
    InvalidResponse,
    #[error("not_accepting_seats")]
    NotAcceptingSeats,
    #[error("offer_changed")]
    OfferChanged,
    #[error("not_running")]
    NotRunning,
    #[error("plan_not_offered")]
    PlanNotOffered,
    #[error("payment_federation_not_accepted")]
    PaymentFederationNotAccepted,
    #[error("payment_federation_unavailable")]
    PaymentFederationUnavailable,
    #[error("invalid_payment")]
    InvalidPayment,
    #[error("capacity_exhausted")]
    CapacityExhausted,
    #[error("unknown_seat")]
    UnknownSeat,
    #[error("unsupported_version")]
    UnsupportedVersion,
    #[error("unsupported_federation_size")]
    UnsupportedFederationSize,
    #[error("unauthorized")]
    Unauthorized,
    #[error("seat_unavailable")]
    SeatUnavailable,
    #[error("federation_is_running")]
    FederationIsRunning,
    #[error("invalid_dkg_input")]
    InvalidDkgInput,
    #[error("meta_key_refused")]
    MetaKeyRefused,
    #[error("meta_value_invalid")]
    MetaValueInvalid,
    #[error("guardian_verification_fee_account_unavailable")]
    GuardianVerificationFeeAccountUnavailable,
    #[error("guardian_verification_fee_account_mismatch")]
    GuardianVerificationFeeAccountMismatch,
    #[error("meta_consensus_changed")]
    MetaConsensusChanged,
    #[error("formation_meta_already_published")]
    FormationMetaAlreadyPublished,
    #[error("meta_target_conflict")]
    MetaTargetConflict,
    #[error("invalid_gateway_api_url")]
    InvalidGatewayApiUrl,
    #[error("unsupported_verb")]
    UnsupportedVerb,
    #[error("other")]
    Other,
    #[error("wrong_state: {0}")]
    WrongState(fedi_decentralized_service_fleet_manager::ServiceStatus),
    #[error("terminal_status: {0}")]
    TerminalStatus(fedi_decentralized_service_fleet_manager::ServiceStatus),
}

impl From<fedi_decentralized_service_fleet_manager::FleetManagerError> for FmanCause {
    fn from(error: fedi_decentralized_service_fleet_manager::FleetManagerError) -> Self {
        use fedi_decentralized_service_fleet_manager::FleetManagerError as E;
        match error {
            E::PlanNotOffered => Self::PlanNotOffered,
            E::PaymentFederationNotAccepted => Self::PaymentFederationNotAccepted,
            E::PaymentFederationUnavailable => Self::PaymentFederationUnavailable,
            E::InvalidPayment => Self::InvalidPayment,
            E::CapacityExhausted => Self::CapacityExhausted,
            E::UnknownSeat => Self::UnknownSeat,
            E::UnsupportedVersion => Self::UnsupportedVersion,
            E::UnsupportedFederationSize => Self::UnsupportedFederationSize,
            E::Unauthorized => Self::Unauthorized,
            E::SeatUnavailable => Self::SeatUnavailable,
            E::FederationIsRunning => Self::FederationIsRunning,
            E::MetaKeyRefused => Self::MetaKeyRefused,
            E::MetaValueInvalid => Self::MetaValueInvalid,
            E::GuardianVerificationFeeAccountUnavailable => {
                Self::GuardianVerificationFeeAccountUnavailable
            }
            E::GuardianVerificationFeeAccountMismatch => {
                Self::GuardianVerificationFeeAccountMismatch
            }
            E::MetaConsensusChanged => Self::MetaConsensusChanged,
            E::FormationMetaAlreadyPublished => Self::FormationMetaAlreadyPublished,
            E::MetaTargetConflict => Self::MetaTargetConflict,
            E::InvalidGatewayApiUrl => Self::InvalidGatewayApiUrl,
            E::WrongState { status } => Self::WrongState(status),
            E::InvalidDkgInput(_) => Self::InvalidDkgInput,
            E::UnsupportedVerb { .. } => Self::UnsupportedVerb,
            E::Other(_) => Self::Other,
        }
    }
}

/// Error returned by FI client operations.
#[derive(Debug, thiserror::Error)]
pub enum FiError {
    #[error(transparent)]
    Fman(#[from] FmanFailure),
    /// Consumer intent failed validation.
    #[error("invalid formation intent: {0}")]
    InvalidIntent(String),
    /// Formation run options failed validation.
    #[error(transparent)]
    InvalidOptions(#[from] crate::InvalidFormationRunOptions),
    /// Durable state could not be read or written.
    #[error("FI storage failure: {0}")]
    Storage(String),
    /// Durable or in-memory recovery facts violate an engine invariant.
    /// Retrying unchanged facts cannot repair them; this is not database I/O.
    #[error("FI storage invariant failure: {0}")]
    StorageInvariant(String),
    /// The consumer-provided identity could not be used.
    #[error("FI identity failure: {0}")]
    Identity(String),
    /// Another driver is already active.
    #[error("formation operation already running")]
    Busy,
    /// No active formation exists.
    #[error("no active formation")]
    NoActiveFormation,
    /// The active formation is past its value-safe abandon window: wallet
    /// output generation was durably armed or the federation already formed.
    /// Commercial authorization alone does not close the window.
    #[error("formation cannot be abandoned: {0}")]
    AbandonUnavailable(AbandonUnavailableReason),
    /// A registry relay operation failed.
    #[error("FI registry failure: {0}")]
    Registry(String),
    /// A verified selection could not be represented safely (for example,
    /// because its aggregate estimate or validity deadline overflowed).
    #[error("FI selection failure: {0}")]
    Selection(String),
    /// The selection walk could not verified-fill the requested seat count.
    ///
    /// Initial public preview maps its absolute deadline to
    /// [`Self::SelectionPreviewTimeout`]. Lower-level and replacement walks can
    /// also produce this shortfall when their pool or walk deadline is
    /// exhausted.
    #[error(
        "selected only {selected} of {requested} FMan seats ({seen} advertisements seen, \
         {eligible} eligible)"
    )]
    InsufficientFmanSeats {
        /// Requested federation size.
        requested: u16,
        /// Seats the walk verified-filled before running out.
        selected: u16,
        /// Total advertisements the bounded enumeration observed.
        seen: usize,
        /// Statically admitted, currently eligible candidates.
        eligible: usize,
    },
    /// Selection preview did not complete strictly before its configured
    /// absolute deadline; expiry wins simultaneous readiness.
    #[error("FMan selection preview timed out")]
    SelectionPreviewTimeout,
    /// The selected seats' advertised setup-price estimate cannot be
    /// represented in millisatoshis.
    #[error("aggregate advertised FMan setup-price estimate overflowed")]
    SelectionEstimateOverflow,
    /// The preview, selected set, exact price, or payer changed before the
    /// wallet output-generation boundary.
    #[error("fresh selection authorization required: {reason}")]
    SelectionReauthorizationRequired {
        reason: SelectionReauthorizationReason,
        failure: Option<FmanFailure>,
    },
    /// A later-stage capability is unavailable.
    #[error("FI capability unavailable: {0:?}")]
    CapabilityUnavailable(Capability),
    /// Pinned locator set failed local validation.
    #[error("invalid Fleet Manager set: {0}")]
    InvalidFleetManagers(String),
    /// A local verdict about a seat; no remote request is attributed.
    #[error("Fleet Manager {index} failure: {message}")]
    FleetManager { index: u16, message: String },
    /// Consumer wallet operation failed.
    #[error("FI payment failure: {0}")]
    Payment(String),
    /// Post-formation FLIP operation failed without exposing private payloads.
    #[error("FI liquidity failure: {0}")]
    Liquidity(String),
    /// A non-terminal liquidity operation already covers this federation.
    /// Providers hold at most one allocation per federation, so minting a
    /// second live request identity risks double-accepted liquidity; resume
    /// the named operation instead.
    /// [`crate::FiClient::current_liquidity_operation`] returns its snapshot
    /// without paging.
    #[error(
        "liquidity operation {} for this federation is still live; fetch it with \
         current_liquidity_operation and resume it instead of starting a new request",
        .operation_id.0
    )]
    LiquidityOperationExists {
        /// Semantic id of the live operation to resume.
        operation_id: crate::LiquidityOperationId,
    },
    /// A post-formation maintenance verb was attempted in another phase.
    #[error("federation maintenance requires formed state, found {phase:?}")]
    MaintenanceWrongState {
        /// Durable phase observed before any maintenance network effect.
        phase: crate::FormationPhase,
    },
    /// One guardian returned a terminal typed refusal for this maintenance
    /// request. Retryable unavailability and stale-base responses never use
    /// this variant.
    #[error("Fleet Manager {index} terminally rejected federation maintenance: {reason}")]
    MaintenanceRejected {
        /// Stable federation seat index.
        index: u16,
        /// Exact typed FMan protocol refusal.
        reason: fedi_decentralized_service_fleet_manager::FleetManagerError,
    },
    /// A real consensus read returned a metadata object too large to hash,
    /// parse, clone, or fan out under Manifold's bounded maintenance policy.
    #[error(
        "federation consensus metadata is {actual_bytes} bytes; maintenance permits at most \
         {max_bytes} bytes"
    )]
    MaintenanceConsensusTooLarge {
        /// Raw object length returned by the consensus reader.
        actual_bytes: usize,
        /// Shared FI/FMan whole-object ceiling.
        max_bytes: usize,
    },
    /// A real consensus read returned metadata that cannot support a typed
    /// field update.
    #[error("federation consensus metadata is invalid: {reason}")]
    MaintenanceConsensusInvalid {
        /// Sanitized structural reason; never includes raw metadata bytes.
        reason: String,
    },
    /// The bounded maintenance run ended without fresh consensus readback.
    #[error(
        "federation maintenance did not converge; unresolved guardians: {unresolved:?}; \
         last guardian errors: {guardian_errors:?}; last consensus error: {consensus_error:?}"
    )]
    MaintenanceConvergence {
        /// Seats that never acknowledged the current consensus-base mutation.
        unresolved: Vec<u16>,
        /// Last sanitized retryable transport/service failure per unresolved seat.
        guardian_errors: Vec<(u16, String)>,
        /// Last sanitized consumer consensus-read failure, when one occurred.
        consensus_error: Option<String>,
    },
    /// Formation polling reached its deadline.
    #[error("formation timed out while {0}")]
    Timeout(String),
}

impl FiError {
    /// Classify automatic retries at the source, without parsing error text.
    /// Unknown transport, wallet and storage failures remain retryable.
    #[must_use]
    pub fn disposition(&self) -> FailureDisposition {
        match self {
            Self::Fman(failure) => match failure.cause {
                FmanCause::InvalidResponse
                | FmanCause::FormationMetaAlreadyPublished
                | FmanCause::OfferChanged
                | FmanCause::TerminalStatus(_) => FailureDisposition::Terminal,
                _ => FailureDisposition::Retryable,
            },
            Self::Storage(_)
            | Self::Busy
            | Self::Registry(_)
            | Self::FleetManager { .. }
            | Self::Payment(_)
            | Self::Liquidity(_)
            | Self::MaintenanceConvergence { .. }
            | Self::Timeout(_) => FailureDisposition::Retryable,
            Self::InvalidIntent(_)
            | Self::InvalidOptions(_)
            | Self::StorageInvariant(_)
            | Self::Identity(_)
            | Self::NoActiveFormation
            | Self::AbandonUnavailable(_)
            | Self::Selection(_)
            | Self::InsufficientFmanSeats { .. }
            | Self::SelectionPreviewTimeout
            | Self::SelectionEstimateOverflow
            | Self::SelectionReauthorizationRequired { .. }
            | Self::CapabilityUnavailable(_)
            | Self::InvalidFleetManagers(_)
            | Self::LiquidityOperationExists { .. }
            | Self::MaintenanceWrongState { .. }
            | Self::MaintenanceRejected { .. }
            | Self::MaintenanceConsensusTooLarge { .. }
            | Self::MaintenanceConsensusInvalid { .. } => FailureDisposition::Terminal,
        }
    }

    /// Return the stable error category used by progress surfaces.
    #[must_use]
    pub fn code(&self) -> FiErrorCode {
        match self {
            Self::Fman(failure) => match failure.cause {
                FmanCause::Timeout => FiErrorCode::Timeout,
                FmanCause::InvalidResponse | FmanCause::FormationMetaAlreadyPublished => {
                    FiErrorCode::InvalidFleetManagers
                }
                _ => FiErrorCode::FleetManager,
            },
            Self::InvalidIntent(_) => FiErrorCode::InvalidIntent,
            Self::InvalidOptions(_) => FiErrorCode::InvalidOptions,
            Self::Storage(_) | Self::StorageInvariant(_) => FiErrorCode::Storage,
            Self::Identity(_) => FiErrorCode::Identity,
            Self::Busy => FiErrorCode::Busy,
            Self::NoActiveFormation => FiErrorCode::NoActiveFormation,
            Self::AbandonUnavailable(_) => FiErrorCode::AbandonUnavailable,
            Self::Registry(_) => FiErrorCode::Registry,
            Self::Selection(_)
            | Self::InsufficientFmanSeats { .. }
            | Self::SelectionEstimateOverflow => FiErrorCode::Selection,
            Self::SelectionPreviewTimeout => FiErrorCode::Timeout,
            Self::SelectionReauthorizationRequired { .. } => {
                FiErrorCode::SelectionReauthorizationRequired
            }
            Self::CapabilityUnavailable(_) => FiErrorCode::CapabilityUnavailable,
            Self::InvalidFleetManagers(_) => FiErrorCode::InvalidFleetManagers,
            Self::FleetManager { .. } => FiErrorCode::FleetManager,
            Self::Payment(_) => FiErrorCode::Payment,
            Self::Liquidity(_) | Self::LiquidityOperationExists { .. } => FiErrorCode::Liquidity,
            Self::MaintenanceWrongState { .. } => FiErrorCode::MaintenanceWrongState,
            Self::MaintenanceRejected { .. } => FiErrorCode::MaintenanceRejected,
            Self::MaintenanceConsensusTooLarge { .. } => FiErrorCode::MaintenanceConsensusTooLarge,
            Self::MaintenanceConsensusInvalid { .. } => FiErrorCode::MaintenanceConsensusInvalid,
            Self::MaintenanceConvergence { .. } => FiErrorCode::MaintenanceConvergence,
            Self::Timeout(_) => FiErrorCode::Timeout,
        }
    }
}
