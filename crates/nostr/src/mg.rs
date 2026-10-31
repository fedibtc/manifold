//! MG-related Nostr event kinds, tags, signing constants, and the shared
//! portable advertisement document.
//!
//! The document types below are the one typed rendering of the
//! `SPEC-mg-nostr-events` kind-37701 content schema (`crates/nostr/specs/`).
//! The MG publisher (`crates/mg/nostr`) and the FA consumer
//! (`crates/fa-client`) both depend on this rendering so producer and
//! verifier can never disagree on the wire shape.

#[cfg(test)]
mod tests;

use fedi_decentralized_domain::{
    HolderAuthorizationEnvelope, ProtocolV1, PurposeTag, SignedJsonEnvelope, SignedJsonError,
};
use fedi_decentralized_service_manifold_guardian::{FedimintdVersion, Plan};
use peerbadge_protocol::SchnorrSignatureProof;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Signed content of a Holder-published MG authorization event.
///
/// This is the cross-component wire shape carried by kind 37705. The outer
/// Nostr event author must still be checked against `holder_id_pubkey`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HolderAuthorizationEventContent {
    /// Wire protocol version.
    pub version: ProtocolV1,

    /// Holder identity duplicated from the event author for signed-content checks.
    pub holder_id_pubkey: String,

    /// Authorization and its backing issuer credential.
    #[serde(flatten)]
    pub authorization: HolderAuthorizationEnvelope,
}

/// MG advertisement event kind.
///
/// Addressable, provisional kind used by MGs to advertise availability and
/// embedded holder authorizations.
pub const MG_ADVERTISEMENT_EVENT_KIND: u16 = 37701;

/// MG advertisement v2 event kind (`SPEC-mg-nostr-events`, *MG
/// advertisement v2*).
///
/// Addressable, provisional kind whose content is an exact-payload signed
/// envelope over [`AdvertisementV2Payload`].
pub const MG_ADVERTISEMENT_V2_EVENT_KIND: u16 = 37709;

/// Holder-published MG authorization event kind.
///
/// Addressable, provisional kind used by holders to publish credentials
/// authorized for use by a specific MG.
pub const HOLDER_AUTHORIZATION_EVENT_KIND: u16 = 37705;

/// MG encrypted backup event kind.
///
/// Addressable, provisional kind used by an MG to publish its own recovery
/// material. Unlike every other kind here this is not a cross-program schema:
/// the content is encrypted to the publishing MG's own backup identity and
/// has no external consumer
/// ([SPEC-nostr-backup-restore](../../mg/specs/SPEC-nostr-backup-restore.md)).
pub const MG_BACKUP_EVENT_KIND: u16 = 37708;

/// `d` tag value used for MG advertisements.
pub const MG_ADVERTISEMENT_D_TAG: &str = "fman-ad";

// Backup events have no `d` tag constant here, deliberately. Their coordinate
// is an opaque value the publishing MG computes under a key only its
// mnemonic derives, so nothing about a document — not even which family it
// belongs to — is stated in the clear. See `mg-core`'s `backup` module.

/// `d` tag value used for MG advertisement v2 events.
pub const MG_ADVERTISEMENT_V2_D_TAG: &str = "mg-ad";

/// Hashtag used to index MG advertisement v2 events.
pub const MG_ADVERTISEMENT_V2_HASHTAG: &str = "fedi-mg";

/// Hashtag used to index MG advertisements.
pub const MG_ADVERTISEMENT_HASHTAG: &str = "fedi-fman";

/// Hashtag used to index holder-published MG authorizations.
pub const MG_AUTHORIZATION_HASHTAG: &str = "fedi-fman-authorization";

/// Prefix for holder-published MG authorization `d` tags.
pub const MG_AUTHORIZATION_D_TAG_PREFIX: &str = "fman-authorization";

/// Build the `d` tag value for a holder-published MG authorization.
pub fn mg_authorization_d_tag(mg_pubkey: &str, credential_id: &str) -> String {
    format!("{MG_AUTHORIZATION_D_TAG_PREFIX}:{mg_pubkey}:{credential_id}")
}

/// Transport identifier of iroh `api_endpoints` entries.
///
/// The publisher emits it and the FA consumer's dial-eligibility check
/// matches on it, so both sides share this constant.
pub const IROH_API_ENDPOINT_TRANSPORT: &str = "iroh";

/// URL scheme prefix of iroh `api_endpoints` URLs (`iroh://<endpoint-id>`).
///
/// The publisher formats endpoint URLs with it and the FA consumer strips it
/// when building a dialing locator, so both sides share this constant.
pub const IROH_API_ENDPOINT_URL_SCHEME: &str = "iroh://";

/// Domain separator for MG advertisement payload signatures.
///
/// Cross-program contract (`SPEC-mg-nostr-events`): programs outside this
/// repository sign and verify advertisements against exactly these bytes and
/// the JCS canonicalization in `payload_digest`. Changing either is a
/// coordinated wire break, never a local refactor.
pub const MG_ADVERTISEMENT_SIGNATURE_DOMAIN: &[u8] = b"fedi-fman-advertisement/v1\0";

/// The portable signed advertisement document (Nostr event `content`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdvertisementDocument {
    /// Signed advertisement payload.
    pub payload: AdvertisementPayload,

    /// MG service-key Schnorr proof over the canonical payload.
    pub proof: SchnorrSignatureProof,
}

/// Unsigned MG advertisement payload (`SPEC-mg-nostr-events`, v1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdvertisementPayload {
    /// Payload schema version; v1 is the only published shape.
    pub version: ProtocolV1,

    /// MUST equal the Nostr event author pubkey (MVP identity rule).
    #[serde(rename = "fman_id_pubkey")]
    pub mg_id_pubkey: String,

    /// MG commitment-signing public key (x-only secp256k1, lowercase hex)
    /// — the `service-manifold-guardian` `Locator` `service_pubkey` FAs verify
    /// signed MG responses against. Distinct from `fman_id_pubkey` (the
    /// Nostr service identity); both derive from the MG root mnemonic.
    pub service_pubkey: String,

    /// Unix seconds at which the MG built this payload.
    pub issued_at: u64,

    /// Unix seconds after which the advertisement must be rejected.
    pub expires_at: u64,

    /// Pre-formation FA setup RPC endpoints; non-trust dialing hints.
    pub api_endpoints: Vec<ApiEndpoint>,

    /// Advertised capacity; non-trust hints.
    pub availability: Availability,

    /// Mirrors the `Plan` enum's own wire encoding, as `GetAvailability`
    /// serves it, so advertisement and RPC can never disagree. The protocol
    /// crate's canonical serde form is the published v1 shape, recorded in
    /// `SPEC-mg-nostr-events` (`crates/nostr/specs/`).
    pub plans: Vec<Plan>,

    /// Embedded verified trust envelopes; the only trust inputs in the
    /// advertisement. MG-verified before embedding; untrusted to consumers
    /// until verified.
    pub holder_authorizations: Vec<HolderAuthorizationEnvelope>,
}

/// One pre-formation FA setup RPC endpoint declared by an advertisement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApiEndpoint {
    /// Transport identifier, [`IROH_API_ENDPOINT_TRANSPORT`] in v1.
    pub transport: String,

    /// Transport-specific endpoint URL.
    pub url: String,
}

/// What the MG will serve; non-trust hints.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Availability {
    /// Exact Fedimintd version this MG runs.
    pub fedimintd_version: FedimintdVersion,

    /// Federation sizes the MG's release supports.
    pub federation_sizes: Vec<u16>,
}

/// The two published MG advertisement formats. Each has its own addressable
/// coordinate, so an MG publishes both and neither replaces the other.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MgAdvertisementFormat {
    /// Kind 37701: [`AdvertisementDocument`] with its JCS payload proof.
    Legacy,

    /// Kind 37709: [`AdvertisementV2Payload`] in an exact-payload envelope.
    V2,
}

impl MgAdvertisementFormat {
    /// Both formats, in publication order.
    pub const ALL: [Self; 2] = [Self::Legacy, Self::V2];

    /// Event kind of this format.
    #[must_use]
    pub const fn kind(self) -> u16 {
        match self {
            Self::Legacy => MG_ADVERTISEMENT_EVENT_KIND,
            Self::V2 => MG_ADVERTISEMENT_V2_EVENT_KIND,
        }
    }

    /// `d` tag of this format.
    #[must_use]
    pub const fn d_tag(self) -> &'static str {
        match self {
            Self::Legacy => MG_ADVERTISEMENT_D_TAG,
            Self::V2 => MG_ADVERTISEMENT_V2_D_TAG,
        }
    }

    /// Index hashtag of this format.
    #[must_use]
    pub const fn hashtag(self) -> &'static str {
        match self {
            Self::Legacy => MG_ADVERTISEMENT_HASHTAG,
            Self::V2 => MG_ADVERTISEMENT_V2_HASHTAG,
        }
    }
}

/// Purpose tag of the MG advertisement v2 envelope.
///
/// Cross-program contract: signers and verifiers outside this repository use
/// exactly these bytes ([`PurposeTag`]).
pub const MG_ADVERTISEMENT_V2_PURPOSE: PurposeTag =
    PurposeTag::new(b"fedi-signed-json/mg-advertisement/v1\0");

/// Signed payload of an MG advertisement v2 (`SPEC-mg-nostr-events`, *MG
/// advertisement v2*).
///
/// The legacy fields keep their meanings and serde shapes. The envelope
/// signer is the MG identity, so the payload carries no `fman_id_pubkey`,
/// and the purpose tag versions the schema, so it carries no `version`.
///
/// Every Manifold-owned field decodes directly: no `#[serde(flatten)]` and no
/// untagged or internally tagged enum, because those buffer unknown values
/// and break the envelope's number rule ([`SignedJsonEnvelope`]).
/// The embedded PeerBadge credential's `info` and `blind_msg` are SDK-owned
/// `serde_json::Value`s and keep the SDK's own number handling.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdvertisementV2Payload {
    /// MG commitment-signing public key, as in [`AdvertisementPayload`].
    pub service_pubkey: String,

    /// Unix seconds at which the MG built this payload.
    pub issued_at: u64,

    /// Unix seconds after which the advertisement must be rejected.
    pub expires_at: u64,

    /// Pre-formation FA setup RPC endpoints; non-trust dialing hints.
    pub api_endpoints: Vec<ApiEndpoint>,

    /// Advertised capacity; non-trust hints.
    pub availability: Availability,

    /// Advertised plans, in the `Plan` enum's own wire encoding.
    pub plans: Vec<Plan>,

    /// Embedded holder-authorization trust envelopes, untrusted until
    /// verified.
    pub holder_authorizations: Vec<HolderAuthorizationEnvelope>,

    /// The MG's own uptime scores from its latest readiness run; absent
    /// means unreported. Self-asserted: consumers may display them but must
    /// not admit, reject, rank or select on them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_permille: Option<AdvertisedUptime>,
}

impl AdvertisementV2Payload {
    /// The kind-37701 payload a v1-only MG would publish for the same offer:
    /// every shared field, without the uptime report.
    #[must_use]
    pub fn to_legacy(&self, mg_id_pubkey: nostr::PublicKey) -> AdvertisementPayload {
        AdvertisementPayload {
            version: ProtocolV1,
            mg_id_pubkey: mg_id_pubkey.to_string(),
            service_pubkey: self.service_pubkey.clone(),
            issued_at: self.issued_at,
            expires_at: self.expires_at,
            api_endpoints: self.api_endpoints.clone(),
            availability: self.availability.clone(),
            plans: self.plans.clone(),
            holder_authorizations: self.holder_authorizations.clone(),
        }
    }
}

impl From<AdvertisementPayload> for AdvertisementV2Payload {
    /// The offer of a legacy payload, with no uptime report.
    fn from(legacy: AdvertisementPayload) -> Self {
        Self {
            service_pubkey: legacy.service_pubkey,
            issued_at: legacy.issued_at,
            expires_at: legacy.expires_at,
            api_endpoints: legacy.api_endpoints,
            availability: legacy.availability,
            plans: legacy.plans,
            holder_authorizations: legacy.holder_authorizations,
            uptime_permille: None,
        }
    }
}

/// An MG's self-reported uptime scores per window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdvertisedUptime {
    /// One-day score.
    #[serde(rename = "1d")]
    pub day: Permille,

    /// Seven-day score.
    #[serde(rename = "7d")]
    pub week: Permille,

    /// Thirty-day score.
    #[serde(rename = "30d")]
    pub month: Permille,
}

/// An integer in `0..=1000`.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Permille(u16);

impl Permille {
    /// The largest value, 100%.
    pub const MAX: u16 = 1000;

    /// The value, in `0..=1000`.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for Permille {
    type Error = PermilleOutOfRange;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(PermilleOutOfRange)
        }
    }
}

impl From<Permille> for u16 {
    fn from(value: Permille) -> Self {
        value.0
    }
}

/// A per-mille value above 1000.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermilleOutOfRange;

impl core::fmt::Display for PermilleOutOfRange {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("per-mille value above 1000")
    }
}

impl std::error::Error for PermilleOutOfRange {}

/// Sign a v2 payload as the MG Nostr service key: the event `content` of a
/// kind-37709 advertisement.
///
/// # Errors
///
/// Returns the envelope's size errors when the payload or envelope exceeds
/// its limit.
pub fn sign_advertisement_v2(
    payload: &AdvertisementV2Payload,
    keys: &nostr::Keys,
) -> Result<SignedJsonEnvelope, SignedJsonError> {
    SignedJsonEnvelope::sign(MG_ADVERTISEMENT_V2_PURPOSE, payload, keys)
}

/// Why a kind-37709 event content is not an advertisement by its author.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdvertisementV2Error {
    /// The content is not a decodable envelope, or its verified payload does
    /// not parse.
    Unparsable,

    /// The envelope signature does not verify under the advertisement tag.
    InvalidSignature,

    /// The verified signer is not the event author.
    AuthorMismatch,
}

impl core::fmt::Display for AdvertisementV2Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::Unparsable => "unparsable MG advertisement v2",
            Self::InvalidSignature => "invalid MG advertisement v2 signature",
            Self::AuthorMismatch => "MG advertisement v2 signer is not the event author",
        })
    }
}

impl std::error::Error for AdvertisementV2Error {}

/// Verify a kind-37709 event `content` and bind its signer to `author`.
///
/// This checks the envelope signature, parses the payload, and requires the
/// verified signer to equal the event author. It does not verify the outer
/// event signature, freshness, or the embedded holder authorizations.
///
/// # Errors
///
/// See [`AdvertisementV2Error`].
pub fn verify_advertisement_v2(
    content: &str,
    author: nostr::PublicKey,
) -> Result<AdvertisementV2Payload, AdvertisementV2Error> {
    let envelope =
        SignedJsonEnvelope::decode(content).map_err(|_| AdvertisementV2Error::Unparsable)?;
    let verified = envelope
        .verify::<AdvertisementV2Payload>(MG_ADVERTISEMENT_V2_PURPOSE)
        .map_err(|error| match error {
            SignedJsonError::InvalidSignature => AdvertisementV2Error::InvalidSignature,
            _ => AdvertisementV2Error::Unparsable,
        })?;
    if verified.signer() != author {
        return Err(AdvertisementV2Error::AuthorMismatch);
    }
    Ok(verified.into_payload())
}

/// Error signing or verifying a portable MG advertisement document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdvertisementDocumentError {
    /// The payload `fman_id_pubkey` does not match the signing key.
    SigningKeyMismatch,

    /// The payload `fman_id_pubkey` is not a canonical Nostr public key.
    MalformedMgIdPubkey,

    /// The payload could not be canonicalized for the signature digest.
    Canonicalize,

    /// The Schnorr proof does not match the payload and its pubkey.
    InvalidProof,
}

impl core::fmt::Display for AdvertisementDocumentError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SigningKeyMismatch => {
                formatter.write_str("advertisement payload pubkey must match the signing key")
            }
            Self::MalformedMgIdPubkey => formatter.write_str("malformed fman_id_pubkey"),
            Self::Canonicalize => {
                formatter.write_str("failed to canonicalize the advertisement payload")
            }
            Self::InvalidProof => formatter.write_str("invalid advertisement signature"),
        }
    }
}

impl std::error::Error for AdvertisementDocumentError {}

/// Sign `payload` with the MG service nostr key: Schnorr over
/// `SHA256(domain || JCS(payload))` (SPEC-mg-nostr-events, *Portable MG
/// advertisement document*).
///
/// # Errors
///
/// Returns an error when the payload pubkey does not match the signing key or
/// the payload cannot be canonicalized.
pub fn sign_advertisement(
    payload: AdvertisementPayload,
    keys: &nostr::Keys,
) -> Result<AdvertisementDocument, AdvertisementDocumentError> {
    if payload.mg_id_pubkey != keys.public_key().to_string() {
        return Err(AdvertisementDocumentError::SigningKeyMismatch);
    }
    let digest = payload_digest(&payload)?;
    let signature = keys.sign_schnorr(&nostr::secp256k1::Message::from_digest(digest));
    Ok(AdvertisementDocument {
        payload,
        proof: SchnorrSignatureProof { signature },
    })
}

/// Verify the advertisement document's self-signature only: the Schnorr
/// proof by the key the payload itself names in `fman_id_pubkey`, over the
/// canonicalized payload bytes (SPEC-mg-nostr-events, *Portable MG
/// advertisement document*).
///
/// Passing proves exactly one thing: whoever holds the key the payload names
/// signed exactly these bytes. It explicitly does NOT:
///
/// - verify the outer Nostr event's id or signature,
/// - bind the payload's `fman_id_pubkey` to the Nostr event author,
/// - check freshness (`issued_at`/`expires_at`),
/// - verify any embedded holder-authorization (PeerBadge) envelope.
///
/// The consumer pipeline must therefore additionally (1) verify the outer
/// event signature, (2) require `payload.fman_id_pubkey` to equal the event
/// author, and (3) verify the embedded holder-authorization envelopes.
///
/// # Errors
///
/// Returns an error when the payload pubkey is malformed, the payload cannot
/// be canonicalized, or the proof does not verify.
pub fn verify_advertisement_self_signature(
    document: &AdvertisementDocument,
) -> Result<(), AdvertisementDocumentError> {
    let pubkey = nostr::PublicKey::parse(&document.payload.mg_id_pubkey)
        .map_err(|_| AdvertisementDocumentError::MalformedMgIdPubkey)?;
    let digest = payload_digest(&document.payload)?;
    nostr::SECP256K1
        .verify_schnorr(
            &document.proof.signature,
            &nostr::secp256k1::Message::from_digest(digest),
            &pubkey
                .xonly()
                .map_err(|_| AdvertisementDocumentError::MalformedMgIdPubkey)?,
        )
        .map_err(|_| AdvertisementDocumentError::InvalidProof)
}

// Cross-program contract: the domain separator and the JCS canonicalization
// below are the byte-level signing convention of `SPEC-mg-nostr-events`.
// External producers and verifiers depend on these exact bytes.
fn payload_digest(payload: &AdvertisementPayload) -> Result<[u8; 32], AdvertisementDocumentError> {
    let canonical = serde_json_canonicalizer::to_vec(payload)
        .map_err(|_| AdvertisementDocumentError::Canonicalize)?;
    Ok(Sha256::new()
        .chain_update(MG_ADVERTISEMENT_SIGNATURE_DOMAIN)
        .chain_update(canonical)
        .finalize()
        .into())
}
