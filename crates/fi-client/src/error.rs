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
    /// The federation already formed; the durable record now holds the
    /// invite deliverable.
    AlreadyFormed,
}

impl std::fmt::Display for AbandonUnavailableReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PaymentOutputsStarted => {
                "payment output generation started; exact recovery must finish before teardown"
            }
            Self::AlreadyFormed => "the federation is already formed",
        })
    }
}

/// Typed reason a Pay-and-create attempt must return to fresh user approval.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionReauthorizationReason {
    /// The advertisement preview's two-minute approval window elapsed.
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
    /// A Fleet Manager transport or protocol operation failed.
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

/// Error returned by FI client operations.
#[derive(Debug, thiserror::Error)]
pub enum FiError {
    /// Consumer intent failed validation.
    #[error("invalid formation intent: {0}")]
    InvalidIntent(String),
    /// Formation run options failed validation.
    #[error(transparent)]
    InvalidOptions(#[from] crate::InvalidFormationRunOptions),
    /// Durable state could not be read or written.
    #[error("FI storage failure: {0}")]
    Storage(String),
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
    #[error("fresh selection authorization required: {0}")]
    SelectionReauthorizationRequired(SelectionReauthorizationReason),
    /// A later-stage capability is unavailable.
    #[error("FI capability unavailable: {0:?}")]
    CapabilityUnavailable(Capability),
    /// Pinned locator set failed local validation.
    #[error("invalid Fleet Manager set: {0}")]
    InvalidFleetManagers(String),
    /// A Fleet Manager transport or protocol operation failed.
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
    /// A paid or free seat presentation was refused.
    #[error("Fleet Manager {index} refused seat: {reason}")]
    SeatRefused { index: u16, reason: String },
    /// Formation polling reached its deadline.
    #[error("formation timed out while {0}")]
    Timeout(String),
}

impl FiError {
    /// Return the stable error category used by progress surfaces.
    #[must_use]
    pub fn code(&self) -> FiErrorCode {
        match self {
            Self::InvalidIntent(_) => FiErrorCode::InvalidIntent,
            Self::InvalidOptions(_) => FiErrorCode::InvalidOptions,
            Self::Storage(_) => FiErrorCode::Storage,
            Self::Identity(_) => FiErrorCode::Identity,
            Self::Busy => FiErrorCode::Busy,
            Self::NoActiveFormation => FiErrorCode::NoActiveFormation,
            Self::AbandonUnavailable(_) => FiErrorCode::AbandonUnavailable,
            Self::Registry(_) => FiErrorCode::Registry,
            Self::Selection(_)
            | Self::InsufficientFmanSeats { .. }
            | Self::SelectionEstimateOverflow => FiErrorCode::Selection,
            Self::SelectionPreviewTimeout => FiErrorCode::Timeout,
            Self::SelectionReauthorizationRequired(_) => {
                FiErrorCode::SelectionReauthorizationRequired
            }
            Self::CapabilityUnavailable(_) => FiErrorCode::CapabilityUnavailable,
            Self::InvalidFleetManagers(_) => FiErrorCode::InvalidFleetManagers,
            Self::FleetManager { .. } | Self::SeatRefused { .. } => FiErrorCode::FleetManager,
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
