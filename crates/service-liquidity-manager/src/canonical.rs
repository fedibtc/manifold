//! Canonical FLIP payload bytes and domain-tagged hashes.
//!
//! These helpers define protocol payload bytes. They are intentionally separate
//! from transport frame encoding.

use crate::{
    CanonicalPayload, GET_ALLOCATION_STATUS_REQUEST_SIGNATURE_DOMAIN,
    GET_ALLOCATION_STATUS_RESPONSE_SIGNATURE_DOMAIN, GET_PROVIDER_INFO_REQUEST_SIGNATURE_DOMAIN,
    GET_PROVIDER_INFO_RESPONSE_SIGNATURE_DOMAIN, LIQUIDITY_PROVIDER_ADVERTISEMENT_SIGNATURE_DOMAIN,
    LiquidityProviderAdvertisement, REQUEST_LIQUIDITY_DETAILS_HASH_DOMAIN,
    REQUEST_LIQUIDITY_REQUEST_SIGNATURE_DOMAIN, REQUEST_LIQUIDITY_RESPONSE_SIGNATURE_DOMAIN,
    RequestLiquidityDetailsCommitmentV1, RequestLiquidityRequest, Sha256Digest,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Error produced while building canonical payload bytes.
#[derive(Debug)]
pub enum CanonicalEncodingError {
    /// Canonical JSON/JCS serialization failed.
    Json(serde_json::Error),

    /// Canonical CBOR serialization failed.
    Cbor(ciborium::ser::Error<std::io::Error>),
}

impl core::fmt::Display for CanonicalEncodingError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Json(err) => write!(f, "canonical JSON encoding failed: {err}"),
            Self::Cbor(err) => write!(f, "canonical CBOR encoding failed: {err}"),
        }
    }
}

impl std::error::Error for CanonicalEncodingError {}

impl From<serde_json::Error> for CanonicalEncodingError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

impl From<ciborium::ser::Error<std::io::Error>> for CanonicalEncodingError {
    fn from(err: ciborium::ser::Error<std::io::Error>) -> Self {
        Self::Cbor(err)
    }
}

/// Public RPC payload class used to select the signing domain.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PublicRpcPayloadDomain {
    /// `GetProviderInfo` request payload.
    GetProviderInfoRequest,

    /// `GetProviderInfo` response payload.
    GetProviderInfoResponse,

    /// `RequestLiquidity` request payload.
    RequestLiquidityRequest,

    /// `RequestLiquidity` response payload.
    RequestLiquidityResponse,

    /// `GetAllocationStatus` request payload.
    GetAllocationStatusRequest,

    /// `GetAllocationStatus` response payload.
    GetAllocationStatusResponse,
}

impl PublicRpcPayloadDomain {
    /// Return the FLIP domain separator for this RPC payload class.
    #[must_use]
    pub fn domain_separator(self) -> &'static [u8] {
        match self {
            Self::GetProviderInfoRequest => GET_PROVIDER_INFO_REQUEST_SIGNATURE_DOMAIN,
            Self::GetProviderInfoResponse => GET_PROVIDER_INFO_RESPONSE_SIGNATURE_DOMAIN,
            Self::RequestLiquidityRequest => REQUEST_LIQUIDITY_REQUEST_SIGNATURE_DOMAIN,
            Self::RequestLiquidityResponse => REQUEST_LIQUIDITY_RESPONSE_SIGNATURE_DOMAIN,
            Self::GetAllocationStatusRequest => GET_ALLOCATION_STATUS_REQUEST_SIGNATURE_DOMAIN,
            Self::GetAllocationStatusResponse => GET_ALLOCATION_STATUS_RESPONSE_SIGNATURE_DOMAIN,
        }
    }
}

/// Serialize a Nostr-portable payload using canonical JSON/JCS.
pub fn canonical_json_payload<T>(payload: &T) -> Result<CanonicalPayload, CanonicalEncodingError>
where
    T: Serialize,
{
    Ok(CanonicalPayload(serde_json_canonicalizer::to_vec(payload)?))
}

/// Serialize an RPC payload using the FLIP canonical CBOR profile.
///
/// FLIP RPC DTOs are typed structs/enums with stable serde field and variant
/// names. Avoid feeding unordered maps into this helper unless their serializer
/// defines deterministic key order.
pub fn canonical_cbor_payload<T>(payload: &T) -> Result<CanonicalPayload, CanonicalEncodingError>
where
    T: Serialize,
{
    let mut bytes = Vec::new();
    ciborium::into_writer(payload, &mut bytes)?;
    Ok(CanonicalPayload(bytes))
}

/// Compute `SHA256(domain_separator || canonical_payload_bytes)`.
#[must_use]
pub fn domain_tagged_sha256(domain_separator: &[u8], canonical_payload: &[u8]) -> Sha256Digest {
    let digest = Sha256::new()
        .chain_update(domain_separator)
        .chain_update(canonical_payload)
        .finalize();
    Sha256Digest(digest.into())
}

/// Canonical JSON bytes for a provider advertisement payload.
pub fn advertisement_canonical_payload(
    advertisement: &LiquidityProviderAdvertisement,
) -> Result<CanonicalPayload, CanonicalEncodingError> {
    canonical_json_payload(advertisement)
}

/// Hash used for advertisement signatures and `advertisement_hash`.
pub fn advertisement_hash(
    advertisement: &LiquidityProviderAdvertisement,
) -> Result<Sha256Digest, CanonicalEncodingError> {
    let payload = advertisement_canonical_payload(advertisement)?;
    Ok(domain_tagged_sha256(
        LIQUIDITY_PROVIDER_ADVERTISEMENT_SIGNATURE_DOMAIN,
        &payload.0,
    ))
}

/// Canonical CBOR bytes for a public RPC signed payload.
pub fn public_rpc_canonical_payload<T>(
    payload: &T,
) -> Result<CanonicalPayload, CanonicalEncodingError>
where
    T: Serialize,
{
    canonical_cbor_payload(payload)
}

/// Hash used for a public RPC signed payload.
pub fn public_rpc_payload_hash<T>(
    domain: PublicRpcPayloadDomain,
    payload: &T,
) -> Result<Sha256Digest, CanonicalEncodingError>
where
    T: Serialize,
{
    let payload = public_rpc_canonical_payload(payload)?;
    Ok(domain_tagged_sha256(domain.domain_separator(), &payload.0))
}

/// Build the request-scoped details commitment represented by a request.
#[must_use]
pub fn request_liquidity_details_commitment(
    request: &RequestLiquidityRequest,
) -> RequestLiquidityDetailsCommitmentV1 {
    RequestLiquidityDetailsCommitmentV1 {
        version: request.version,
        requester_pubkey: request.requester_pubkey.clone(),
        provider_pubkey: request.provider_pubkey.clone(),
        network: request.network,
        amounts: request.amounts.clone(),
        federation_details: request.federation_details.clone(),
        expires_at: request.expires_at,
    }
}

/// Canonical CBOR bytes for `RequestLiquidityDetailsCommitmentV1`.
pub fn request_liquidity_details_canonical_payload(
    commitment: &RequestLiquidityDetailsCommitmentV1,
) -> Result<CanonicalPayload, CanonicalEncodingError> {
    canonical_cbor_payload(commitment)
}

/// Compute `details_payload_hash`.
pub fn request_liquidity_details_hash(
    commitment: &RequestLiquidityDetailsCommitmentV1,
) -> Result<Sha256Digest, CanonicalEncodingError> {
    let payload = request_liquidity_details_canonical_payload(commitment)?;
    Ok(domain_tagged_sha256(
        REQUEST_LIQUIDITY_DETAILS_HASH_DOMAIN,
        &payload.0,
    ))
}

/// Compute the expected `details_payload_hash` for a `RequestLiquidityRequest`.
pub fn request_liquidity_details_hash_for_request(
    request: &RequestLiquidityRequest,
) -> Result<Sha256Digest, CanonicalEncodingError> {
    request_liquidity_details_hash(&request_liquidity_details_commitment(request))
}
