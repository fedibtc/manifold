//! FMan-related Nostr event kinds, tags, signing constants, and the shared
//! portable advertisement document.
//!
//! The document types below are the one typed rendering of the
//! `SPEC-fman-nostr-events` kind-37701 content schema (`crates/nostr/specs/`).
//! The FMan publisher (`crates/fman/nostr`) and the FI consumer
//! (`crates/fi-client`) both depend on this rendering so producer and
//! verifier can never disagree on the wire shape.

#[cfg(test)]
mod tests;

use fedi_credential_sdk_protocol::SchnorrSignatureProof;
use fedi_decentralized_domain::{HolderAuthorizationEnvelope, ProtocolV1};
use fedi_decentralized_service_fleet_manager::Plan;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Signed content of a Holder-published FMan authorization event.
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

/// FMan advertisement event kind.
///
/// Addressable, provisional kind used by FMans to advertise availability and
/// embedded holder authorizations.
pub const FMAN_ADVERTISEMENT_EVENT_KIND: u16 = 37701;

/// Holder-published FMan authorization event kind.
///
/// Addressable, provisional kind used by holders to publish credentials
/// authorized for use by a specific FMan.
pub const HOLDER_AUTHORIZATION_EVENT_KIND: u16 = 37705;

/// FMan encrypted backup event kind.
///
/// Addressable, provisional kind used by an FMan to publish its own recovery
/// material. Unlike every other kind here this is not a cross-program schema:
/// the content is encrypted to the publishing FMan's own backup identity and
/// has no external consumer
/// ([SPEC-nostr-backup-restore](../../fman/specs/SPEC-nostr-backup-restore.md)).
pub const FMAN_BACKUP_EVENT_KIND: u16 = 37708;

/// `d` tag value used for FMan advertisements.
pub const FMAN_ADVERTISEMENT_D_TAG: &str = "fman-ad";

// Backup events have no `d` tag constant here, deliberately. Their coordinate
// is an opaque value the publishing FMan computes under a key only its
// mnemonic derives, so nothing about a document — not even which family it
// belongs to — is stated in the clear. See `fman-core`'s `backup` module.

/// Hashtag used to index FMan advertisements.
pub const FMAN_ADVERTISEMENT_HASHTAG: &str = "fedi-fman";

/// Hashtag used to index holder-published FMan authorizations.
pub const FMAN_AUTHORIZATION_HASHTAG: &str = "fedi-fman-authorization";

/// Prefix for holder-published FMan authorization `d` tags.
pub const FMAN_AUTHORIZATION_D_TAG_PREFIX: &str = "fman-authorization";

/// Build the `d` tag value for a holder-published FMan authorization.
pub fn fman_authorization_d_tag(fman_pubkey: &str, credential_id: &str) -> String {
    format!("{FMAN_AUTHORIZATION_D_TAG_PREFIX}:{fman_pubkey}:{credential_id}")
}

/// Transport identifier of iroh `api_endpoints` entries.
///
/// The publisher emits it and the FI consumer's dial-eligibility check
/// matches on it, so both sides share this constant.
pub const IROH_API_ENDPOINT_TRANSPORT: &str = "iroh";

/// URL scheme prefix of iroh `api_endpoints` URLs (`iroh://<endpoint-id>`).
///
/// The publisher formats endpoint URLs with it and the FI consumer strips it
/// when building a dialing locator, so both sides share this constant.
pub const IROH_API_ENDPOINT_URL_SCHEME: &str = "iroh://";

/// Domain separator for FMan advertisement payload signatures.
///
/// Cross-program contract (`SPEC-fman-nostr-events`): programs outside this
/// repository sign and verify advertisements against exactly these bytes and
/// the JCS canonicalization in `payload_digest`. Changing either is a
/// coordinated wire break, never a local refactor.
pub const FMAN_ADVERTISEMENT_SIGNATURE_DOMAIN: &[u8] = b"fedi-fman-advertisement/v1\0";

/// The portable signed advertisement document (Nostr event `content`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdvertisementDocument {
    /// Signed advertisement payload.
    pub payload: AdvertisementPayload,

    /// FMan service-key Schnorr proof over the canonical payload.
    pub proof: SchnorrSignatureProof,
}

/// Unsigned FMan advertisement payload (`SPEC-fman-nostr-events`, v1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdvertisementPayload {
    /// Payload schema version; v1 is the only published shape.
    pub version: ProtocolV1,

    /// MUST equal the Nostr event author pubkey (MVP identity rule).
    pub fman_id_pubkey: String,

    /// FMan commitment-signing public key (x-only secp256k1, lowercase hex)
    /// — the `service-fleet-manager` `Locator` `service_pubkey` FIs verify
    /// signed FMan responses against. Distinct from `fman_id_pubkey` (the
    /// Nostr service identity); both derive from the FMan root mnemonic.
    pub service_pubkey: String,

    /// Unix seconds at which the FMan built this payload.
    pub issued_at: u64,

    /// Unix seconds after which the advertisement must be rejected.
    pub expires_at: u64,

    /// Pre-formation FI setup RPC endpoints; non-trust dialing hints.
    pub api_endpoints: Vec<ApiEndpoint>,

    /// Advertised capacity; non-trust hints.
    pub availability: Availability,

    /// Mirrors the `Plan` enum's own wire encoding, as `GetAvailability`
    /// serves it, so advertisement and RPC can never disagree. The protocol
    /// crate's canonical serde form is the published v1 shape, recorded in
    /// `SPEC-fman-nostr-events` (`crates/nostr/specs/`).
    pub plans: Vec<Plan>,

    /// Embedded verified trust envelopes; the only trust inputs in the
    /// advertisement. FMan-verified before embedding; untrusted to consumers
    /// until verified.
    pub holder_authorizations: Vec<HolderAuthorizationEnvelope>,
}

/// One pre-formation FI setup RPC endpoint declared by an advertisement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApiEndpoint {
    /// Transport identifier, [`IROH_API_ENDPOINT_TRANSPORT`] in v1.
    pub transport: String,

    /// Transport-specific endpoint URL.
    pub url: String,
}

/// What the FMan will serve; non-trust hints.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Availability {
    /// Fedimintd releases the FMan can run (singleton in MVP).
    pub fedimintd_versions: Vec<String>,

    /// Federation sizes the FMan's release supports.
    pub federation_sizes: Vec<u16>,
}

/// Error signing or verifying a portable FMan advertisement document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdvertisementDocumentError {
    /// The payload `fman_id_pubkey` does not match the signing key.
    SigningKeyMismatch,

    /// The payload `fman_id_pubkey` is not a canonical Nostr public key.
    MalformedFmanIdPubkey,

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
            Self::MalformedFmanIdPubkey => formatter.write_str("malformed fman_id_pubkey"),
            Self::Canonicalize => {
                formatter.write_str("failed to canonicalize the advertisement payload")
            }
            Self::InvalidProof => formatter.write_str("invalid advertisement signature"),
        }
    }
}

impl std::error::Error for AdvertisementDocumentError {}

/// Sign `payload` with the FMan service nostr key: Schnorr over
/// `SHA256(domain || JCS(payload))` (SPEC-fman-nostr-events, *Portable FMan
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
    if payload.fman_id_pubkey != keys.public_key().to_string() {
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
/// canonicalized payload bytes (SPEC-fman-nostr-events, *Portable FMan
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
    let pubkey = nostr::PublicKey::parse(&document.payload.fman_id_pubkey)
        .map_err(|_| AdvertisementDocumentError::MalformedFmanIdPubkey)?;
    let digest = payload_digest(&document.payload)?;
    nostr::SECP256K1
        .verify_schnorr(
            &document.proof.signature,
            &nostr::secp256k1::Message::from_digest(digest),
            &pubkey
                .xonly()
                .map_err(|_| AdvertisementDocumentError::MalformedFmanIdPubkey)?,
        )
        .map_err(|_| AdvertisementDocumentError::InvalidProof)
}

// Cross-program contract: the domain separator and the JCS canonicalization
// below are the byte-level signing convention of `SPEC-fman-nostr-events`.
// External producers and verifiers depend on these exact bytes.
fn payload_digest(payload: &AdvertisementPayload) -> Result<[u8; 32], AdvertisementDocumentError> {
    let canonical = serde_json_canonicalizer::to_vec(payload)
        .map_err(|_| AdvertisementDocumentError::Canonicalize)?;
    Ok(Sha256::new()
        .chain_update(FMAN_ADVERTISEMENT_SIGNATURE_DOMAIN)
        .chain_update(canonical)
        .finalize()
        .into())
}
