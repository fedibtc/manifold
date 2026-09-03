//! Federation-directory and public FMan trust-material protocol types.

#[cfg(test)]
mod tests;

use serde::de::Error as _;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    HolderAuthorizationEnvelope, ProtocolV1, Pubkey, SchnorrSignatureProof, Timestamp, Url,
};

/// Fedimint consensus metadata key containing public FMan API URLs.
pub const FMAN_API_URLS_META_FIELD_KEY: &str = "fedi:fman_api_urls";

/// Canonical payload type string for public FMan trust-material responses.
pub const FMAN_TRUST_MATERIAL_CANONICAL_TYPE: &str = "fedi.fman.trust-material";

/// Signature domain for public FMan trust-material responses.
pub const FMAN_TRUST_MATERIAL_SIGNATURE_DOMAIN_SEPARATOR: &[u8] = b"fedi-fman-trust-material/v1\0";

/// Maximum number of FMan public API URLs accepted in federation metadata.
pub const FMAN_API_URLS_MAX_COUNT: usize = 64;

/// Maximum canonical JSON value length accepted for `fedi:fman_api_urls`.
pub const FMAN_API_URLS_MAX_VALUE_BYTES: usize = 8192;

/// Maximum individual public FMan API URL length accepted in federation metadata.
pub const FMAN_API_URL_MAX_BYTES: usize = 512;

/// Maximum canonical public trust-material response size accepted by verifiers.
pub const FMAN_TRUST_MATERIAL_MAX_RESPONSE_BYTES: usize = 131_072;

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

/// Public request for an FMan's current trust material.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetFmanTrustMaterialRequest {
    /// Protocol version for this request shape.
    pub version: ProtocolV1,
}

/// Current FMan trust material signed by the FMan service key.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct FmanTrustMaterial {
    /// FMan service/advertisement pubkey that signs this response.
    pub fman_pubkey: Pubkey,

    /// Response issue timestamp.
    pub issued_at: Timestamp,

    /// Response freshness limit.
    pub expires_at: Timestamp,

    /// Current public FMan API URLs for this service identity.
    pub public_api_urls: Vec<Url>,

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

impl FmanTrustMaterial {
    /// Build JCS canonical bytes for this public trust-material payload.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let payload = json!({
            "type": FMAN_TRUST_MATERIAL_CANONICAL_TYPE,
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
            .chain_update(FMAN_TRUST_MATERIAL_SIGNATURE_DOMAIN_SEPARATOR)
            .chain_update(self.canonical_bytes()?)
            .finalize();

        Ok(digest.into())
    }
}

/// Signed response returned by the public FMan trust-material API.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetFmanTrustMaterialResponse {
    /// Protocol version for this response shape.
    pub version: ProtocolV1,

    /// FMan trust material payload.
    pub material: FmanTrustMaterial,

    /// Schnorr signature by `material.fman_pubkey` over the canonical payload.
    pub proof: SchnorrSignatureProof,
}

impl GetFmanTrustMaterialResponse {
    /// Verify this response's FMan service-key envelope signature.
    ///
    /// This verifies only the outer signed public trust-material envelope.
    /// Prefer [`Self::verify_for_fman`] when evaluating freshly fetched material.
    ///
    /// # Errors
    ///
    /// Returns an error when the FMan pubkey is malformed, canonicalization
    /// fails, or the Schnorr signature does not verify.
    pub fn verify_envelope_signature(
        &self,
    ) -> Result<FmanTrustMaterial, FmanTrustMaterialVerificationError> {
        let public_key = nostr::PublicKey::parse(&self.material.fman_pubkey.0)
            .map_err(|_| FmanTrustMaterialVerificationError::InvalidFmanPubkey)?;
        if self.material.fman_pubkey.0 != public_key.to_string() {
            return Err(FmanTrustMaterialVerificationError::InvalidFmanPubkey);
        }
        let public_key = public_key
            .xonly()
            .map_err(|_| FmanTrustMaterialVerificationError::InvalidFmanPubkey)?;
        let message = nostr::secp256k1::Message::from_digest(
            self.material
                .digest()
                .map_err(|_| FmanTrustMaterialVerificationError::CanonicalizationFailed)?,
        );

        nostr::SECP256K1
            .verify_schnorr(&self.proof.signature, &message, &public_key)
            .map_err(|_| FmanTrustMaterialVerificationError::InvalidSignature)?;

        Ok(self.material.clone())
    }

    /// Verify this response for an expected FMan and verifier clock.
    ///
    /// This helper verifies envelope signature, response resource bounds,
    /// freshness/expiry, public API URL validation, and FMan ownership of holder
    /// authorizations. Credential cryptography, issuer trust, and revocation
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
    pub fn verify_for_fman(
        &self,
        expected_fman: &Pubkey,
        now: Timestamp,
        max_validity_secs: u64,
    ) -> Result<FmanTrustMaterial, FmanTrustMaterialVerificationError> {
        validate_response_bounds(self)?;
        if &self.material.fman_pubkey != expected_fman {
            return Err(FmanTrustMaterialVerificationError::UnexpectedFman);
        }
        validate_freshness(
            self.material.issued_at,
            self.material.expires_at,
            now,
            max_validity_secs,
        )?;

        FmanApiUrlsMetadata::new(self.material.public_api_urls.clone())
            .map_err(FmanTrustMaterialVerificationError::InvalidPublicApiUrls)?;
        validate_canonical_url_order(&self.material.public_api_urls)
            .map_err(FmanTrustMaterialVerificationError::InvalidPublicApiUrls)?;

        let verified_material = self.verify_envelope_signature()?;
        validate_holder_authorization_subjects(&verified_material)?;

        Ok(verified_material)
    }
}

/// Error returned when verifying a public FMan trust-material response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FmanTrustMaterialVerificationError {
    /// The material's `fman_pubkey` is not a valid canonical Nostr/secp256k1
    /// public key.
    InvalidFmanPubkey,

    /// The trust-material payload could not be canonicalized for verification.
    CanonicalizationFailed,

    /// The Schnorr signature does not verify for the claimed FMan pubkey.
    InvalidSignature,

    /// The response identity is not the FMan selected by consensus metadata.
    UnexpectedFman,

    /// The canonical response is too large.
    ResponseTooLarge,

    /// The response contains too many public API URLs.
    TooManyPublicApiUrls,

    /// The response contains too many holder authorizations.
    TooManyHolderAuthorizations,

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

    /// A holder authorization subject does not equal `material.fman_pubkey`.
    HolderAuthorizationSubjectMismatch,
}

impl core::fmt::Display for FmanTrustMaterialVerificationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidFmanPubkey => f.write_str("invalid FMan pubkey"),
            Self::CanonicalizationFailed => {
                f.write_str("FMan trust material canonicalization failed")
            }
            Self::InvalidSignature => f.write_str("invalid FMan trust material signature"),
            Self::UnexpectedFman => f.write_str("FMan trust material identity mismatch"),
            Self::ResponseTooLarge => f.write_str("FMan trust material response is too large"),
            Self::TooManyPublicApiUrls => {
                f.write_str("FMan trust material response has too many public API URLs")
            }
            Self::TooManyHolderAuthorizations => {
                f.write_str("FMan trust material response has too many holder authorizations")
            }
            Self::InvalidFreshnessWindow => {
                f.write_str("FMan trust material freshness window is invalid")
            }
            Self::IssuedInFuture => f.write_str("FMan trust material was issued in the future"),
            Self::Expired => f.write_str("FMan trust material has expired"),
            Self::ValidityWindowTooLarge => {
                f.write_str("FMan trust material validity window is too large")
            }
            Self::InvalidPublicApiUrls(err) => write!(f, "invalid public FMan API URLs: {err}"),
            Self::HolderAuthorizationSubjectMismatch => {
                f.write_str("holder authorization subject does not match FMan pubkey")
            }
        }
    }
}

impl std::error::Error for FmanTrustMaterialVerificationError {}

/// Validate response size and count bounds.
fn validate_response_bounds(
    response: &GetFmanTrustMaterialResponse,
) -> Result<(), FmanTrustMaterialVerificationError> {
    let response_bytes = serde_json_canonicalizer::to_vec(response)
        .map_err(|_| FmanTrustMaterialVerificationError::CanonicalizationFailed)?;
    if response_bytes.len() > FMAN_TRUST_MATERIAL_MAX_RESPONSE_BYTES {
        return Err(FmanTrustMaterialVerificationError::ResponseTooLarge);
    }
    if response.material.public_api_urls.len() > FMAN_API_URLS_MAX_COUNT {
        return Err(FmanTrustMaterialVerificationError::TooManyPublicApiUrls);
    }
    if response.material.holder_authorizations.len() > FMAN_TRUST_MATERIAL_MAX_HOLDER_AUTHORIZATIONS
    {
        return Err(FmanTrustMaterialVerificationError::TooManyHolderAuthorizations);
    }

    Ok(())
}

/// Validate trust-material issue and expiry timestamps.
fn validate_freshness(
    issued_at: Timestamp,
    expires_at: Timestamp,
    now: Timestamp,
    max_validity_secs: u64,
) -> Result<(), FmanTrustMaterialVerificationError> {
    if expires_at <= issued_at {
        return Err(FmanTrustMaterialVerificationError::InvalidFreshnessWindow);
    }
    if issued_at.0
        > now
            .0
            .saturating_add(FMAN_TRUST_MATERIAL_MAX_FUTURE_SKEW_SECS)
    {
        return Err(FmanTrustMaterialVerificationError::IssuedInFuture);
    }
    if expires_at <= now {
        return Err(FmanTrustMaterialVerificationError::Expired);
    }
    if expires_at.0.saturating_sub(issued_at.0) > max_validity_secs {
        return Err(FmanTrustMaterialVerificationError::ValidityWindowTooLarge);
    }

    Ok(())
}

/// Validate holder authorization subjects against the material owner.
fn validate_holder_authorization_subjects(
    material: &FmanTrustMaterial,
) -> Result<(), FmanTrustMaterialVerificationError> {
    for envelope in &material.holder_authorizations {
        if envelope
            .holder_authorization
            .authorization
            .subject_pubkey
            .0
            .to_string()
            != material.fman_pubkey.0
        {
            return Err(FmanTrustMaterialVerificationError::HolderAuthorizationSubjectMismatch);
        }
    }

    Ok(())
}
