//! DKG messages.

use crate::{FederationName, FiId, GuardianCode, SeatId, Timestamp};

const DKG_COMPLETION_CALLBACK_URL_MAX_BYTES: usize = 2_048;
const DKG_COMPLETION_IDEMPOTENCY_KEY_MAX_BYTES: usize = 128;

/// Bearer callback an FMan invokes after this DKG attempt is durably usable.
///
/// The URL embeds a push-gateway hook secret. Its fields are private and its
/// debug representation is deliberately redacted so signed requests, durable
/// recovery structs, and errors cannot accidentally disclose the capability.
#[derive(Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "UncheckedDkgCompletionCallback")]
pub struct DkgCompletionCallback {
    callback_url: String,
    idempotency_key: String,
}

/// Named construction fields for a DKG completion bearer.
pub struct DkgCompletionCallbackInput {
    /// Exact push-gateway hook URL, including its bearer path.
    pub callback_url: String,
    /// Stable key used to deduplicate callback retries.
    pub idempotency_key: String,
}

impl DkgCompletionCallback {
    /// Construct a bounded callback. The receiving FMan separately validates
    /// that the URL is an exact hook path under its configured gateway origin.
    pub fn new(input: DkgCompletionCallbackInput) -> Result<Self, InvalidDkgCompletionCallback> {
        let DkgCompletionCallbackInput {
            callback_url,
            idempotency_key,
        } = input;
        if callback_url.is_empty()
            || DKG_COMPLETION_CALLBACK_URL_MAX_BYTES < callback_url.len()
            || callback_url.chars().any(char::is_control)
        {
            return Err(InvalidDkgCompletionCallback(
                "callback URL must be 1..=2048 bytes with no control characters",
            ));
        }
        if idempotency_key.is_empty()
            || DKG_COMPLETION_IDEMPOTENCY_KEY_MAX_BYTES < idempotency_key.len()
            || idempotency_key.chars().any(char::is_control)
        {
            return Err(InvalidDkgCompletionCallback(
                "idempotency key must be 1..=128 bytes with no control characters",
            ));
        }
        Ok(Self {
            callback_url,
            idempotency_key,
        })
    }

    /// Return the bearer URL to the narrow invocation boundary.
    #[must_use]
    pub fn callback_url(&self) -> &str {
        &self.callback_url
    }

    /// Return the gateway-scoped idempotency key.
    #[must_use]
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }
}

impl std::fmt::Debug for DkgCompletionCallback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DkgCompletionCallback")
            .field("callback_url", &"<redacted>")
            .field("idempotency_key", &"<redacted>")
            .finish()
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedDkgCompletionCallback {
    callback_url: String,
    idempotency_key: String,
}

impl TryFrom<UncheckedDkgCompletionCallback> for DkgCompletionCallback {
    type Error = InvalidDkgCompletionCallback;

    fn try_from(value: UncheckedDkgCompletionCallback) -> Result<Self, Self::Error> {
        Self::new(DkgCompletionCallbackInput {
            callback_url: value.callback_url,
            idempotency_key: value.idempotency_key,
        })
    }
}

/// A callback field failed its protocol-level size or text bounds.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid DKG completion callback: {0}")]
pub struct InvalidDkgCompletionCallback(&'static str);

/// Ceiling on a DKG display name's UTF-8 length, in bytes.
pub const DKG_NAME_MAX_BYTES: usize = 128;

/// A DKG display name failed [`validate_dkg_display_name`].
#[derive(Debug, thiserror::Error)]
#[error("must be 1..=128 UTF-8 bytes, contain visible text, and have no control characters")]
pub struct InvalidDkgDisplayName;

/// The one display-name rule for DKG guardian and federation names
/// (SPEC-fi-rpc *Boundary validation and policy*).
///
/// A check rather than a validating constructor, deliberately: the wire
/// wrappers stay permissive so a malformed *authenticated* request is
/// answered with the typed `InvalidDkgInput` policy error instead of failing
/// envelope payload decoding. One definition serves both sides — the FMan
/// enforces it at its RPC boundary, and an FI can pre-check before signing.
pub fn validate_dkg_display_name(value: &str) -> Result<(), InvalidDkgDisplayName> {
    if value.is_empty()
        || value.len() > DKG_NAME_MAX_BYTES
        || value.trim().is_empty()
        || value.chars().any(char::is_control)
    {
        return Err(InvalidDkgDisplayName);
    }
    Ok(())
}

/// Request to get this guardian's DKG code.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetDkgCodeRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity.
    pub fi_id: FiId,

    /// Seat used by this guardian.
    pub seat_id: SeatId,

    /// Leader-only federation display name.
    pub federation_name: Option<FederationName>,
}

/// Response containing this guardian's DKG code.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetDkgCodeResponse {
    /// Guardian code to exchange with other guardians.
    pub guardian_code: GuardianCode,
}

/// Request to start DKG after collecting all guardian codes.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StartDkgRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity.
    pub fi_id: FiId,

    /// Seat used by this guardian.
    pub seat_id: SeatId,

    /// Guardian codes of all guardians.
    pub guardian_codes: Vec<GuardianCode>,

    /// Optional installation-scoped push-gateway callback for the durable
    /// DKG/Wallet Service completion event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_callback: Option<DkgCompletionCallback>,
}

/// Acknowledgement that DKG was started (or, for a retry with the same code
/// set, already running). Deliberately empty: failure travels as a typed
/// error, so a success flag would be an always-true boolean inviting clients
/// to branch on it. `started: bool` would always be true and is therefore omitted.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct StartDkgResponse;

/// Request to replace the current child and start DKG on its fresh session.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RestartDkgRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity.
    pub fi_id: FiId,

    /// Seat used by this guardian.
    pub seat_id: SeatId,

    /// Guardian codes of all guardians.
    pub guardian_codes: Vec<GuardianCode>,
}

/// Status after replacing the child and either starting a fresh ceremony or
/// observing that the prior ceremony completed first.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct RestartDkgResponse {
    /// Observed seat status after replacing the child or finding it configured.
    pub status: crate::ServiceStatus,
}

#[cfg(test)]
#[path = "dkg/tests.rs"]
mod tests;
