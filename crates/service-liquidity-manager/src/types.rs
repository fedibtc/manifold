//! FLIP protocol vocabulary shared by the public and admin API surfaces:
//! identifier newtypes, signature domain separators, provider policy shapes,
//! and list/pagination primitives.
//!
//! Cross-component primitives live in the shared domain crate; only
//! FLIP-owned vocabulary belongs here.

use fedi_decentralized_services::domain::{
    BitcoinNetwork, FleetSeatId, GuardianIdentity, PeerId, ProtocolVersion, Pubkey, Timestamp, Url,
};
use serde::{Deserialize, Serialize};

/// Opaque allocated item identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemId(pub String);

/// Opaque configured gateway identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GatewayId(pub String);

/// Operator-visible gateway display name.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GatewayName(pub String);

/// Opaque wallet operation identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WalletOperationId(pub String);

/// Opaque installed attestation payload identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AttestationPayloadId(pub String);

/// Raw provider attestation payload ingested by the operator.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AttestationPayload(pub Vec<u8>);

/// Opaque public API endpoint identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RpcEndpointId(pub String);

/// Transport-specific public API endpoint locator.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RpcEndpointAddress(pub String);

/// Optional transport discovery hint for an endpoint.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RpcDiscoveryHint(pub String);

/// RPC protocol name negotiated for an endpoint.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RpcProtocolName(pub String);

/// SHA-256 digest of a canonical domain-tagged FLIP payload.
///
/// Unlike [`HashBytes`](crate::HashBytes), which carries opaque
/// foreign-produced hashes of unspecified length (for example the
/// FMan-shaped `federation_config_hash`), every value of this type is a
/// digest this protocol computed itself, so the 32-byte shape is part of
/// the type. Serialization is identical to a 32-byte `HashBytes`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sha256Digest(pub [u8; 32]);

impl TryFrom<Vec<u8>> for Sha256Digest {
    type Error = WrongDigestLength;

    fn try_from(bytes: Vec<u8>) -> Result<Self, WrongDigestLength> {
        let len = bytes.len();
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| WrongDigestLength(len))?;
        Ok(Self(bytes))
    }
}

/// Byte slice offered as a [`Sha256Digest`] was not 32 bytes long.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WrongDigestLength(pub usize);

impl core::fmt::Display for WrongDigestLength {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SHA-256 digest must be 32 bytes, got {}", self.0)
    }
}

impl std::error::Error for WrongDigestLength {}

/// Public Liquidity API ALPN used by the MVP Iroh transport.
pub const PUBLIC_LIQUIDITY_API_ALPN: &[u8] = b"fedi/flip/public-liquidity/1";

/// Current Public Liquidity API protocol version, spoken by every public
/// verb and advertised in `api_versions`.
pub const PUBLIC_LIQUIDITY_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);

/// Domain separator for FLIP provider advertisement signatures.
pub const LIQUIDITY_PROVIDER_ADVERTISEMENT_SIGNATURE_DOMAIN: &[u8] =
    b"fedi-flip-advertisement/v1\0";

/// Domain separator for `GetProviderInfo` request signatures.
pub const GET_PROVIDER_INFO_REQUEST_SIGNATURE_DOMAIN: &[u8] =
    b"fedi-flip-get-provider-info-request/v1\0";

/// Domain separator for `GetProviderInfo` response signatures.
pub const GET_PROVIDER_INFO_RESPONSE_SIGNATURE_DOMAIN: &[u8] =
    b"fedi-flip-get-provider-info-response/v1\0";

/// Domain separator for `RequestLiquidity` request signatures.
pub const REQUEST_LIQUIDITY_REQUEST_SIGNATURE_DOMAIN: &[u8] =
    b"fedi-flip-request-liquidity-request/v1\0";

/// Domain separator for `RequestLiquidity` response signatures.
pub const REQUEST_LIQUIDITY_RESPONSE_SIGNATURE_DOMAIN: &[u8] =
    b"fedi-flip-request-liquidity-response/v1\0";

/// Domain separator for `GetAllocationStatus` request signatures.
pub const GET_ALLOCATION_STATUS_REQUEST_SIGNATURE_DOMAIN: &[u8] =
    b"fedi-flip-get-allocation-status-request/v1\0";

/// Domain separator for `GetAllocationStatus` response signatures.
pub const GET_ALLOCATION_STATUS_RESPONSE_SIGNATURE_DOMAIN: &[u8] =
    b"fedi-flip-get-allocation-status-response/v1\0";

/// Domain separator for `details_payload_hash` commitments.
pub const REQUEST_LIQUIDITY_DETAILS_HASH_DOMAIN: &[u8] = b"fedi-flip-details-payload-hash/v1\0";

/// Non-standard RPC transport name.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RpcTransportName(pub String);

/// Key for future role metadata attached to a fleet seat.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoleMetadataKey(pub String);

/// Value for future role metadata attached to a fleet seat.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoleMetadataValue(pub String);

/// Unencrypted FLIP data-directory tarball backup archive handle or path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BackupArchive(pub String);

/// Duration in whole seconds for operator-configurable timings.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DurationSecs(pub u64);

/// Liquidity source type.
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
pub enum SourceType {
    /// Gateway/LN liquidity.
    #[strum(serialize = "gateway")]
    Gateway,

    /// Stability-pool liquidity.
    #[strum(serialize = "stability_pool")]
    StabilityPool,
}

/// Provider capacity configuration mode.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CapacityMode {
    /// Use currently available funds as the capacity basis.
    #[strum(serialize = "available_funds")]
    AvailableFunds,

    /// Use at most an operator-configured cap.
    #[strum(serialize = "explicit_cap")]
    ExplicitCap,
}

/// Provider policy verification requirement.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, strum::Display, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum VerificationRequirement {
    /// Every distinct FMan identity operating at least one seat must be trusted.
    ///
    /// A single FMan may operate multiple peers in one federation; duplicate
    /// peers do not create additional trusted identities for policy counting.
    #[strum(serialize = "all_trusted")]
    AllTrusted,

    /// Distinct trusted FMan identities must satisfy the consensus threshold.
    ///
    /// The verifier first matches FMan peer attestations to config peers, then
    /// counts each trusted `fman_pubkey` once when evaluating the threshold,
    /// regardless of how many peers that FMan operates.
    #[strum(serialize = "consensus_majority_trusted")]
    ConsensusMajorityTrusted,
}

/// Accepted attester rule in provider policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AcceptedAttesterPolicy {
    /// Trusted attester public key.
    pub attester_pubkey: Pubkey,

    /// Requirement this attester policy must satisfy.
    pub verification_requirement: VerificationRequirement,
}

/// Public provider policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderPolicy {
    /// Attester policies accepted by this provider.
    pub accepted_attester_policies: Vec<AcceptedAttesterPolicy>,

    /// Bitcoin networks supported by this provider.
    pub supported_networks: Vec<BitcoinNetwork>,
}

/// Optional operator-readable provider metadata.
///
/// This is advertised for display and support routing only. It is not used for
/// trust decisions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderDisplay {
    /// Provider display name.
    pub name: Option<String>,

    /// Provider website.
    pub website: Option<Url>,

    /// Provider support or contact locator.
    pub contact: Option<String>,
}

/// Maximum byte length of `ProviderDisplay.name`.
pub const PROVIDER_DISPLAY_NAME_MAX_BYTES: usize = 128;

/// Maximum byte length of `ProviderDisplay.website`.
pub const PROVIDER_DISPLAY_WEBSITE_MAX_BYTES: usize = 512;

/// Maximum byte length of `ProviderDisplay.contact`.
pub const PROVIDER_DISPLAY_CONTACT_MAX_BYTES: usize = 256;

/// Error returned when provider display metadata violates the MVP limits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderDisplayValidationError {
    /// `name` is longer than [`PROVIDER_DISPLAY_NAME_MAX_BYTES`].
    NameTooLong,

    /// `website` is longer than [`PROVIDER_DISPLAY_WEBSITE_MAX_BYTES`].
    WebsiteTooLong,

    /// `website` is not an `https` URL.
    WebsiteNotHttps,

    /// `contact` is longer than [`PROVIDER_DISPLAY_CONTACT_MAX_BYTES`].
    ContactTooLong,

    /// A display field contains a control character.
    ControlCharacter,
}

impl core::fmt::Display for ProviderDisplayValidationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NameTooLong => write!(
                f,
                "display name is longer than {PROVIDER_DISPLAY_NAME_MAX_BYTES} bytes"
            ),
            Self::WebsiteTooLong => write!(
                f,
                "display website is longer than {PROVIDER_DISPLAY_WEBSITE_MAX_BYTES} bytes"
            ),
            Self::WebsiteNotHttps => f.write_str("display website is not an https URL"),
            Self::ContactTooLong => write!(
                f,
                "display contact is longer than {PROVIDER_DISPLAY_CONTACT_MAX_BYTES} bytes"
            ),
            Self::ControlCharacter => f.write_str("display metadata contains a control character"),
        }
    }
}

impl std::error::Error for ProviderDisplayValidationError {}

impl ProviderDisplay {
    /// Validate this display metadata against the MVP advertisement limits.
    ///
    /// Display metadata stays non-authoritative; these limits only bound what
    /// the provider publishes: `name` at most 128 bytes, `website` an `https`
    /// URL at most 512 bytes, `contact` at most 256 bytes, and no control
    /// characters in any field.
    ///
    /// # Errors
    ///
    /// Returns a [`ProviderDisplayValidationError`] naming the first violated
    /// limit.
    pub fn validate(&self) -> Result<(), ProviderDisplayValidationError> {
        if let Some(name) = &self.name {
            if name.len() > PROVIDER_DISPLAY_NAME_MAX_BYTES {
                return Err(ProviderDisplayValidationError::NameTooLong);
            }
            if name.chars().any(char::is_control) {
                return Err(ProviderDisplayValidationError::ControlCharacter);
            }
        }

        if let Some(website) = &self.website {
            if website.0.len() > PROVIDER_DISPLAY_WEBSITE_MAX_BYTES {
                return Err(ProviderDisplayValidationError::WebsiteTooLong);
            }
            if !website.0.starts_with("https://") {
                return Err(ProviderDisplayValidationError::WebsiteNotHttps);
            }
            if website.0.chars().any(char::is_control) {
                return Err(ProviderDisplayValidationError::ControlCharacter);
            }
        }

        if let Some(contact) = &self.contact {
            if contact.len() > PROVIDER_DISPLAY_CONTACT_MAX_BYTES {
                return Err(ProviderDisplayValidationError::ContactTooLong);
            }
            if contact.chars().any(char::is_control) {
                return Err(ProviderDisplayValidationError::ControlCharacter);
            }
        }

        Ok(())
    }
}

/// RPC transport named in provider advertisements or admin config.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcTransport {
    /// Iroh endpoint transport.
    Iroh,

    /// Placeholder for a future HTTP/JSON endpoint.
    HttpJson,

    /// Placeholder for a future JSON-RPC endpoint.
    JsonRpc,

    /// Other transport name.
    Other(RpcTransportName),
}

/// Parsed public API endpoint metadata.
///
/// Nostr advertisements publish endpoint URLs. This internal shape is available
/// for admin/config state after those URLs are parsed and endpoint identity
/// binding rules are applied. In MVP, endpoint identity binding comes from the
/// provider-signed advertisement carrying the endpoint URL plus the authenticated
/// Iroh identity reached when dialing that URL.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RpcEndpoint {
    /// Endpoint identifier.
    pub endpoint_id: RpcEndpointId,

    /// Transport used by the endpoint.
    pub transport: RpcTransport,

    /// Transport-specific address, URL, or node id.
    pub address: RpcEndpointAddress,

    /// Optional discovery hints for the transport.
    pub discovery_hints: Vec<RpcDiscoveryHint>,

    /// Supported RPC protocol name.
    pub rpc_protocol_name: RpcProtocolName,

    /// Endpoint expiry or rotation timestamp.
    pub expires_at: Option<Timestamp>,
}

/// Configured fleet-manager seat in a target federation.
///
/// FLIP treats this app-provided seat as an optional diagnostic hint. The
/// authoritative peer list, guardian identities, consensus threshold, FMan API
/// directory, and peer attestations are derived from the invite code, consensus
/// metadata, and public FMan APIs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FleetSeat {
    /// Seat identifier assigned by the app or formation protocol.
    pub seat_id: FleetSeatId,

    /// Fedimint peer id or guardian seat identifier.
    pub peer_id: PeerId,

    /// Guardian or consensus public identity from final federation config.
    pub guardian_identity: GuardianIdentity,

    /// Fleet-manager pubkey hint observed during formation.
    pub fleet_manager_pubkey: Pubkey,

    /// Future non-authoritative role metadata.
    ///
    /// MVP verification ignores these fields. They are reserved for labels such
    /// as app role, operator grouping, or deployment hints that may be useful to
    /// dashboards without changing the trust calculation.
    pub role_metadata: Vec<(RoleMetadataKey, RoleMetadataValue)>,
}

/// Page request for admin list APIs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageRequest {
    /// Opaque cursor from a previous page.
    pub cursor: Option<PageCursor>,

    /// Maximum number of entries to return.
    pub limit: u32,
}

/// Opaque page cursor.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PageCursor(pub String);

/// Time range filter for admin list APIs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimeRange {
    /// Inclusive lower bound.
    pub from: Option<Timestamp>,

    /// Exclusive upper bound.
    pub to: Option<Timestamp>,
}

/// Paginated list response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListResponse<T> {
    /// Returned page items.
    pub items: Vec<T>,

    /// Cursor for the next page, when more data is available.
    pub next_page: Option<PageCursor>,
}
