//! Federation-directory and public FMan trust-material protocol types.

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use serde::de::Error as _;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    FederationId, HashBytes, HolderAuthorizationEnvelope, PeerId, ProtocolV1, Pubkey,
    SchnorrSignatureProof, Timestamp, Url, fman_peer_attestation::FmanPeerAttestation,
};

/// Fedimint consensus metadata key containing public FMan API URLs.
pub const FMAN_API_URLS_META_FIELD_KEY: &str = "fedi:fman_api_urls";

/// Canonical payload type string for public FMan federation trust-material responses.
pub const FMAN_FEDERATION_TRUST_MATERIAL_CANONICAL_TYPE: &str =
    "fedi.fman.federation-trust-material";

/// Signature domain for public FMan federation trust-material responses.
pub const FMAN_FEDERATION_TRUST_MATERIAL_SIGNATURE_DOMAIN_SEPARATOR: &[u8] =
    b"fedi-fman-federation-trust-material/v1\0";

/// Maximum number of FMan public API URLs accepted in federation metadata.
pub const FMAN_API_URLS_MAX_COUNT: usize = 64;

/// Maximum canonical JSON value length accepted for `fedi:fman_api_urls`.
pub const FMAN_API_URLS_MAX_VALUE_BYTES: usize = 8192;

/// Maximum individual public FMan API URL length accepted in federation metadata.
pub const FMAN_API_URL_MAX_BYTES: usize = 512;

/// Maximum peer ids accepted in one public trust-material request filter.
pub const FMAN_TRUST_MATERIAL_PEER_FILTER_MAX_COUNT: usize = 64;

/// Maximum canonical public trust-material request size accepted by FMans.
pub const FMAN_TRUST_MATERIAL_MAX_REQUEST_BYTES: usize = 4096;

/// Intended maximum raw public trust-material request body size before
/// deserialization.
///
/// The current FMan server uses the generic iroh RPC frame limit instead and
/// does not apply this trust-material-specific limit before deserialization.
pub const FMAN_TRUST_MATERIAL_MAX_RAW_REQUEST_BYTES: usize = 4096;

/// Maximum federation id length accepted in public trust-material requests.
pub const FMAN_TRUST_MATERIAL_FEDERATION_ID_MAX_BYTES: usize = 128;

/// Maximum federation config hash length accepted in public trust-material requests.
pub const FMAN_TRUST_MATERIAL_CONFIG_HASH_MAX_BYTES: usize = 128;

/// Maximum peer id length accepted in public trust-material request filters.
pub const FMAN_TRUST_MATERIAL_PEER_ID_MAX_BYTES: usize = 64;

/// Maximum canonical public trust-material response size accepted by verifiers.
pub const FMAN_TRUST_MATERIAL_MAX_RESPONSE_BYTES: usize = 131_072;

/// Maximum FMan peer attestations accepted in one public trust-material response.
pub const FMAN_TRUST_MATERIAL_MAX_PEER_ATTESTATIONS: usize = 64;

/// Maximum holder authorizations accepted in one public trust-material response.
///
/// Each authorization carries its own backing credential, so this bounds both.
pub const FMAN_TRUST_MATERIAL_MAX_HOLDER_AUTHORIZATIONS: usize = 64;

/// Maximum distinct Holder authorizations retained by one FMan identity.
///
/// A Fleet Manager has one authorization set shared by all federations it
/// operates. Keeping this equal to the public response bound ensures every
/// retained authorization remains representable in one response.
pub const FMAN_HOLDER_AUTHORIZATION_RETENTION_MAX_COUNT: usize =
    FMAN_TRUST_MATERIAL_MAX_HOLDER_AUTHORIZATIONS;

/// Maximum future clock skew accepted for a Holder authorization statement.
pub const FMAN_HOLDER_AUTHORIZATION_MAX_FUTURE_SKEW_SECS: u64 = 3600;

/// Maximum future clock skew accepted for signed trust-material responses.
pub const FMAN_TRUST_MATERIAL_MAX_FUTURE_SKEW_SECS: u64 = 3600;

/// Canonical Fedimint metadata directory value for public FMan API discovery.
///
/// The value is discovery data only. Verifiers must treat every URL as an
/// untrusted locator, fetch signed trust material from FMans, and verify the
/// returned evidence locally before making policy decisions.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FmanApiUrlsMetadata {
    /// Protocol version for this metadata value.
    version: ProtocolV1,

    /// Sorted and deduplicated public FMan API URLs.
    fman_api_urls: Vec<Url>,
}

impl FmanApiUrlsMetadata {
    /// Build validated metadata from an arbitrary URL list.
    ///
    /// URLs are validated, sorted, deduplicated, and then canonicalized. The
    /// current MVP accepts only stable `iroh://...` URLs.
    ///
    /// # Errors
    ///
    /// Returns an error if URL validation, count limits, or serialized value
    /// limits fail.
    pub fn new(urls: impl IntoIterator<Item = Url>) -> Result<Self, FmanApiUrlsMetadataError> {
        let mut urls = urls.into_iter().collect::<Vec<_>>();
        for url in &urls {
            validate_fman_api_url(url)?;
        }
        urls.sort();
        urls.dedup();
        if urls.is_empty() {
            return Err(FmanApiUrlsMetadataError::Empty);
        }
        if urls.len() > FMAN_API_URLS_MAX_COUNT {
            return Err(FmanApiUrlsMetadataError::TooManyUrls);
        }

        let metadata = Self {
            version: ProtocolV1,
            fman_api_urls: urls,
        };
        let canonical = metadata.canonical_bytes()?;
        if canonical.len() > FMAN_API_URLS_MAX_VALUE_BYTES {
            return Err(FmanApiUrlsMetadataError::ValueTooLarge);
        }

        Ok(metadata)
    }

    /// Return the protocol version for this metadata value.
    #[must_use]
    pub fn version(&self) -> ProtocolV1 {
        self.version
    }

    /// Return the validated public FMan API URLs.
    #[must_use]
    pub fn fman_api_urls(&self) -> &[Url] {
        &self.fman_api_urls
    }

    /// Parse and validate a canonical metadata value.
    ///
    /// # Errors
    ///
    /// Returns an error if JSON parsing fails, the value is not in canonical
    /// form, or validation fails.
    pub fn parse_canonical(value: &str) -> Result<Self, FmanApiUrlsMetadataError> {
        if value.len() > FMAN_API_URLS_MAX_VALUE_BYTES {
            return Err(FmanApiUrlsMetadataError::ValueTooLarge);
        }
        let parsed: RawFmanApiUrlsMetadata =
            serde_json::from_str(value).map_err(|_| FmanApiUrlsMetadataError::MalformedJson)?;
        let normalized = Self::new(parsed.fman_api_urls)?;
        if normalized.canonical_string()? != value {
            return Err(FmanApiUrlsMetadataError::NonCanonical);
        }

        Ok(normalized)
    }

    /// Serialize this value as canonical JSON bytes suitable for Fedimint metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, FmanApiUrlsMetadataError> {
        serde_json_canonicalizer::to_vec(self)
            .map_err(|_| FmanApiUrlsMetadataError::CanonicalizationFailed)
    }

    /// Serialize this value as a canonical JSON string.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails or produces
    /// non-UTF-8 bytes.
    pub fn canonical_string(&self) -> Result<String, FmanApiUrlsMetadataError> {
        String::from_utf8(self.canonical_bytes()?)
            .map_err(|_| FmanApiUrlsMetadataError::CanonicalizationFailed)
    }
}

impl<'de> serde::Deserialize<'de> for FmanApiUrlsMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawFmanApiUrlsMetadata::deserialize(deserializer)?;
        let _version = raw.version;
        validate_canonical_url_order(&raw.fman_api_urls).map_err(D::Error::custom)?;
        let metadata = Self::new(raw.fman_api_urls).map_err(D::Error::custom)?;

        Ok(metadata)
    }
}

/// Raw serde shape for `FmanApiUrlsMetadata` before invariant validation.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFmanApiUrlsMetadata {
    /// Protocol version for this metadata value.
    version: ProtocolV1,

    /// Raw public FMan API URLs.
    fman_api_urls: Vec<Url>,
}

/// Error returned when validating the `fedi:fman_api_urls` metadata value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FmanApiUrlsMetadataError {
    /// The metadata value has no URLs.
    Empty,

    /// The metadata value contains too many URLs.
    TooManyUrls,

    /// The canonical metadata value exceeds the configured size limit.
    ValueTooLarge,

    /// The metadata value is not valid JSON.
    MalformedJson,

    /// The metadata JSON is valid but not exactly canonical.
    NonCanonical,

    /// Canonical JSON serialization failed.
    CanonicalizationFailed,

    /// A URL is empty.
    EmptyUrl,

    /// A URL exceeds the individual URL size limit.
    UrlTooLong,

    /// A URL contains ASCII control characters.
    UrlContainsControlCharacter,

    /// The URL scheme is not supported by the MVP validator.
    UnsupportedUrlScheme,

    /// The URL does not include transport endpoint material after the scheme.
    MissingEndpoint,
}

impl core::fmt::Display for FmanApiUrlsMetadataError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => f.write_str("FMan API URL metadata is empty"),
            Self::TooManyUrls => f.write_str("FMan API URL metadata has too many URLs"),
            Self::ValueTooLarge => f.write_str("FMan API URL metadata value is too large"),
            Self::MalformedJson => f.write_str("FMan API URL metadata JSON is malformed"),
            Self::NonCanonical => f.write_str("FMan API URL metadata is not canonical"),
            Self::CanonicalizationFailed => {
                f.write_str("FMan API URL metadata canonicalization failed")
            }
            Self::EmptyUrl => f.write_str("FMan API URL is empty"),
            Self::UrlTooLong => f.write_str("FMan API URL is too long"),
            Self::UrlContainsControlCharacter => {
                f.write_str("FMan API URL contains a control character")
            }
            Self::UnsupportedUrlScheme => f.write_str("FMan API URL scheme is unsupported"),
            Self::MissingEndpoint => f.write_str("FMan API URL is missing endpoint material"),
        }
    }
}

impl std::error::Error for FmanApiUrlsMetadataError {}

/// Validate one public FMan API URL for the metadata directory.
///
/// The MVP intentionally accepts only `iroh://...` locators and requires them
/// to be stable/restorable for the FMan service identity.
///
/// # Errors
///
/// Returns an error if the URL is empty, too large, not an Iroh URL, or lacks an
/// endpoint body.
pub fn validate_fman_api_url(url: &Url) -> Result<(), FmanApiUrlsMetadataError> {
    let value = url.0.as_str();
    if value.is_empty() {
        return Err(FmanApiUrlsMetadataError::EmptyUrl);
    }
    if value.len() > FMAN_API_URL_MAX_BYTES {
        return Err(FmanApiUrlsMetadataError::UrlTooLong);
    }
    if value.chars().any(char::is_control) {
        return Err(FmanApiUrlsMetadataError::UrlContainsControlCharacter);
    }
    let Some(endpoint) = value.strip_prefix("iroh://") else {
        return Err(FmanApiUrlsMetadataError::UnsupportedUrlScheme);
    };
    if endpoint.is_empty() {
        return Err(FmanApiUrlsMetadataError::MissingEndpoint);
    }

    Ok(())
}

/// Validate that a URL list is already sorted and deduplicated.
fn validate_canonical_url_order(urls: &[Url]) -> Result<(), FmanApiUrlsMetadataError> {
    let mut sorted = urls.to_vec();
    sorted.sort();
    sorted.dedup();
    if sorted != urls {
        return Err(FmanApiUrlsMetadataError::NonCanonical);
    }

    Ok(())
}

/// Public request for a FMan's trust material for one federation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GetFederationTrustMaterialRequest {
    /// Protocol version for this request shape.
    pub version: ProtocolV1,

    /// Target federation id derived from the invite-code-previewed final config.
    pub federation_id: FederationId,

    /// Hash of the invite-code-previewed final federation config.
    pub federation_config_hash: HashBytes,

    /// Peer filter for callers that only need a subset.
    ///
    /// Empty means "return every peer this FMan operates in the requested
    /// federation."
    pub peer_ids: Vec<PeerId>,
}

impl GetFederationTrustMaterialRequest {
    /// Validate this public trust-material request.
    ///
    /// # Errors
    ///
    /// Returns an error if the canonical request is too large, identifiers are
    /// malformed/oversized, or the peer filter is too large or contains
    /// duplicates.
    pub fn validate(&self) -> Result<(), FmanFederationTrustMaterialVerificationError> {
        let request_bytes = serde_json_canonicalizer::to_vec(self)
            .map_err(|_| FmanFederationTrustMaterialVerificationError::CanonicalizationFailed)?;
        if request_bytes.len() > FMAN_TRUST_MATERIAL_MAX_REQUEST_BYTES {
            return Err(FmanFederationTrustMaterialVerificationError::RequestTooLarge);
        }
        validate_identifier(
            &self.federation_id.0,
            FMAN_TRUST_MATERIAL_FEDERATION_ID_MAX_BYTES,
            FmanFederationTrustMaterialVerificationError::InvalidFederationId,
        )?;
        validate_hash_bytes(
            &self.federation_config_hash,
            FMAN_TRUST_MATERIAL_CONFIG_HASH_MAX_BYTES,
        )?;
        validate_peer_filter(&self.peer_ids)
    }
}

/// FMan trust material for one federation, signed by the FMan service key.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FmanFederationTrustMaterial {
    /// FMan service/advertisement pubkey that signs this response.
    pub fman_pubkey: Pubkey,

    /// Target federation id derived from the final config.
    pub federation_id: FederationId,

    /// Hash of the final federation config.
    pub federation_config_hash: HashBytes,

    /// Response issue timestamp.
    pub issued_at: Timestamp,

    /// Response freshness limit.
    pub expires_at: Timestamp,

    /// Current public FMan API URLs for this service identity.
    pub public_api_urls: Vec<Url>,

    /// FMan-signed peer attestations for peers this FMan operates.
    pub peer_attestations: Vec<FmanPeerAttestation>,

    /// Holder authorizations binding holder-owned badges to this FMan identity,
    /// each carried together with the credential backing it.
    ///
    /// The authorization and its backing credential travel as one
    /// [`HolderAuthorizationEnvelope`] rather than as two parallel lists.
    /// Parallel lists make an authorization with no matching credential
    /// representable, so every consumer would have to re-pair them by digest and
    /// decide what an unpairable entry means; the envelope makes that state
    /// unrepresentable instead. The FMan side already holds envelopes — its
    /// advertisement loop embeds exactly this type — so splitting them here
    /// would be an un-pairing performed only to be undone.
    pub holder_authorizations: Vec<HolderAuthorizationEnvelope>,
}

impl FmanFederationTrustMaterial {
    /// Build JCS canonical bytes for this public trust-material payload.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let payload = json!({
            "type": FMAN_FEDERATION_TRUST_MATERIAL_CANONICAL_TYPE,
            "version": ProtocolV1,
            "material": self,
        });

        serde_json_canonicalizer::to_vec(&payload)
    }

    /// Compute the signature digest for this public trust-material payload.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn digest(&self) -> Result<[u8; 32], serde_json::Error> {
        let digest = Sha256::new()
            .chain_update(FMAN_FEDERATION_TRUST_MATERIAL_SIGNATURE_DOMAIN_SEPARATOR)
            .chain_update(self.canonical_bytes()?)
            .finalize();

        Ok(digest.into())
    }
}

/// Signed response returned by the public FMan trust-material API.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GetFederationTrustMaterialResponse {
    /// Protocol version for this response shape.
    pub version: ProtocolV1,

    /// FMan trust material payload.
    pub material: FmanFederationTrustMaterial,

    /// Schnorr signature by `material.fman_pubkey` over the canonical payload.
    pub proof: SchnorrSignatureProof,
}

impl GetFederationTrustMaterialResponse {
    /// Verify this response's FMan service-key envelope signature.
    ///
    /// This verifies only the outer signed public trust-material envelope.
    /// Prefer [`Self::verify_for_request`] when evaluating material fetched for
    /// a concrete invite-code-derived federation.
    ///
    /// # Errors
    ///
    /// Returns an error when the FMan pubkey is malformed, canonicalization
    /// fails, or the Schnorr signature does not verify.
    pub fn verify_envelope_signature(
        &self,
    ) -> Result<FmanFederationTrustMaterial, FmanFederationTrustMaterialVerificationError> {
        let public_key = nostr::PublicKey::parse(&self.material.fman_pubkey.0)
            .map_err(|_| FmanFederationTrustMaterialVerificationError::InvalidFmanPubkey)?;
        if self.material.fman_pubkey.0 != public_key.to_string() {
            return Err(FmanFederationTrustMaterialVerificationError::InvalidFmanPubkey);
        }
        let public_key = public_key
            .xonly()
            .map_err(|_| FmanFederationTrustMaterialVerificationError::InvalidFmanPubkey)?;
        let message =
            nostr::secp256k1::Message::from_digest(self.material.digest().map_err(|_| {
                FmanFederationTrustMaterialVerificationError::CanonicalizationFailed
            })?);

        nostr::SECP256K1
            .verify_schnorr(&self.proof.signature, &message, &public_key)
            .map_err(|_| FmanFederationTrustMaterialVerificationError::InvalidSignature)?;

        Ok(self.material.clone())
    }

    /// Verify this response for a concrete request and verifier clock.
    ///
    /// This helper verifies envelope signature, response resource bounds,
    /// request federation/config-hash binding, freshness/expiry, public API URL
    /// validation, nested peer-attestation signatures, FMan ownership of nested
    /// attestations and holder authorizations, peer-filter handling, and duplicate
    /// peer-claim rejection. Credential cryptography, issuer trust, and revocation
    /// checks remain caller responsibilities because they depend on local verifier
    /// policy and issuer-controlled inputs.
    ///
    /// `max_validity_secs` is the caller's accepted upper bound for
    /// `expires_at - issued_at`.
    ///
    /// # Errors
    ///
    /// Returns an error if any envelope, freshness, resource-bound, or nested
    /// ownership check fails.
    pub fn verify_for_request(
        &self,
        request: &GetFederationTrustMaterialRequest,
        now: Timestamp,
        max_validity_secs: u64,
    ) -> Result<FmanFederationTrustMaterial, FmanFederationTrustMaterialVerificationError> {
        request.validate()?;
        validate_response_bounds(self)?;

        if self.material.federation_id != request.federation_id {
            return Err(FmanFederationTrustMaterialVerificationError::FederationMismatch);
        }
        if self.material.federation_config_hash != request.federation_config_hash {
            return Err(FmanFederationTrustMaterialVerificationError::ConfigHashMismatch);
        }
        validate_freshness(
            self.material.issued_at,
            self.material.expires_at,
            now,
            max_validity_secs,
        )?;

        FmanApiUrlsMetadata::new(self.material.public_api_urls.clone())
            .map_err(FmanFederationTrustMaterialVerificationError::InvalidPublicApiUrls)?;
        validate_canonical_url_order(&self.material.public_api_urls)
            .map_err(FmanFederationTrustMaterialVerificationError::InvalidPublicApiUrls)?;

        let verified_material = self.verify_envelope_signature()?;
        validate_nested_peer_attestations(request, &verified_material)?;
        validate_holder_authorization_subjects(&verified_material)?;

        Ok(verified_material)
    }
}

/// Error returned when verifying a public FMan trust-material response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FmanFederationTrustMaterialVerificationError {
    /// The material's `fman_pubkey` is not a valid canonical Nostr/secp256k1
    /// public key.
    InvalidFmanPubkey,

    /// The trust-material payload could not be canonicalized for verification.
    CanonicalizationFailed,

    /// The Schnorr signature does not verify for the claimed FMan pubkey.
    InvalidSignature,

    /// The request peer filter contains too many peer ids.
    PeerFilterTooLarge,

    /// The canonical request is too large.
    RequestTooLarge,

    /// The request federation id is malformed or too long.
    InvalidFederationId,

    /// The request federation config hash is empty or too long.
    InvalidConfigHash,

    /// A request peer id is malformed or too long.
    InvalidPeerId,

    /// The request peer filter contains duplicate peer ids.
    DuplicatePeerFilter,

    /// The canonical response is too large.
    ResponseTooLarge,

    /// The response contains too many public API URLs.
    TooManyPublicApiUrls,

    /// The response contains too many peer attestations.
    TooManyPeerAttestations,

    /// The response contains too many holder authorizations.
    TooManyHolderAuthorizations,

    /// The response federation id does not match the request.
    FederationMismatch,

    /// The response federation config hash does not match the request.
    ConfigHashMismatch,

    /// The response expires before or at its issue time.
    InvalidFreshnessWindow,

    /// The response issue time is too far in the future.
    IssuedInFuture,

    /// The response has expired.
    Expired,

    /// The response validity window exceeds verifier policy.
    ValidityWindowTooLarge,

    /// The response public API URL list is invalid.
    InvalidPublicApiUrls(FmanApiUrlsMetadataError),

    /// A nested peer attestation failed signature verification.
    InvalidPeerAttestation,

    /// A nested peer attestation claims a different FMan pubkey.
    NestedFmanMismatch,

    /// A nested peer attestation claims a different federation id.
    NestedFederationMismatch,

    /// A nested peer attestation claims a different federation config hash.
    NestedConfigHashMismatch,

    /// A nested peer attestation is outside the requested peer filter.
    PeerFilterMismatch,

    /// More than one nested peer attestation claims the same peer id.
    DuplicatePeerClaim,

    /// A holder authorization subject does not equal `material.fman_pubkey`.
    HolderAuthorizationSubjectMismatch,
}

impl core::fmt::Display for FmanFederationTrustMaterialVerificationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidFmanPubkey => f.write_str("invalid FMan pubkey"),
            Self::CanonicalizationFailed => {
                f.write_str("FMan federation trust material canonicalization failed")
            }
            Self::InvalidSignature => {
                f.write_str("invalid FMan federation trust material signature")
            }
            Self::PeerFilterTooLarge => f.write_str("FMan trust material peer filter is too large"),
            Self::RequestTooLarge => f.write_str("FMan trust material request is too large"),
            Self::InvalidFederationId => {
                f.write_str("FMan trust material request federation id is invalid")
            }
            Self::InvalidConfigHash => {
                f.write_str("FMan trust material request config hash is invalid")
            }
            Self::InvalidPeerId => f.write_str("FMan trust material request peer id is invalid"),
            Self::DuplicatePeerFilter => {
                f.write_str("FMan trust material peer filter contains duplicates")
            }
            Self::ResponseTooLarge => f.write_str("FMan trust material response is too large"),
            Self::TooManyPublicApiUrls => {
                f.write_str("FMan trust material response has too many public API URLs")
            }
            Self::TooManyPeerAttestations => {
                f.write_str("FMan trust material response has too many peer attestations")
            }
            Self::TooManyHolderAuthorizations => {
                f.write_str("FMan trust material response has too many holder authorizations")
            }
            Self::FederationMismatch => f.write_str("FMan trust material federation mismatch"),
            Self::ConfigHashMismatch => f.write_str("FMan trust material config hash mismatch"),
            Self::InvalidFreshnessWindow => {
                f.write_str("FMan trust material freshness window is invalid")
            }
            Self::IssuedInFuture => f.write_str("FMan trust material was issued in the future"),
            Self::Expired => f.write_str("FMan trust material has expired"),
            Self::ValidityWindowTooLarge => {
                f.write_str("FMan trust material validity window is too large")
            }
            Self::InvalidPublicApiUrls(err) => write!(f, "invalid public FMan API URLs: {err}"),
            Self::InvalidPeerAttestation => f.write_str("invalid nested FMan peer attestation"),
            Self::NestedFmanMismatch => f.write_str("nested FMan peer attestation pubkey mismatch"),
            Self::NestedFederationMismatch => {
                f.write_str("nested FMan peer attestation federation mismatch")
            }
            Self::NestedConfigHashMismatch => {
                f.write_str("nested FMan peer attestation config hash mismatch")
            }
            Self::PeerFilterMismatch => {
                f.write_str("nested FMan peer attestation outside peer filter")
            }
            Self::DuplicatePeerClaim => f.write_str("duplicate nested FMan peer claim"),
            Self::HolderAuthorizationSubjectMismatch => {
                f.write_str("holder authorization subject does not match FMan pubkey")
            }
        }
    }
}

impl std::error::Error for FmanFederationTrustMaterialVerificationError {}

/// Validate request peer-filter bounds and uniqueness.
fn validate_peer_filter(
    peer_ids: &[PeerId],
) -> Result<(), FmanFederationTrustMaterialVerificationError> {
    if peer_ids.len() > FMAN_TRUST_MATERIAL_PEER_FILTER_MAX_COUNT {
        return Err(FmanFederationTrustMaterialVerificationError::PeerFilterTooLarge);
    }
    let unique = peer_ids.iter().collect::<BTreeSet<_>>();
    if unique.len() != peer_ids.len() {
        return Err(FmanFederationTrustMaterialVerificationError::DuplicatePeerFilter);
    }
    for peer_id in peer_ids {
        validate_identifier(
            &peer_id.0,
            FMAN_TRUST_MATERIAL_PEER_ID_MAX_BYTES,
            FmanFederationTrustMaterialVerificationError::InvalidPeerId,
        )?;
    }

    Ok(())
}

/// Validate a string identifier used in a public trust-material request.
fn validate_identifier(
    value: &str,
    max_len: usize,
    error: FmanFederationTrustMaterialVerificationError,
) -> Result<(), FmanFederationTrustMaterialVerificationError> {
    if value.is_empty() || value.len() > max_len || value.chars().any(char::is_control) {
        return Err(error);
    }

    Ok(())
}

/// Validate hash bytes used in a public trust-material request.
fn validate_hash_bytes(
    value: &HashBytes,
    max_len: usize,
) -> Result<(), FmanFederationTrustMaterialVerificationError> {
    if value.0.is_empty() || value.0.len() > max_len {
        return Err(FmanFederationTrustMaterialVerificationError::InvalidConfigHash);
    }

    Ok(())
}

/// Validate response size and count bounds.
fn validate_response_bounds(
    response: &GetFederationTrustMaterialResponse,
) -> Result<(), FmanFederationTrustMaterialVerificationError> {
    let response_bytes = serde_json_canonicalizer::to_vec(response)
        .map_err(|_| FmanFederationTrustMaterialVerificationError::CanonicalizationFailed)?;
    if response_bytes.len() > FMAN_TRUST_MATERIAL_MAX_RESPONSE_BYTES {
        return Err(FmanFederationTrustMaterialVerificationError::ResponseTooLarge);
    }
    if response.material.public_api_urls.len() > FMAN_API_URLS_MAX_COUNT {
        return Err(FmanFederationTrustMaterialVerificationError::TooManyPublicApiUrls);
    }
    if response.material.peer_attestations.len() > FMAN_TRUST_MATERIAL_MAX_PEER_ATTESTATIONS {
        return Err(FmanFederationTrustMaterialVerificationError::TooManyPeerAttestations);
    }
    if response.material.holder_authorizations.len() > FMAN_TRUST_MATERIAL_MAX_HOLDER_AUTHORIZATIONS
    {
        return Err(FmanFederationTrustMaterialVerificationError::TooManyHolderAuthorizations);
    }

    Ok(())
}

/// Validate trust-material issue and expiry timestamps.
fn validate_freshness(
    issued_at: Timestamp,
    expires_at: Timestamp,
    now: Timestamp,
    max_validity_secs: u64,
) -> Result<(), FmanFederationTrustMaterialVerificationError> {
    if expires_at <= issued_at {
        return Err(FmanFederationTrustMaterialVerificationError::InvalidFreshnessWindow);
    }
    if issued_at.0
        > now
            .0
            .saturating_add(FMAN_TRUST_MATERIAL_MAX_FUTURE_SKEW_SECS)
    {
        return Err(FmanFederationTrustMaterialVerificationError::IssuedInFuture);
    }
    if expires_at <= now {
        return Err(FmanFederationTrustMaterialVerificationError::Expired);
    }
    if expires_at.0.saturating_sub(issued_at.0) > max_validity_secs {
        return Err(FmanFederationTrustMaterialVerificationError::ValidityWindowTooLarge);
    }

    Ok(())
}

/// Validate nested peer attestations against the request and material owner.
fn validate_nested_peer_attestations(
    request: &GetFederationTrustMaterialRequest,
    material: &FmanFederationTrustMaterial,
) -> Result<(), FmanFederationTrustMaterialVerificationError> {
    let filter = request.peer_ids.iter().collect::<BTreeSet<_>>();
    let mut seen_peers = BTreeSet::new();

    for attestation in &material.peer_attestations {
        let statement = attestation
            .verify()
            .map_err(|_| FmanFederationTrustMaterialVerificationError::InvalidPeerAttestation)?;
        if statement.fman_pubkey != material.fman_pubkey {
            return Err(FmanFederationTrustMaterialVerificationError::NestedFmanMismatch);
        }
        if statement.federation_id != request.federation_id {
            return Err(FmanFederationTrustMaterialVerificationError::NestedFederationMismatch);
        }
        if statement.federation_config_hash != request.federation_config_hash {
            return Err(FmanFederationTrustMaterialVerificationError::NestedConfigHashMismatch);
        }
        if !filter.is_empty() && !filter.contains(&statement.peer_id) {
            return Err(FmanFederationTrustMaterialVerificationError::PeerFilterMismatch);
        }
        if !seen_peers.insert(statement.peer_id) {
            return Err(FmanFederationTrustMaterialVerificationError::DuplicatePeerClaim);
        }
    }

    Ok(())
}

/// Validate holder authorization subjects against the material owner.
fn validate_holder_authorization_subjects(
    material: &FmanFederationTrustMaterial,
) -> Result<(), FmanFederationTrustMaterialVerificationError> {
    for envelope in &material.holder_authorizations {
        if envelope
            .holder_authorization
            .authorization
            .subject_pubkey
            .0
            .to_string()
            != material.fman_pubkey.0
        {
            return Err(
                FmanFederationTrustMaterialVerificationError::HolderAuthorizationSubjectMismatch,
            );
        }
    }

    Ok(())
}
