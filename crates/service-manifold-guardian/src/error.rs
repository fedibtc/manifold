//! Manifold Guardian protocol error vocabulary.

use crate::ServiceStatus;

/// Typed errors the Manifold Guardian returns to FA and public MG API callers.
///
/// The canonical wire form is still being finalized (the protocol doc
/// currently lists only `No can do for {reason}`); this enumerates the cases
/// the design references so the reference trait is actionable. `Other` carries
/// the human-readable fallback reason.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ManifoldGuardianError {
    /// The requested plan does not exactly match a current offer.
    #[error("plan not offered")]
    PlanNotOffered,

    /// The quote selected a payment federation this MG does not accept.
    #[error("payment federation not accepted")]
    PaymentFederationNotAccepted,

    /// The federation is accepted, but this MG's payment client is not ready.
    #[error("payment federation temporarily unavailable, retry")]
    PaymentFederationUnavailable,

    /// Quote or payment evidence failed offline validation.
    #[error("invalid payment")]
    InvalidPayment,

    /// No seat can be quoted at the current offer epoch.
    #[error("capacity exhausted")]
    CapacityExhausted,

    /// Seat id is missing or belongs to another FA.
    #[error("unknown seat")]
    UnknownSeat,
    /// The requested `fedimintd` version is not in the offered set.
    #[error("unsupported fedimintd version")]
    UnsupportedVersion,

    /// The requested federation size is not in the advertised set.
    #[error("unsupported federation size")]
    UnsupportedFederationSize,

    /// The [`crate::SignedRequest`] envelope failed verification: unparsable
    /// payload, malformed FA key or signature, bad signature, or stale
    /// timestamp. Deliberately coarse — the detail is daemon log material
    /// (see [`crate::AuthError`]), not an oracle for the wire.
    #[error("unauthorized")]
    Unauthorized,

    /// The verb is not valid in the seat's current lifecycle state; carries
    /// the current wire status so the FA resynchronizes from `GetStatus`
    /// instead of guessing (SPEC-seat-lifecycle).
    #[error("wrong state for this verb (currently {status})")]
    WrongState {
        /// The seat's current canonical wire status.
        status: ServiceStatus,
    },

    /// The seat's `fedimintd` child is not currently serving (spawn failure,
    /// crash backoff, or unresponsive); retryable (SPEC-seat-lifecycle).
    #[error("seat temporarily unavailable, retry")]
    SeatUnavailable,

    /// The source of a guardian transfer has asked its guardian to stop once
    /// the current consensus session completes, and it has not stopped yet.
    /// The destination retries `TransferSeat` (SPEC-guardian-transfer).
    #[error("guardian stops after its current session, retry")]
    AwaitingSessionEnd,

    /// The source's guardian has stopped, but the relays do not yet confirm
    /// a backup that records the stop. The destination retries `TransferSeat`
    /// (SPEC-guardian-transfer).
    #[error("guardian transfer awaits its backup, retry")]
    AwaitingBackup,

    /// Formation operations cannot replace a seat whose federation is formed.
    #[error("federation is running")]
    FederationIsRunning,

    /// A DKG input failed deterministic-code or `StartDkg` code-set validation. Carries the human-readable reason.
    #[error("invalid DKG input: {0}")]
    InvalidDkgInput(String),

    /// A `SetMetaField` key is not in the active allowlist.
    #[error("meta key refused")]
    MetaKeyRefused,

    /// A `SetMetaField` value failed its typed validator.
    #[error("meta value invalid")]
    MetaValueInvalid,

    /// This MG has no deployment-pinned Guardian Verification Fee account
    /// with which to validate and derive formation recipients.
    #[error("Guardian Verification Fee account unavailable")]
    GuardianVerificationFeeAccountUnavailable,

    /// The request's Guardian Verification Fee account differs from this
    /// MG's deployment-pinned configuration.
    #[error(
        "Guardian Verification Fee account does not match this Manifold Guardian's configuration"
    )]
    GuardianVerificationFeeAccountMismatch,

    /// A metadata mutation request was based on an older consensus meta object.
    #[error("meta consensus changed, reread and retry")]
    MetaConsensusChanged,

    /// Formation metadata was already adopted and is immutable.
    #[error("formation metadata already published")]
    FormationMetaAlreadyPublished,

    /// A metadata mutation named a consensus base this guardian already
    /// admitted for a different whole-object target. Not clearable by reread:
    /// the base stays pinned until consensus moves or the guardian process
    /// restarts, so retrying the same mutation there cannot succeed.
    #[error("meta target conflict, base pinned to a different admitted value")]
    MetaTargetConflict,

    /// A gateway registration did not carry a syntactically valid API URL.
    #[error("invalid gateway API URL")]
    InvalidGatewayApiUrl,

    /// A seat transfer grant does not name this seat, this MG, or the
    /// seat's federation.
    #[error("seat transfer grant does not match this seat")]
    SeatTransferGrantMismatch,

    /// The destination MG already hosts a guardian of the grant's federation.
    #[error("this Manifold Guardian already hosts a guardian of that federation")]
    FederationAlreadyHosted,

    /// The verb exists in the protocol crate but is intentionally not
    /// implemented by this daemon/profile version.
    #[error("{verb} unsupported in this Manifold Guardian profile")]
    UnsupportedVerb {
        /// Protocol verb name.
        verb: String,
    },

    /// Human-readable fallback ("No can do for {reason}").
    #[error("no can do for {0}")]
    Other(String),
}

impl From<fedi_iroh_rpc::RpcError> for ManifoldGuardianError {
    fn from(_error: fedi_iroh_rpc::RpcError) -> Self {
        // The generated trait client requires this compatibility conversion.
        // Consumers that classify transport failures must instead retain the
        // low-level outer `RpcResult` in their local connector adapter.
        Self::Other("local RPC call failed".to_owned())
    }
}

#[cfg(test)]
mod tests;
