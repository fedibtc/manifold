//! Status and federation metadata messages.

#[cfg(test)]
mod tests;

use std::fmt;

use fedi_decentralized_services::domain::{
    FMAN_SEAT_BINDINGS_MAX_COUNT, FmanPeerAttestation, SeatEndpointProof,
};
use serde::de::{IgnoredAny, SeqAccess, Visitor};
use stability_pool_common::{Account, AccountType};

use crate::{
    FedimintStats, FiId, GatewayApiUrl, GuardianCode, InviteCode, MetaConsensusBase, MetaFieldKey,
    MetaFieldValue, SeatHealth, SeatId, Timestamp, ValidUntilDate,
};

/// Request for DKG or running federation status.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetStatusRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity.
    pub fi_id: FiId,

    /// Seat being queried.
    pub seat_id: SeatId,
}

/// Overall DKG or federation status.
///
/// `Display` emits the canonical cross-program wire status strings
/// (SPEC-fi-rpc boundary), not the Rust variant names.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq, strum::Display)]
pub enum ServiceStatus {
    /// Seat has no durable formation and no live ceremony.
    #[strum(serialize = "new")]
    New,

    /// DKG is in process.
    #[strum(serialize = "DKG in process")]
    DkgInProcess,

    /// A formed record exists but the guardian's final data directory is absent.
    #[strum(serialize = "guardian data loss")]
    DataLoss,

    /// Federation is formed.
    #[strum(serialize = "running")]
    Running,

    /// Seat was decommissioned by the operator (an `InfiniteBestEffort` seat
    /// never expires, so only the operator ends it).
    #[strum(serialize = "decommissioned")]
    Decommissioned,
}

/// Extra status data specific to the current status. The FMan projects its
/// finer-grained lifecycle onto the canonical wire status and carries
/// the status-specific info here.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub enum StatusDetail {
    /// Guardian code generated.
    GuardianCode(GuardianCode),

    /// DKG-specific status data.
    Dkg(DkgStatusInfo),

    /// Running or in-grace federation status data.
    Federation(FederationStatusInfo),

    /// Suspended for non-payment.
    Suspended(SuspendedStatusInfo),

    /// Deleted for non-payment, or operator-decommissioned.
    Ended(EndedStatusInfo),

    /// No additional detail is available yet.
    None,
}

/// DKG-specific status information.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct DkgStatusInfo {
    /// Peer connection information.
    pub peer_connections: Vec<PeerConnectionInfo>,
}

/// Information about a peer connection during DKG.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct PeerConnectionInfo {
    /// Peer identifier, provisional until the domain type exists.
    pub peer_id: String,

    /// Whether this peer is connected.
    pub connected: bool,

    /// Latency in milliseconds, if known.
    pub latency_ms: Option<u64>,
}

/// Running (or in-grace) federation status information.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct FederationStatusInfo {
    /// Date paid through.
    pub valid_until_date: Option<ValidUntilDate>,

    /// Federation invite code, once running.
    pub invite_code: Option<InviteCode>,

    /// Guardian peer connections and latencies.
    pub peer_connections: Vec<PeerConnectionInfo>,

    /// Whether the seat is in the post-`valid_until` grace window.
    pub in_grace: bool,

    /// Grace deadline, when `in_grace`.
    pub grace_deadline: Option<Timestamp>,

    /// This seat's `fedimintd` stats.
    pub stats: Option<FedimintStats>,
}

/// Status info for a suspended-for-non-payment seat.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct SuspendedStatusInfo {
    /// When the seat was suspended.
    pub suspended_since: Timestamp,

    /// Deadline after which a suspended seat is deleted.
    pub retention_deadline: Timestamp,
}

/// Why a seat reached a terminal state.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq, strum::Display)]
pub enum EndedReason {
    /// Deleted after the non-payment retention window elapsed.
    DeletedForNoPayment,

    /// Stopped by the operator.
    Decommissioned,
}

/// Status info for a terminal (deleted or decommissioned) seat.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct EndedStatusInfo {
    /// Why the seat ended.
    pub reason: EndedReason,

    /// When it ended.
    pub at: Timestamp,

    /// Optional operator note (e.g. decommission reason).
    pub note: Option<String>,
}

/// Response containing DKG or running federation status.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetStatusResponse {
    /// Overall status.
    pub status: ServiceStatus,

    /// Additional status-specific information.
    pub detail: StatusDetail,

    /// Seat-health marker: not `Healthy` while the seat's `fedimintd` is not
    /// serving, so the FI can tell "retry shortly" from "stuck". `None` where
    /// no child should exist: pre-`Confirmed`, `Suspended` (intentionally
    /// stopped), and terminal states.
    pub seat_health: Option<SeatHealth>,
}

/// Request for a running seat's federation invite code.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetInviteCodeRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity.
    pub fi_id: FiId,

    /// Seat being queried.
    pub seat_id: SeatId,
}

/// Response containing the federation invite code.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetInviteCodeResponse {
    /// Federation invite code. Available once the seat is `Running`.
    pub invite_code: InviteCode,
}

/// Request for the running seat's FMan peer attestation.
///
/// This is FI/seat-scoped maintenance data. External verifiers and FLIP read
/// the authoritative bindings out of the `fedi:fman_seat_bindings` consensus
/// metadata the FI assembles from these responses, never from the FI's own
/// word ([`SPEC-federation-trust-directory`](../../domain/specs/SPEC-federation-trust-directory.md)).
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetPeerAttestationRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity.
    pub fi_id: FiId,

    /// Running seat being attested.
    pub seat_id: SeatId,
}

/// Response containing the running seat's FMan peer attestation.
///
/// The returned signed object can be stored for FI recovery or diagnostics,
/// and is the input the FI assembles the directory from, but it is not itself
/// the federation-eligibility trust-material source for FLIP.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetPeerAttestationResponse {
    /// FMan-signed binding between this service identity and its Fedimint peer.
    pub fman_peer_attestation: FmanPeerAttestation,

    /// Endpoint-key endorsement used when admitting the binding to consensus.
    pub seat_endpoint_proof: SeatEndpointProof,
}

/// Request to set a federation metadata field.
///
/// The FMan serves only keys in its compiled validator set, so this is a
/// proposal rather than a write. `fedi:fman_seat_bindings` has cross-component
/// semantics: after DKG the FI must submit the exact same canonical directory
/// through every running seat, because the value becomes consensus only once
/// threshold guardians have submitted byte-identical bytes
/// ([`SPEC-federation-trust-directory`](../../domain/specs/SPEC-federation-trust-directory.md)).
/// All whole-object mutations use the guarded merge/rebase protocol in
/// [`SPEC-fi-metadata-maintenance`](../../fman/specs/SPEC-fi-metadata-maintenance.md).
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct SetMetaFieldRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity.
    pub fi_id: FiId,

    /// Seat being modified.
    pub seat_id: SeatId,

    /// Exact consensus occurrence on which this read-modify-write is based:
    /// the meta module's monotone revision together with the raw object.
    ///
    /// A stale request is refused rather than merged into a newer object. The
    /// FI must reread consensus, rebase the same typed mutation, and retry.
    /// Byte-identical content readopted later carries a fresh revision and so
    /// a fresh base; a request bound to the earlier occurrence stays stale.
    pub expected_base: MetaConsensusBase,

    /// Metadata field key.
    pub key: MetaFieldKey,

    /// Metadata field value.
    pub value: MetaFieldValue,
}

/// Response to a metadata update request. Success acknowledges one guardian
/// vote submission, not threshold consensus adoption; callers must read the
/// live consensus value back.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct SetMetaFieldResponse;

/// Ask one guardian to store a gateway in its local LNv2 module.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RegisterGatewayRequest {
    /// Envelope timestamp.
    pub ts: Timestamp,

    /// Federation Initiator identity bound to the seat.
    pub fi_id: FiId,

    /// FMan seat whose guardian should register the gateway.
    pub seat_id: SeatId,

    /// Client-reachable LNv2 gateway API URL.
    pub gateway_api: GatewayApiUrl,
}

/// Result of registering one gateway with one guardian.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RegisterGatewayResponse {
    /// Whether this call inserted a new URL; false means it was already present.
    pub was_added: bool,
}

/// Full payer-compatible account for one guardian-fee recipient.
///
/// The wrapper makes the fee-account domain explicit at the protocol boundary
/// while preserving the stability-pool module's exact JSON shape. Construction
/// and deserialization reject every account except a single-signature
/// `BtcDepositor` account.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(try_from = "Account", into = "Account")]
pub struct GuardianFeeAccount(Account);

impl GuardianFeeAccount {
    /// Return the canonical stability-pool account identifier.
    #[must_use]
    pub fn account_id(&self) -> String {
        self.0.id().to_string()
    }

    /// Borrow the validated full stability-pool account.
    #[must_use]
    pub fn as_account(&self) -> &Account {
        &self.0
    }

    /// Consume the protocol wrapper at the FMan policy boundary.
    #[must_use]
    pub fn into_account(self) -> Account {
        self.0
    }
}

impl TryFrom<Account> for GuardianFeeAccount {
    type Error = GuardianFeeAccountError;

    fn try_from(account: Account) -> Result<Self, Self::Error> {
        if account.acc_type() != AccountType::BtcDepositor || account.as_single().is_none() {
            return Err(GuardianFeeAccountError);
        }
        Ok(Self(account))
    }
}

impl From<GuardianFeeAccount> for Account {
    fn from(account: GuardianFeeAccount) -> Self {
        account.0
    }
}

/// A fee recipient account was not a single-signature `BtcDepositor` account.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("guardian-fee account must be a single-signature BtcDepositor account")]
pub struct GuardianFeeAccountError;

/// One recipient of the federation's guardian-fee remittances.
///
/// The full account and repeated `account_id` exactly match the versioned
/// metadata contract. The repetition is validated rather than trusted. The
/// FMan chooses the metadata version; it is not FI-selectable.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GuardianFeeRecipient {
    /// Complete validated account descriptor.
    pub account: GuardianFeeAccount,

    /// Stability-pool `BtcDepositor` account identifier.
    pub account_id: String,

    /// This account's share of guardian-fee remittances.
    pub weight: u64,
}

impl GuardianFeeRecipient {
    /// Construct the only self-consistent wire entry for an account.
    #[must_use]
    pub fn new(account: GuardianFeeAccount, weight: u64) -> Self {
        Self {
            account_id: account.account_id(),
            account,
            weight,
        }
    }
}

/// Maximum weighted-recipient list length accepted by the payer.
pub const MAX_GUARDIAN_FEE_RECIPIENTS: usize = 32;
/// Maximum canonical bytes for one validated full account.
pub const MAX_GUARDIAN_FEE_ACCOUNT_BYTES: usize = 256;
/// Maximum canonical bytes for the complete versioned recipient value.
pub const MAX_GUARDIAN_FEE_RECIPIENT_LIST_BYTES: usize = 8_192;
/// Metadata key holding the guardian-fee send rate.
pub const GUARDIAN_FEE_SEND_PPM_META_FIELD_KEY: &str = "fedi:guardian_fee_send_ppm";
/// Metadata key holding the fixed weighted recipient list.
pub const GUARDIAN_FEE_RECIPIENTS_META_FIELD_KEY: &str = "fedi:guardian_fee_remittance_account";

/// Fixed FI share in the Manifold MVP policy.
pub const FI_GUARDIAN_FEE_WEIGHT: u64 = 4;
/// Fixed share for each accepted guardian.
pub const GUARDIAN_GUARDIAN_FEE_WEIGHT: u64 = 1;
/// Fixed weight for the Guardian Verification Fee.
pub const GUARDIAN_VERIFICATION_FEE_WEIGHT: u64 = 1;

#[derive(serde::Serialize)]
struct GuardianFeeRecipientList<'a> {
    version: u16,
    recipients: &'a [GuardianFeeRecipient],
}

/// Why a recipient vector cannot be encoded into the version-1 metadata shape.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GuardianFeeRecipientListError {
    #[error("guardian-fee recipient count must be between 1 and 32")]
    InvalidCount,
    #[error("guardian-fee recipient account exceeds the canonical byte cap")]
    AccountTooLarge,
    #[error("guardian-fee recipient account id does not match its full account")]
    AccountIdMismatch,
    #[error("guardian-fee recipient weight must be positive and the total must not overflow")]
    InvalidWeight,
    #[error("guardian-fee recipients must be unique and strictly sorted by account id")]
    NotCanonical,
    #[error("guardian-fee recipient metadata could not be canonicalized")]
    Encoding,
    #[error("guardian-fee recipient metadata exceeds the canonical byte cap")]
    ListTooLarge,
}

/// Validate and encode the exact weighted-recipient metadata contract.
pub fn canonical_guardian_fee_recipient_list(
    recipients: &[GuardianFeeRecipient],
) -> Result<String, GuardianFeeRecipientListError> {
    if recipients.is_empty() || recipients.len() > MAX_GUARDIAN_FEE_RECIPIENTS {
        return Err(GuardianFeeRecipientListError::InvalidCount);
    }
    let mut previous = None;
    let mut total = 0_u64;
    for recipient in recipients {
        let account_bytes = serde_json_canonicalizer::to_vec(recipient.account.as_account())
            .map_err(|_| GuardianFeeRecipientListError::Encoding)?;
        if account_bytes.len() > MAX_GUARDIAN_FEE_ACCOUNT_BYTES {
            return Err(GuardianFeeRecipientListError::AccountTooLarge);
        }
        let id = recipient.account.as_account().id();
        if recipient.account_id != id.to_string() {
            return Err(GuardianFeeRecipientListError::AccountIdMismatch);
        }
        if previous.as_ref().is_some_and(|previous| previous >= &id) {
            return Err(GuardianFeeRecipientListError::NotCanonical);
        }
        previous = Some(id);
        total = total
            .checked_add(recipient.weight)
            .filter(|_| recipient.weight != 0)
            .ok_or(GuardianFeeRecipientListError::InvalidWeight)?;
    }
    if total == 0 {
        return Err(GuardianFeeRecipientListError::InvalidWeight);
    }
    let bytes = serde_json_canonicalizer::to_vec(&GuardianFeeRecipientList {
        version: 1,
        recipients,
    })
    .map_err(|_| GuardianFeeRecipientListError::Encoding)?;
    if bytes.len() > MAX_GUARDIAN_FEE_RECIPIENT_LIST_BYTES {
        return Err(GuardianFeeRecipientListError::ListTooLarge);
    }
    String::from_utf8(bytes).map_err(|_| GuardianFeeRecipientListError::Encoding)
}

/// FI proposal for the immutable formation metadata bundle.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct ProposeFormationMetaRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity.
    pub fi_id: FiId,

    /// Seat whose guardian casts this vote.
    pub seat_id: SeatId,

    /// Exact consensus object on which this formation write is based.
    pub expected_base: MetaConsensusBase,

    /// Attestations paired with their API-endpoint-key endorsements. Each FMan
    /// independently constructs the canonical seat-binding directory.
    #[serde(deserialize_with = "deserialize_formation_seat_bindings")]
    pub seat_bindings: Vec<FormationSeatBinding>,

    /// Residual FI recipient account for the fixed fee split.
    pub fi_fee_account: GuardianFeeAccount,

    /// Deployment-pinned Guardian Verification Fee account every FMan must
    /// match before voting.
    /// The FI states the expected configuration; it does not choose the account.
    pub guardian_verification_fee_account: GuardianFeeAccount,

    /// Guardian-fee send rate in parts per million.
    pub send_ppm: u64,
}

/// One prospective federation peer's signed identity binding and endpoint-key
/// endorsement.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct FormationSeatBinding {
    /// FMan-signed identity and fee-account binding for this prospective peer.
    pub attestation: FmanPeerAttestation,
    /// Proof that the final-config endpoint key endorses the attestation.
    pub endpoint_proof: SeatEndpointProof,
}

fn deserialize_formation_seat_bindings<'de, D>(
    deserializer: D,
) -> Result<Vec<FormationSeatBinding>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct BoundedSeatBindingsVisitor;

    impl<'de> Visitor<'de> for BoundedSeatBindingsVisitor {
        type Value = Vec<FormationSeatBinding>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "at most {FMAN_SEAT_BINDINGS_MAX_COUNT} formation seat bindings"
            )
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut bindings = Vec::with_capacity(
                seq.size_hint()
                    .unwrap_or_default()
                    .min(FMAN_SEAT_BINDINGS_MAX_COUNT),
            );
            while bindings.len() < FMAN_SEAT_BINDINGS_MAX_COUNT {
                let Some(binding) = seq.next_element()? else {
                    return Ok(bindings);
                };
                bindings.push(binding);
            }
            if seq.next_element::<IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom(format_args!(
                    "formation seat bindings exceed maximum count {FMAN_SEAT_BINDINGS_MAX_COUNT}"
                )));
            }
            Ok(bindings)
        }
    }

    deserializer.deserialize_seq(BoundedSeatBindingsVisitor)
}

/// Response to a formation metadata submission. Success acknowledges one
/// guardian vote submission, not threshold consensus adoption.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct ProposeFormationMetaResponse;

/// Request for this seat's `fedimintd` stats.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetFedimintStatsRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity.
    pub fi_id: FiId,

    /// Seat being queried.
    pub seat_id: SeatId,
}

/// Response containing this seat's `fedimintd` stats.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GetFedimintStatsResponse {
    /// This seat's `fedimintd` stats.
    pub stats: FedimintStats,
}
