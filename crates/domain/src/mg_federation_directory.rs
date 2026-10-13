//! Federation-directory and public MG trust-material protocol types.

#[cfg(test)]
mod tests;

use serde::de::Error as _;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    HolderAuthorizationEnvelope, ProtocolV1, Pubkey, SchnorrSignatureProof, Timestamp, Url,
};

/// Fedimint consensus metadata key containing public MG API URLs.
pub const MG_API_URLS_META_FIELD_KEY: &str = "fedi:fman_api_urls";

/// Canonical payload type string for public MG trust-material responses.
pub const MG_TRUST_MATERIAL_CANONICAL_TYPE: &str = "fedi.fman.trust-material";

/// Signature domain for public MG trust-material responses.
pub const MG_TRUST_MATERIAL_SIGNATURE_DOMAIN_SEPARATOR: &[u8] = b"fedi-fman-trust-material/v1\0";

/// Maximum number of MG public API URLs accepted in federation metadata.
pub const MG_API_URLS_MAX_COUNT: usize = 64;

/// Maximum canonical JSON value length accepted for `fedi:fman_api_urls`.
pub const MG_API_URLS_MAX_VALUE_BYTES: usize = 8192;

/// Maximum individual public MG API URL length accepted in federation metadata.
pub const MG_API_URL_MAX_BYTES: usize = 512;

/// Maximum canonical public trust-material response size accepted by verifiers.
pub const MG_TRUST_MATERIAL_MAX_RESPONSE_BYTES: usize = 131_072;

/// Maximum holder authorizations accepted in one public trust-material response.
///
/// Each authorization carries its own backing credential, so this bounds both.
pub const MG_TRUST_MATERIAL_MAX_HOLDER_AUTHORIZATIONS: usize = 64;

/// Maximum future clock skew accepted for a Holder authorization statement.
pub const MG_HOLDER_AUTHORIZATION_MAX_FUTURE_SKEW_SECS: u64 = 3600;

/// Maximum future clock skew accepted for signed trust-material responses.
pub const MG_TRUST_MATERIAL_MAX_FUTURE_SKEW_SECS: u64 = 3600;

/// Canonical Fedimint metadata directory value for public MG API discovery.
///
/// The value is discovery data only. Verifiers must treat every URL as an
/// untrusted locator, fetch signed trust material from MGs, and verify the
/// returned evidence locally before making policy decisions.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct MgApiUrlsMetadata {
    /// Protocol version for this metadata value.
    version: ProtocolV1,

    /// Sorted and deduplicated public MG API URLs.
    #[serde(rename = "fman_api_urls")]
    mg_api_urls: Vec<Url>,
}

impl MgApiUrlsMetadata {
    /// Build validated metadata from an arbitrary URL list.
    ///
    /// URLs are validated, sorted, deduplicated, and then canonicalized. The
    /// current MVP accepts only stable `iroh://...` URLs.
    ///
    /// # Errors
    ///
    /// Returns an error if URL validation, count limits, or serialized value
    /// limits fail.
    pub fn new(urls: impl IntoIterator<Item = Url>) -> Result<Self, MgApiUrlsMetadataError> {
        let mut urls = urls.into_iter().collect::<Vec<_>>();
        for url in &urls {
            validate_mg_api_url(url)?;
        }
        urls.sort();
        urls.dedup();
        if urls.is_empty() {
            return Err(MgApiUrlsMetadataError::Empty);
        }
        if urls.len() > MG_API_URLS_MAX_COUNT {
            return Err(MgApiUrlsMetadataError::TooManyUrls);
        }

        let metadata = Self {
            version: ProtocolV1,
            mg_api_urls: urls,
        };
        let canonical = metadata.canonical_bytes()?;
        if canonical.len() > MG_API_URLS_MAX_VALUE_BYTES {
            return Err(MgApiUrlsMetadataError::ValueTooLarge);
        }

        Ok(metadata)
    }

    /// Return the protocol version for this metadata value.
    #[must_use]
    pub fn version(&self) -> ProtocolV1 {
        self.version
    }

    /// Return the validated public MG API URLs.
    #[must_use]
    pub fn mg_api_urls(&self) -> &[Url] {
        &self.mg_api_urls
    }

    /// Parse and validate a canonical metadata value.
    ///
    /// # Errors
    ///
    /// Returns an error if JSON parsing fails, the value is not in canonical
    /// form, or validation fails.
    pub fn parse_canonical(value: &str) -> Result<Self, MgApiUrlsMetadataError> {
        if value.len() > MG_API_URLS_MAX_VALUE_BYTES {
            return Err(MgApiUrlsMetadataError::ValueTooLarge);
        }
        let parsed: RawMgApiUrlsMetadata =
            serde_json::from_str(value).map_err(|_| MgApiUrlsMetadataError::MalformedJson)?;
        let normalized = Self::new(parsed.mg_api_urls)?;
        if normalized.canonical_string()? != value {
            return Err(MgApiUrlsMetadataError::NonCanonical);
        }

        Ok(normalized)
    }

    /// Serialize this value as canonical JSON bytes suitable for Fedimint metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MgApiUrlsMetadataError> {
        serde_json_canonicalizer::to_vec(self)
            .map_err(|_| MgApiUrlsMetadataError::CanonicalizationFailed)
    }

    /// Serialize this value as a canonical JSON string.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails or produces
    /// non-UTF-8 bytes.
    pub fn canonical_string(&self) -> Result<String, MgApiUrlsMetadataError> {
        String::from_utf8(self.canonical_bytes()?)
            .map_err(|_| MgApiUrlsMetadataError::CanonicalizationFailed)
    }
}

impl<'de> serde::Deserialize<'de> for MgApiUrlsMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawMgApiUrlsMetadata::deserialize(deserializer)?;
        let _version = raw.version;
        validate_canonical_url_order(&raw.mg_api_urls).map_err(D::Error::custom)?;
        let metadata = Self::new(raw.mg_api_urls).map_err(D::Error::custom)?;

        Ok(metadata)
    }
}

/// Raw serde shape for `MgApiUrlsMetadata` before invariant validation.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMgApiUrlsMetadata {
    /// Protocol version for this metadata value.
    version: ProtocolV1,

    /// Raw public MG API URLs.
    #[serde(rename = "fman_api_urls")]
    mg_api_urls: Vec<Url>,
}

/// Error returned when validating the `fedi:fman_api_urls` metadata value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MgApiUrlsMetadataError {
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

impl core::fmt::Display for MgApiUrlsMetadataError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => f.write_str("MG API URL metadata is empty"),
            Self::TooManyUrls => f.write_str("MG API URL metadata has too many URLs"),
            Self::ValueTooLarge => f.write_str("MG API URL metadata value is too large"),
            Self::MalformedJson => f.write_str("MG API URL metadata JSON is malformed"),
            Self::NonCanonical => f.write_str("MG API URL metadata is not canonical"),
            Self::CanonicalizationFailed => {
                f.write_str("MG API URL metadata canonicalization failed")
            }
            Self::EmptyUrl => f.write_str("MG API URL is empty"),
            Self::UrlTooLong => f.write_str("MG API URL is too long"),
            Self::UrlContainsControlCharacter => {
                f.write_str("MG API URL contains a control character")
            }
            Self::UnsupportedUrlScheme => f.write_str("MG API URL scheme is unsupported"),
            Self::MissingEndpoint => f.write_str("MG API URL is missing endpoint material"),
        }
    }
}

impl std::error::Error for MgApiUrlsMetadataError {}

/// Validate one public MG API URL for the metadata directory.
///
/// The MVP intentionally accepts only `iroh://...` locators and requires them
/// to be stable/restorable for the MG service identity.
///
/// # Errors
///
/// Returns an error if the URL is empty, too large, not an Iroh URL, or lacks an
/// endpoint body.
pub fn validate_mg_api_url(url: &Url) -> Result<(), MgApiUrlsMetadataError> {
    let value = url.0.as_str();
    if value.is_empty() {
        return Err(MgApiUrlsMetadataError::EmptyUrl);
    }
    if value.len() > MG_API_URL_MAX_BYTES {
        return Err(MgApiUrlsMetadataError::UrlTooLong);
    }
    if value.chars().any(char::is_control) {
        return Err(MgApiUrlsMetadataError::UrlContainsControlCharacter);
    }
    let Some(endpoint) = value.strip_prefix("iroh://") else {
        return Err(MgApiUrlsMetadataError::UnsupportedUrlScheme);
    };
    if endpoint.is_empty() {
        return Err(MgApiUrlsMetadataError::MissingEndpoint);
    }

    Ok(())
}

/// Validate that a URL list is already sorted and deduplicated.
fn validate_canonical_url_order(urls: &[Url]) -> Result<(), MgApiUrlsMetadataError> {
    let mut sorted = urls.to_vec();
    sorted.sort();
    sorted.dedup();
    if sorted != urls {
        return Err(MgApiUrlsMetadataError::NonCanonical);
    }

    Ok(())
}

/// Public request for an MG's current trust material.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetMgTrustMaterialRequest {
    /// Protocol version for this request shape.
    pub version: ProtocolV1,
}

/// Current MG trust material signed by the MG service key.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct MgTrustMaterial {
    /// MG service/advertisement pubkey that signs this response.
    #[serde(rename = "fman_pubkey")]
    pub mg_pubkey: Pubkey,

    /// Response issue timestamp.
    pub issued_at: Timestamp,

    /// Response freshness limit.
    pub expires_at: Timestamp,

    /// Current public MG API URLs for this service identity.
    pub public_api_urls: Vec<Url>,

    /// Holder authorizations binding holder-owned badges to this MG identity,
    /// each carried together with the credential backing it.
    ///
    /// The authorization and its backing credential travel as one
    /// [`HolderAuthorizationEnvelope`] rather than as two parallel lists.
    /// Parallel lists make an authorization with no matching credential
    /// representable, so every consumer would have to re-pair them by digest and
    /// decide what an unpairable entry means; the envelope makes that state
    /// unrepresentable instead. The MG side already holds envelopes — its
    /// advertisement loop embeds exactly this type — so splitting them here
    /// would be an un-pairing performed only to be undone.
    pub holder_authorizations: Vec<HolderAuthorizationEnvelope>,
}

impl MgTrustMaterial {
    /// Build JCS canonical bytes for this public trust-material payload.
    ///
    /// # Errors
    ///
    /// Returns an error if canonical JSON serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let payload = json!({
            "type": MG_TRUST_MATERIAL_CANONICAL_TYPE,
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
            .chain_update(MG_TRUST_MATERIAL_SIGNATURE_DOMAIN_SEPARATOR)
            .chain_update(self.canonical_bytes()?)
            .finalize();

        Ok(digest.into())
    }
}

/// Signed response returned by the public MG trust-material API.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetMgTrustMaterialResponse {
    /// Protocol version for this response shape.
    pub version: ProtocolV1,

    /// MG trust material payload.
    pub material: MgTrustMaterial,

    /// Schnorr signature by `material.fman_pubkey` over the canonical payload.
    pub proof: SchnorrSignatureProof,
}

impl GetMgTrustMaterialResponse {
    /// Verify this response's MG service-key envelope signature.
    ///
    /// This verifies only the outer signed public trust-material envelope.
    /// Prefer [`Self::verify_for_mg`] when evaluating freshly fetched material.
    ///
    /// # Errors
    ///
    /// Returns an error when the MG pubkey is malformed, canonicalization
    /// fails, or the Schnorr signature does not verify.
    pub fn verify_envelope_signature(
        &self,
    ) -> Result<MgTrustMaterial, MgTrustMaterialVerificationError> {
        let public_key = nostr::PublicKey::parse(&self.material.mg_pubkey.0)
            .map_err(|_| MgTrustMaterialVerificationError::InvalidMgPubkey)?;
        if self.material.mg_pubkey.0 != public_key.to_string() {
            return Err(MgTrustMaterialVerificationError::InvalidMgPubkey);
        }
        let public_key = public_key
            .xonly()
            .map_err(|_| MgTrustMaterialVerificationError::InvalidMgPubkey)?;
        let message = nostr::secp256k1::Message::from_digest(
            self.material
                .digest()
                .map_err(|_| MgTrustMaterialVerificationError::CanonicalizationFailed)?,
        );

        nostr::SECP256K1
            .verify_schnorr(&self.proof.signature, &message, &public_key)
            .map_err(|_| MgTrustMaterialVerificationError::InvalidSignature)?;

        Ok(self.material.clone())
    }

    /// Verify this response for an expected MG and verifier clock.
    ///
    /// This helper verifies envelope signature, response resource bounds,
    /// freshness/expiry, public API URL validation, and MG ownership of holder
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
    pub fn verify_for_mg(
        &self,
        expected_mg: &Pubkey,
        now: Timestamp,
        max_validity_secs: u64,
    ) -> Result<MgTrustMaterial, MgTrustMaterialVerificationError> {
        validate_response_bounds(self)?;
        if &self.material.mg_pubkey != expected_mg {
            return Err(MgTrustMaterialVerificationError::UnexpectedMg);
        }
        validate_freshness(
            self.material.issued_at,
            self.material.expires_at,
            now,
            max_validity_secs,
        )?;

        MgApiUrlsMetadata::new(self.material.public_api_urls.clone())
            .map_err(MgTrustMaterialVerificationError::InvalidPublicApiUrls)?;
        validate_canonical_url_order(&self.material.public_api_urls)
            .map_err(MgTrustMaterialVerificationError::InvalidPublicApiUrls)?;

        let verified_material = self.verify_envelope_signature()?;
        validate_holder_authorization_subjects(&verified_material)?;

        Ok(verified_material)
    }
}

/// Error returned when verifying a public MG trust-material response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MgTrustMaterialVerificationError {
    /// The material's `fman_pubkey` is not a valid canonical Nostr/secp256k1
    /// public key.
    InvalidMgPubkey,

    /// The trust-material payload could not be canonicalized for verification.
    CanonicalizationFailed,

    /// The Schnorr signature does not verify for the claimed MG pubkey.
    InvalidSignature,

    /// The response identity is not the MG selected by consensus metadata.
    UnexpectedMg,

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
    InvalidPublicApiUrls(MgApiUrlsMetadataError),

    /// A holder authorization subject does not equal `material.fman_pubkey`.
    HolderAuthorizationSubjectMismatch,
}

impl core::fmt::Display for MgTrustMaterialVerificationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidMgPubkey => f.write_str("invalid MG pubkey"),
            Self::CanonicalizationFailed => {
                f.write_str("MG trust material canonicalization failed")
            }
            Self::InvalidSignature => f.write_str("invalid MG trust material signature"),
            Self::UnexpectedMg => f.write_str("MG trust material identity mismatch"),
            Self::ResponseTooLarge => f.write_str("MG trust material response is too large"),
            Self::TooManyPublicApiUrls => {
                f.write_str("MG trust material response has too many public API URLs")
            }
            Self::TooManyHolderAuthorizations => {
                f.write_str("MG trust material response has too many holder authorizations")
            }
            Self::InvalidFreshnessWindow => {
                f.write_str("MG trust material freshness window is invalid")
            }
            Self::IssuedInFuture => f.write_str("MG trust material was issued in the future"),
            Self::Expired => f.write_str("MG trust material has expired"),
            Self::ValidityWindowTooLarge => {
                f.write_str("MG trust material validity window is too large")
            }
            Self::InvalidPublicApiUrls(err) => write!(f, "invalid public MG API URLs: {err}"),
            Self::HolderAuthorizationSubjectMismatch => {
                f.write_str("holder authorization subject does not match MG pubkey")
            }
        }
    }
}

impl std::error::Error for MgTrustMaterialVerificationError {}

/// Validate response size and count bounds.
fn validate_response_bounds(
    response: &GetMgTrustMaterialResponse,
) -> Result<(), MgTrustMaterialVerificationError> {
    let response_bytes = serde_json_canonicalizer::to_vec(response)
        .map_err(|_| MgTrustMaterialVerificationError::CanonicalizationFailed)?;
    if response_bytes.len() > MG_TRUST_MATERIAL_MAX_RESPONSE_BYTES {
        return Err(MgTrustMaterialVerificationError::ResponseTooLarge);
    }
    if response.material.public_api_urls.len() > MG_API_URLS_MAX_COUNT {
        return Err(MgTrustMaterialVerificationError::TooManyPublicApiUrls);
    }
    if response.material.holder_authorizations.len() > MG_TRUST_MATERIAL_MAX_HOLDER_AUTHORIZATIONS {
        return Err(MgTrustMaterialVerificationError::TooManyHolderAuthorizations);
    }

    Ok(())
}

/// Validate trust-material issue and expiry timestamps.
fn validate_freshness(
    issued_at: Timestamp,
    expires_at: Timestamp,
    now: Timestamp,
    max_validity_secs: u64,
) -> Result<(), MgTrustMaterialVerificationError> {
    if expires_at <= issued_at {
        return Err(MgTrustMaterialVerificationError::InvalidFreshnessWindow);
    }
    if issued_at.0 > now.0.saturating_add(MG_TRUST_MATERIAL_MAX_FUTURE_SKEW_SECS) {
        return Err(MgTrustMaterialVerificationError::IssuedInFuture);
    }
    if expires_at <= now {
        return Err(MgTrustMaterialVerificationError::Expired);
    }
    if expires_at.0.saturating_sub(issued_at.0) > max_validity_secs {
        return Err(MgTrustMaterialVerificationError::ValidityWindowTooLarge);
    }

    Ok(())
}

/// Validate holder authorization subjects against the material owner.
fn validate_holder_authorization_subjects(
    material: &MgTrustMaterial,
) -> Result<(), MgTrustMaterialVerificationError> {
    for envelope in &material.holder_authorizations {
        if envelope
            .holder_authorization
            .authorization
            .subject_pubkey
            .0
            .to_string()
            != material.mg_pubkey.0
        {
            return Err(MgTrustMaterialVerificationError::HolderAuthorizationSubjectMismatch);
        }
    }

    Ok(())
}
