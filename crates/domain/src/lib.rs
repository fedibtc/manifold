//! Domain types and data shapes for decentralized federation.
//!
//! Several wrappers here are still provisional protocol placeholders. They are
//! expected to move toward credential-SDK types and canonical binary
//! representations as the cross-component contracts settle.

mod federation_config;
mod fman_federation_directory;
mod fman_peer_attestation;
mod fman_seat_bindings;
mod gateway;
mod setup_payment_federations;
pub mod test_support;
mod trust_score;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub use federation_config::{
    FEDERATION_CONFIG_HASH_DOMAIN_SEPARATOR, FederationConfigError, FederationSeat,
    FederationSeats, consensus_threshold, federation_config_hash, federation_id, federation_seats,
    parse_protocol_peer_id, protocol_peer_id,
};
pub use fedi_credential_sdk_protocol::{ProtocolV1, SchnorrSignatureProof};
pub use fman_federation_directory::{
    FMAN_API_URL_MAX_BYTES, FMAN_API_URLS_MAX_COUNT, FMAN_API_URLS_MAX_VALUE_BYTES,
    FMAN_API_URLS_META_FIELD_KEY, FMAN_HOLDER_AUTHORIZATION_MAX_FUTURE_SKEW_SECS,
    FMAN_HOLDER_AUTHORIZATION_RETENTION_MAX_COUNT, FMAN_TRUST_MATERIAL_CANONICAL_TYPE,
    FMAN_TRUST_MATERIAL_MAX_FUTURE_SKEW_SECS, FMAN_TRUST_MATERIAL_MAX_HOLDER_AUTHORIZATIONS,
    FMAN_TRUST_MATERIAL_MAX_RESPONSE_BYTES, FMAN_TRUST_MATERIAL_SIGNATURE_DOMAIN_SEPARATOR,
    FmanApiUrlsMetadata, FmanApiUrlsMetadataError, FmanTrustMaterial,
    FmanTrustMaterialVerificationError, GetFmanTrustMaterialRequest, GetFmanTrustMaterialResponse,
    validate_fman_api_url,
};
pub use fman_peer_attestation::{
    FMAN_PEER_ATTESTATION_CANONICAL_TYPE, FMAN_PEER_ATTESTATION_SIGNATURE_DOMAIN_SEPARATOR,
    FMAN_SEAT_ENDPOINT_PROOF_DOMAIN, FmanPeerAttestation, FmanPeerAttestationStatement,
    FmanPeerAttestationVerificationError, SeatEndpointProof,
};
pub use fman_seat_bindings::{
    FMAN_SEAT_BINDINGS_MAX_COUNT, FMAN_SEAT_BINDINGS_MAX_VALUE_BYTES,
    FMAN_SEAT_BINDINGS_META_FIELD_KEY, FmanSeatBindings, FmanSeatBindingsError,
    VerifiedSeatBinding,
};
pub use gateway::{GatewayApiUrl, InvalidGatewayApiUrl};
pub use setup_payment_federations::{
    AdmittedSetupPaymentFederations, DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM, FmanVersion,
    SETUP_PAYMENT_FEDERATION_INVITE_MAX_BYTES, SETUP_PAYMENT_FEDERATIONS_MAX_CONTENT_BYTES,
    SETUP_PAYMENT_FEDERATIONS_MAX_COUNT, SETUP_PAYMENT_MAX_MIN_FEE_PPM,
    SetupPaymentFederationsContent, SetupPaymentFederationsContentError,
};
pub use trust_score::{
    HolderAuthorizationEnvelope, HolderTrustEnvelopeError, PeerBadgeTrustPolicy,
    PeerBadgeTrustPolicyConfigError, PeerBadgeTrustPolicyError, TRUST_SCORE_LEVEL_MAX,
    TRUST_SCORE_LEVEL_MIN, TRUST_SCORE_SCHEMA_V1, TrustScoreBadgeV1, TrustScoreSchemaError,
    parse_trust_score_badge_v1, trust_score_blind_msg_v1, trust_score_info_v1,
    verify_holder_trust_envelope,
};

/// Public key used as a protocol actor identity.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Pubkey(pub String);

/// Protocol timestamp used for freshness and replay protection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Timestamp(pub u64);

/// Protocol version used by component API payloads.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ProtocolVersion(pub u16);

/// Federation display name.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct FederationName(pub String);

/// Fedimint federation id.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct FederationId(pub String);

/// Fedimint invite code.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct InviteCode(pub String);

/// Fedimint peer id or guardian seat identifier.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct PeerId(pub String);

/// Seat identifier assigned by the app or formation protocol.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct FleetSeatId(pub String);

/// Guardian or consensus public identity from the final federation config.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct GuardianIdentity(pub String);

/// Bitcoin amount denominated in sats.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Sats(pub u64);

/// Supported Bitcoin network.
#[derive(
    Clone,
    Copy,
    Debug,
    Deserialize,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    strum::Display,
    strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
pub enum BitcoinNetwork {
    /// Bitcoin mainnet.
    #[strum(serialize = "bitcoin")]
    Bitcoin,

    /// Bitcoin testnet.
    #[strum(serialize = "testnet")]
    Testnet,

    /// Bitcoin signet.
    #[strum(serialize = "signet")]
    Signet,

    /// Bitcoin regtest.
    #[strum(serialize = "regtest")]
    Regtest,
}

/// Canonical hash bytes.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct HashBytes(pub Vec<u8>);

/// Opaque canonical payload bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CanonicalPayload(pub Vec<u8>);

/// Signature bytes over a canonical protocol payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Signature(pub Vec<u8>);

/// Signed deterministic protocol payload document.
///
/// This follows the FMan Nostr convention: the signed document is portable
/// outside Nostr, `proof.signature` signs canonical `payload`, and all version,
/// freshness, and actor identity fields live inside the typed payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Signed<T> {
    /// Typed protocol payload.
    pub payload: T,

    /// Proof over canonical payload bytes.
    pub proof: PayloadProof,
}

/// Proof over a canonical signed payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PayloadProof {
    /// Signature over canonical payload bytes.
    pub signature: Signature,
}

/// Sensitive operator-provided value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SecretString(pub String);

/// URL used in provisional wire payloads.
///
/// This is a temporary string wrapper. Replace it with a proper URL type from a
/// URL parsing crate once the serialization and validation profile is settled.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Url(pub String);

/// Legacy opaque proof binding a fleet-manager identity to a federation seat.
///
/// New FLIP/FMan flows use [`FmanPeerAttestation`] instead. This wrapper remains
/// only for old provisional call sites that have not yet been updated.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SeatBindingProof(pub Vec<u8>);

/// Opaque id of a federation domain object.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct DomainId(String);

impl DomainId {
    /// Creates a new domain id.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
