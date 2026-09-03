//! App-facing Public Liquidity API messages.

use crate::{
    AcceptedAttesterPolicy, BitcoinNetwork, FederationId, FederationName, FleetSeat,
    FmanPeerAttestation, GatewayApiUrl, GatewayId, GatewayName, GetFmanTrustMaterialResponse,
    HashBytes, HolderAuthorizationEnvelope, InviteCode, ItemId, ProtocolVersion, ProviderDisplay,
    ProviderPolicy, Pubkey, RevocationLocation, RpcEndpointId, Sats, Sha256Digest, SourceType,
    Timestamp, Url, WalletOperationId,
};
use serde::{Deserialize, Serialize};

/// Public event payload published by the provider for discovery.
///
/// In MVP this is also the only public ready event: FLIP publishes it only when
/// setup and dependency validation have succeeded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiquidityProviderAdvertisement {
    /// Spec version for this advertisement document.
    pub version: ProtocolVersion,

    /// Provider identity.
    pub provider_pubkey: Pubkey,

    /// Advertisement issue timestamp.
    pub issued_at: Timestamp,

    /// Advertisement expiry.
    pub expires_at: Timestamp,

    /// Supported liquidity sources.
    pub supported_sources: Vec<SourceType>,

    /// Inline trust envelopes binding holder-owned trust to this provider key.
    ///
    /// Each entry embeds the operator-signed holder authorization together
    /// with the backing issuer credential, matching the FMan advertisement
    /// convention; apps verify provider trust from the advertisement content
    /// alone plus issuer-controlled inputs.
    pub holder_authorizations: Vec<HolderAuthorizationEnvelope>,

    /// Provider policy visible to apps before request submission.
    pub policy: ProviderPolicy,

    /// Optional operator-readable provider metadata.
    pub display: Option<ProviderDisplay>,

    /// App-facing API endpoint URLs.
    pub api_endpoints: Vec<Url>,

    /// Supported Public Liquidity API versions.
    pub api_versions: Vec<ProtocolVersion>,

    /// Relay hints for finding this advertisement.
    pub relay_hints: Vec<Url>,
}

/// Optional provider preflight after validating an advertisement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetProviderInfoRequest {
    /// Spec version for this request payload.
    pub version: ProtocolVersion,

    /// Requester identity.
    pub requester_pubkey: Pubkey,

    /// Expected provider pubkey.
    pub provider_pubkey: Pubkey,

    /// Request issue timestamp.
    pub issued_at: Timestamp,

    /// Hash of the advertisement used for discovery.
    pub advertisement_hash: Sha256Digest,

    /// Public Liquidity API versions supported by the app.
    pub client_supported_versions: Vec<ProtocolVersion>,
}

/// Provider info response.
///
/// This call is optional in MVP. The response mostly repeats public
/// advertisement information, while confirming availability, endpoint identity,
/// and API version negotiation before the app sends private federation details.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetProviderInfoResponse {
    /// Spec version for this response payload.
    pub version: ProtocolVersion,

    /// Provider pubkey.
    pub provider_pubkey: Pubkey,

    /// Response issue timestamp.
    pub issued_at: Timestamp,

    /// Hash of the advertisement used for discovery.
    pub advertisement_hash: Sha256Digest,

    /// Supported liquidity sources.
    pub supported_sources: Vec<SourceType>,

    /// Current provider policy.
    pub policy: ProviderPolicy,

    /// Endpoint selected for this API session.
    pub api_endpoint_id: RpcEndpointId,

    /// Negotiated Public Liquidity API version.
    pub api_version: ProtocolVersion,

    /// Availability or rejection outcome.
    pub outcome: ProviderInfoOutcome,
}

/// Provider info outcome.
#[derive(Clone, Debug, Eq, PartialEq, strum::Display, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderInfoOutcome {
    /// Provider is available.
    #[strum(serialize = "available")]
    Available,

    /// Provider rejected the info request.
    #[strum(serialize = "rejected")]
    Rejected(PublicRejection),
}

/// Request concrete liquidity for a federation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestLiquidityRequest {
    /// Spec version for this request payload.
    pub version: ProtocolVersion,

    /// App or requester identity.
    pub requester_pubkey: Pubkey,

    /// Provider identity this request targets.
    pub provider_pubkey: Pubkey,

    /// Request issue timestamp.
    pub issued_at: Timestamp,

    /// Target Bitcoin network.
    pub network: BitcoinNetwork,

    /// Requested minimum and optional maximum amounts.
    pub amounts: LiquidityAmountBounds,

    /// Hash binding all request-scoped private details carried in this request.
    ///
    /// This hash commits to no FI-carried trust material. FLIP derives the
    /// federation config and its consensus metadata from `invite_code` and
    /// reads the seat bindings from `fedi:fman_seat_bindings` itself.
    /// `fman_endorsement` and `fman_trust_material` are both excluded, for the
    /// reason given on those fields.
    pub details_payload_hash: Sha256Digest,

    /// Private federation details required for provider evaluation.
    pub federation_details: FederationLiquidityDetails,

    /// Admission gate: one guardian's endorsement of this federation.
    ///
    /// `None` is a rejection, never a bypass — it is optional on the wire only
    /// so that a request lacking one still deserializes and can be answered
    /// with a signed `invalid_credentials` rejection instead of a bare
    /// transport error.
    ///
    /// Deliberately outside `details_payload_hash`. The commitment is the
    /// allocation's retry identity, so a requester re-sending the same intent
    /// with a freshly collected endorsement must hash identically and be
    /// answered from the existing allocation rather than treated as a
    /// conflicting request.
    pub fman_endorsement: Option<FmanEndorsement>,

    /// Signed trust material for every FMan identity operating this federation.
    ///
    /// Each entry is one FMan's own signed statement, collected by the
    /// requester from that FMan directly. Requester-supplied carriage is safe
    /// because it is not requester-*authored*: the seat-binding directory in
    /// consensus metadata pins which `fman_pubkey` operates each seat, and an
    /// entry is only ever consulted for the identity it is signed by, so
    /// substituting or omitting one cannot promote an identity the federation
    /// does not name. What the requester controls is whether an identity is
    /// answered for at all, and an unanswered identity is untrusted.
    ///
    /// `None` is a rejection, never a bypass — optional on the wire only so a
    /// request lacking it still deserializes and can be answered with a signed
    /// rejection instead of a bare transport error.
    ///
    /// Deliberately outside `details_payload_hash`, for the same reason as
    /// `fman_endorsement`: material is collected per attempt, so a retry
    /// carrying freshly fetched material must hash identically and resolve to
    /// the existing allocation.
    pub fman_trust_material: Option<Vec<GetFmanTrustMaterialResponse>>,

    /// Request expiry.
    pub expires_at: Timestamp,
}

/// One guardian's endorsement admitting a liquidity request.
///
/// Holding an endorsement is what authorizes the request. It names a
/// federation and binds nothing to the requester, carries no expiry of its
/// own, and is never authoritative for per-guardian policy — FLIP still
/// verifies every seat binding and resolves every operating identity's live
/// trust independently. See
/// [`SPEC-flip-rpc`](https://github.com/fedibtc/manifold/blob/master/crates/liquidity-manager-daemon/specs/SPEC-flip-rpc.md).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FmanEndorsement {
    /// FMan-signed binding between one guardian seat and its FMan identity.
    pub attestation: FmanPeerAttestation,

    /// Inline trust envelope binding an issuer badge to that FMan identity.
    pub trust: HolderAuthorizationEnvelope,
}

/// Canonical details commitment hashed into `details_payload_hash`.
///
/// This excludes request freshness, proof, and admission-gate fields by
/// construction; every field in this commitment is included in the hash
/// preimage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestLiquidityDetailsCommitmentV1 {
    /// Spec version for this commitment payload.
    pub version: ProtocolVersion,

    /// App or requester identity.
    pub requester_pubkey: Pubkey,

    /// Provider identity this request targets.
    pub provider_pubkey: Pubkey,

    /// Target Bitcoin network.
    pub network: BitcoinNetwork,

    /// Requested minimum and optional maximum amounts.
    pub amounts: LiquidityAmountBounds,

    /// Private federation details required for provider evaluation.
    pub federation_details: FederationLiquidityDetails,

    /// Request expiry.
    pub expires_at: Timestamp,
}

/// Gateway/LN and stability-pool amount bounds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LiquidityAmountBounds {
    /// Minimum amount for the single committed gateway/LN allocation.
    pub gateway_min_amount: Sats,

    /// Optional maximum amount for the single committed gateway/LN allocation.
    pub gateway_max_amount: Option<Sats>,

    /// Minimum amount for the committed stability-pool item.
    pub stability_min_amount: Sats,

    /// Optional maximum amount for the committed stability-pool item.
    pub stability_max_amount: Option<Sats>,
}

/// Private federation details sent only over authenticated RPC.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FederationLiquidityDetails {
    /// Fedimint invite code.
    pub invite_code: InviteCode,

    /// FI-observed Fedimint federation id.
    ///
    /// The provider must derive the authoritative value from the invite code.
    pub federation_id: FederationId,

    /// Federation display name.
    pub federation_name: FederationName,

    /// FI-observed hash of final federation config or formation transcript.
    ///
    /// The provider must derive the authoritative value from the invite-code
    /// preview before matching the consensus seat-binding directory.
    pub federation_config_hash: HashBytes,

    /// Optional FI-provided seat hints from formation.
    ///
    /// The provider must preview the invite code, derive the authoritative peer
    /// set itself, and fetch FMan-signed peer attestations from consensus
    /// metadata. These hints are retained only for
    /// diagnostics, idempotency analysis, and operator support.
    pub fleet_seat_hints: Vec<FleetSeat>,

    /// Revocation-location hints supplied by the app.
    ///
    /// These hints are not authoritative. Required issuer revocation lookups must
    /// complete using issuer-trusted locations before a request can be accepted.
    pub revocation_locations: Vec<RevocationLocation>,
}

/// Response to a liquidity request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestLiquidityResponse {
    /// Spec version for this response payload.
    pub version: ProtocolVersion,

    /// Private details hash.
    pub details_payload_hash: Sha256Digest,

    /// Provider identity.
    pub provider_pubkey: Pubkey,

    /// Response issue timestamp.
    pub issued_at: Timestamp,

    /// Accepted allocation or rejection outcome.
    pub outcome: RequestLiquidityOutcome,
}

/// Liquidity request outcome.
#[derive(Clone, Debug, Eq, PartialEq, strum::Display, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestLiquidityOutcome {
    /// Provider accepted and durably created allocation work.
    #[strum(serialize = "accepted")]
    Accepted(AllocationStatus),

    /// Provider rejected the request.
    #[strum(serialize = "rejected")]
    Rejected(PublicRejection),
}

/// Request allocation status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetAllocationStatusRequest {
    /// Spec version for this request payload.
    pub version: ProtocolVersion,

    /// Requester identity.
    pub requester_pubkey: Pubkey,

    /// Private details hash.
    pub details_payload_hash: Sha256Digest,

    /// Expected provider identity.
    pub provider_pubkey: Pubkey,

    /// Request issue timestamp.
    pub issued_at: Timestamp,
}

/// Allocation status response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetAllocationStatusResponse {
    /// Spec version for this response payload.
    pub version: ProtocolVersion,

    /// Provider identity.
    pub provider_pubkey: Pubkey,

    /// Response issue timestamp.
    pub issued_at: Timestamp,

    /// Current allocation status.
    pub status: AllocationStatus,
}

/// Status of accepted allocation work.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AllocationStatus {
    /// Private details hash.
    pub details_payload_hash: Sha256Digest,

    /// Provider identity.
    pub provider_pubkey: Pubkey,

    /// Status for each provider-committed allocation item.
    pub item_statuses: Vec<AllocationItemStatus>,
}

/// Per-item allocation status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AllocationItemStatus {
    /// Provider-committed target for this allocation item.
    pub target: AllocationItemTarget,

    /// Current item status.
    pub status: ItemAllocationStatus,

    /// Fulfilled amount once complete.
    pub fulfilled_amount: Option<Sats>,

    /// Completion evidence hints once complete.
    pub completion_evidence: Option<CompletionEvidence>,

    /// Failure details when failed.
    pub failure: Option<LiquidityFailure>,

    /// Last status update timestamp.
    pub updated_at: Timestamp,
}

/// Provider-committed allocation target.
#[derive(Clone, Debug, Eq, PartialEq, strum::Display, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationItemTarget {
    /// Gateway/LN allocation target.
    #[strum(serialize = "gateway")]
    Gateway {
        /// Item id.
        item_id: ItemId,

        /// Gateway id.
        gateway_id: GatewayId,

        /// Gateway display name.
        gateway_name: GatewayName,

        /// Committed allocation amount.
        amount: Sats,
    },

    /// Stability-pool allocation target.
    #[strum(serialize = "stability_pool")]
    StabilityPool {
        /// Item id.
        item_id: ItemId,

        /// Committed allocation amount.
        amount: Sats,
    },
}

/// Item allocation status.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    strum::Display,
    strum::EnumString,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ItemAllocationStatus {
    /// Item is queued.
    #[strum(serialize = "pending")]
    Pending,

    /// Item is running.
    #[strum(serialize = "running")]
    Running,

    /// Item stopped safely and requires an operator retry or cancellation.
    #[strum(serialize = "action_required")]
    ActionRequired,

    /// Item completed.
    #[strum(serialize = "completed")]
    Completed,

    /// Item failed.
    #[strum(serialize = "failed")]
    Failed,

    /// Item was cancelled.
    #[strum(serialize = "cancelled")]
    Cancelled,
}

/// Completion evidence hints.
#[derive(Clone, Debug, Eq, PartialEq, strum::Display, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionEvidence {
    /// Gateway/LN completion evidence.
    #[strum(serialize = "gateway")]
    Gateway(GatewayCompletionEvidence),

    /// Stability-pool completion evidence.
    #[strum(serialize = "stability_pool")]
    StabilityPool(StabilityPoolCompletionEvidence),
}

/// Gateway/LN completion evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GatewayCompletionEvidence {
    /// Gateway id.
    pub gateway_id: GatewayId,

    /// Canonical client-reachable LNv2 gateway API URL.
    pub gateway_api: GatewayApiUrl,

    /// Fulfilled amount.
    pub fulfilled_amount: Sats,

    /// Observed gateway balance for this federation and gateway pair.
    pub observed_gateway_balance: Sats,

    /// Observation time.
    pub observed_at: Timestamp,

    /// Withdrawal transaction id, when available.
    pub withdrawal_txid: Option<String>,

    /// Wallet operation id, when available.
    pub wallet_operation_id: Option<WalletOperationId>,
}

/// Stability-pool completion evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StabilityPoolCompletionEvidence {
    /// Fulfilled amount.
    pub fulfilled_amount: Sats,

    /// Observed stability-pool provided amount.
    pub observed_provided_amount: Sats,

    /// Observation time.
    pub observed_at: Timestamp,

    /// Peg-in operation id, when available.
    pub peg_in_operation_id: Option<String>,

    /// Stability-pool deposit operation id, when available.
    pub stability_pool_deposit_operation_id: Option<String>,
}

/// Public request rejection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublicRejection {
    /// Machine-readable rejection code.
    pub code: PublicRejectionCode,

    /// Optional operator-readable detail.
    pub reason: Option<String>,
}

/// Public liquidity request rejection code.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PublicRejectionCode {
    /// Requested network is unsupported.
    #[strum(serialize = "unsupported_network")]
    UnsupportedNetwork,

    /// Requested source type is unsupported.
    #[strum(serialize = "unsupported_source_type")]
    UnsupportedSourceType,

    /// Request expired.
    #[strum(serialize = "request_expired")]
    RequestExpired,

    /// Amount bounds are invalid.
    #[strum(serialize = "invalid_amount_bounds")]
    InvalidAmountBounds,

    /// Federation does not satisfy provider policy.
    #[strum(serialize = "policy_mismatch")]
    PolicyMismatch,

    /// Provider is unavailable.
    #[strum(serialize = "provider_unavailable")]
    ProviderUnavailable,

    /// Provider lacks capacity for the requested allocation.
    #[strum(serialize = "insufficient_capacity")]
    InsufficientCapacity,

    /// Request id was reused with different request content.
    #[strum(serialize = "request_conflict")]
    RequestConflict,

    /// Private details are invalid or inconsistent.
    #[strum(serialize = "invalid_details_payload")]
    InvalidDetailsPayload,

    /// Credentials are missing or invalid.
    #[strum(serialize = "invalid_credentials")]
    InvalidCredentials,

    /// FMan peer attestation is missing or invalid.
    #[strum(serialize = "invalid_seat_binding")]
    InvalidSeatBinding,

    /// Public Liquidity API version is unsupported.
    #[strum(serialize = "version_unsupported")]
    VersionUnsupported,

    /// Internal provider error.
    #[strum(serialize = "internal_error")]
    InternalError,
}

/// Allocation failure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LiquidityFailure {
    /// Machine-readable failure code.
    pub code: LiquidityFailureCode,

    /// Optional operator-readable detail.
    pub reason: Option<String>,
}

/// Allocation failure code.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum LiquidityFailureCode {
    /// Request expired.
    #[strum(serialize = "request_expired")]
    RequestExpired,

    /// Policy no longer matches.
    #[strum(serialize = "policy_mismatch")]
    PolicyMismatch,

    /// Provider funds are insufficient.
    #[strum(serialize = "insufficient_provider_funds")]
    InsufficientProviderFunds,

    /// Gateway attach failed.
    #[strum(serialize = "gateway_attach_failed")]
    GatewayAttachFailed,

    /// Wallet withdrawal failed.
    #[strum(serialize = "withdraw_failed")]
    WithdrawFailed,

    /// Stability-pool operation failed.
    #[strum(serialize = "stability_pool_failed")]
    StabilityPoolFailed,

    /// Internal provider error.
    #[strum(serialize = "internal_error")]
    InternalError,
}

/// Verification summary exposed to operators.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VerificationSummary {
    /// Federation whose request was verified.
    pub federation_id: FederationId,

    /// Whether provider policy passed.
    pub policy_result: VerificationCheckStatus,

    /// Seat-binding checks.
    pub seat_checks: Vec<VerificationCheck>,

    /// Credential checks.
    pub credential_checks: Vec<VerificationCheck>,

    /// Revocation checks.
    pub revocation_checks: Vec<VerificationCheck>,

    /// Accepted attester policy that passed, when any.
    pub accepted_attester_policy: Option<AcceptedAttesterPolicy>,

    /// Failure reason when verification failed.
    pub failure_reason: Option<String>,
}

/// Verification check status.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum VerificationCheckStatus {
    /// Check passed.
    #[strum(serialize = "passed")]
    Passed,

    /// Check failed.
    #[strum(serialize = "failed")]
    Failed,

    /// Check did not run.
    #[strum(serialize = "not_run")]
    NotRun,
}

/// Individual verification check result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VerificationCheck {
    /// Stable check name.
    pub name: String,

    /// Check status.
    pub status: VerificationCheckStatus,

    /// Subject of the check, such as a fleet-manager pubkey or seat id.
    pub subject: Option<String>,

    /// Operator-readable detail.
    pub detail: Option<String>,
}
