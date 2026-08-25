//! Schnorr signing and verification over canonical Public Liquidity API
//! payloads.
//!
//! Every public request and response is signed over a domain-separated hash of
//! its canonical payload. Without an imported provider identity the auth
//! provider fails closed, so an unconfigured daemon signs nothing.

use fedi_decentralized_service_liquidity_manager::{
    GetAllocationStatusRequest, GetAllocationStatusResponse, GetProviderInfoRequest,
    GetProviderInfoResponse, LiquidityProviderAdvertisement, PayloadProof, PublicRpcPayloadDomain,
    RequestLiquidityRequest, RequestLiquidityResponse, ServiceResult, Sha256Digest, Signature,
    Signed, advertisement_hash, public_rpc_payload_hash,
};
use nostr_sdk::{
    Keys, PublicKey, SECP256K1,
    secp256k1::{Message, XOnlyPublicKey, schnorr::Signature as SchnorrSignature},
};
use serde::Serialize;
use std::sync::Arc;

use crate::config::DaemonArgs;
use crate::database::Database;
use crate::identity::{self, ProductionProviderIdentity};
use crate::secret_store::SecretStore;
use crate::{failed_precondition, internal_error, invalid_argument, permission_denied};

/// How the public API signs, as reported to an operator.
///
/// Formatted into Admin API health, so the displayed names are a surface an
/// operator reads and are stated explicitly rather than case-converted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, strum::Display)]
pub(crate) enum AuthMode {
    /// A provider signing key is installed and public payloads are signed.
    #[strum(serialize = "schnorr")]
    Schnorr,

    /// No provider signing key is installed; every signing call fails closed.
    #[strum(serialize = "schnorr_unconfigured")]
    SchnorrUnconfigured,
}

/// Operator-visible authentication/proof mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AuthModeInfo {
    /// Stable machine-readable mode name.
    pub mode: AuthMode,

    /// Whether a provider signing key is installed and signing can succeed.
    pub signing_ready: bool,

    /// Operator-readable detail.
    pub detail: &'static str,
}

/// Public payload authentication and provider proof boundary: Schnorr
/// signatures over canonical domain-tagged FLIP payload hashes.
pub(crate) trait PublicAuthProvider: Send + Sync {
    fn mode(&self) -> AuthModeInfo;

    /// Nostr secret key used to publish Nostr-wrapped signed documents.
    fn relay_signing_secret_hex(&self) -> Option<String> {
        None
    }

    fn sign_advertisement(
        &self,
        payload: LiquidityProviderAdvertisement,
    ) -> ServiceResult<Signed<LiquidityProviderAdvertisement>>;

    /// Verify a provider-info request and return its deterministic payload hash.
    fn verify_get_provider_info_request(
        &self,
        signed: &Signed<GetProviderInfoRequest>,
    ) -> ServiceResult<Sha256Digest>;

    fn sign_get_provider_info_response(
        &self,
        payload: GetProviderInfoResponse,
    ) -> ServiceResult<Signed<GetProviderInfoResponse>>;

    /// Verify a liquidity request and return its deterministic payload hash.
    fn verify_request_liquidity_request(
        &self,
        signed: &Signed<RequestLiquidityRequest>,
    ) -> ServiceResult<Sha256Digest>;

    fn sign_request_liquidity_response(
        &self,
        payload: RequestLiquidityResponse,
    ) -> ServiceResult<Signed<RequestLiquidityResponse>>;

    /// Verify an allocation-status request and return its deterministic payload hash.
    fn verify_get_allocation_status_request(
        &self,
        signed: &Signed<GetAllocationStatusRequest>,
    ) -> ServiceResult<Sha256Digest>;

    fn sign_get_allocation_status_response(
        &self,
        payload: GetAllocationStatusResponse,
    ) -> ServiceResult<Signed<GetAllocationStatusResponse>>;
}

/// Builds the public authentication provider from daemon boot args: the
/// imported provider signing key when installed, otherwise the fail-closed
/// unconfigured provider.
pub(crate) async fn provider_from_args(
    database: &Database,
    secret_store: &SecretStore,
    args: &DaemonArgs,
) -> anyhow::Result<Arc<dyn PublicAuthProvider>> {
    match identity::load_or_import_production_provider_identity(
        database,
        secret_store,
        args.provider_nostr_secret_key.as_deref(),
    )
    .await?
    {
        Some(identity) => Ok(Arc::new(SchnorrAuthProvider::new(identity)?)),
        None => Ok(Arc::new(UnconfiguredAuthProvider)),
    }
}

/// Schnorr proof implementation over the imported provider signing key.
#[derive(Clone, Debug)]
pub(crate) struct SchnorrAuthProvider {
    provider_pubkey: fedi_decentralized_service_liquidity_manager::Pubkey,
    nostr_secret_key_hex: String,
    keys: Keys,
}

impl SchnorrAuthProvider {
    pub(crate) fn new(identity: ProductionProviderIdentity) -> anyhow::Result<Self> {
        let keys = Keys::parse(&identity.nostr_secret_key_hex)?;
        Ok(Self {
            provider_pubkey: identity.provider_pubkey,
            nostr_secret_key_hex: keys.secret_key().to_secret_hex(),
            keys,
        })
    }

    fn sign_hash(&self, hash: Sha256Digest) -> ServiceResult<PayloadProof> {
        let message = Message::from_digest(hash.0);
        Ok(PayloadProof {
            signature: Signature(self.keys.sign_schnorr(&message).serialize().to_vec()),
        })
    }

    fn verify_rpc<T>(
        &self,
        domain: PublicRpcPayloadDomain,
        signed: &Signed<T>,
        signer: &fedi_decentralized_service_liquidity_manager::Pubkey,
    ) -> ServiceResult<Sha256Digest>
    where
        T: Serialize,
    {
        let expected = public_rpc_payload_hash(domain, &signed.payload).map_err(internal_error)?;
        verify_schnorr(signer, &signed.proof.signature, expected)?;
        Ok(expected)
    }

    fn sign_rpc<T>(&self, domain: PublicRpcPayloadDomain, payload: T) -> ServiceResult<Signed<T>>
    where
        T: Serialize,
    {
        let payload_hash = public_rpc_payload_hash(domain, &payload).map_err(internal_error)?;
        Ok(Signed {
            payload,
            proof: self.sign_hash(payload_hash)?,
        })
    }

    fn ensure_provider_recipient(
        &self,
        provider_pubkey: &fedi_decentralized_service_liquidity_manager::Pubkey,
    ) -> ServiceResult<()> {
        if provider_pubkey == &self.provider_pubkey {
            Ok(())
        } else {
            Err(permission_denied(
                "signed payload targets a different provider",
            ))
        }
    }
}

impl PublicAuthProvider for SchnorrAuthProvider {
    fn mode(&self) -> AuthModeInfo {
        AuthModeInfo {
            mode: AuthMode::Schnorr,
            signing_ready: true,
            detail: "Schnorr signatures over canonical FLIP payload hashes; full requester endpoint actor-binding remains a tracked open item",
        }
    }

    fn relay_signing_secret_hex(&self) -> Option<String> {
        Some(self.nostr_secret_key_hex.clone())
    }

    fn sign_advertisement(
        &self,
        payload: LiquidityProviderAdvertisement,
    ) -> ServiceResult<Signed<LiquidityProviderAdvertisement>> {
        self.ensure_provider_recipient(&payload.provider_pubkey)?;
        let hash = advertisement_hash(&payload).map_err(internal_error)?;
        Ok(Signed {
            payload,
            proof: self.sign_hash(hash)?,
        })
    }

    fn verify_get_provider_info_request(
        &self,
        signed: &Signed<GetProviderInfoRequest>,
    ) -> ServiceResult<Sha256Digest> {
        self.ensure_provider_recipient(&signed.payload.provider_pubkey)?;
        self.verify_rpc(
            PublicRpcPayloadDomain::GetProviderInfoRequest,
            signed,
            &signed.payload.requester_pubkey,
        )
    }

    fn sign_get_provider_info_response(
        &self,
        payload: GetProviderInfoResponse,
    ) -> ServiceResult<Signed<GetProviderInfoResponse>> {
        self.ensure_provider_recipient(&payload.provider_pubkey)?;
        self.sign_rpc(PublicRpcPayloadDomain::GetProviderInfoResponse, payload)
    }

    fn verify_request_liquidity_request(
        &self,
        signed: &Signed<RequestLiquidityRequest>,
    ) -> ServiceResult<Sha256Digest> {
        self.ensure_provider_recipient(&signed.payload.provider_pubkey)?;
        self.verify_rpc(
            PublicRpcPayloadDomain::RequestLiquidityRequest,
            signed,
            &signed.payload.requester_pubkey,
        )
    }

    fn sign_request_liquidity_response(
        &self,
        payload: RequestLiquidityResponse,
    ) -> ServiceResult<Signed<RequestLiquidityResponse>> {
        self.ensure_provider_recipient(&payload.provider_pubkey)?;
        self.sign_rpc(PublicRpcPayloadDomain::RequestLiquidityResponse, payload)
    }

    fn verify_get_allocation_status_request(
        &self,
        signed: &Signed<GetAllocationStatusRequest>,
    ) -> ServiceResult<Sha256Digest> {
        self.ensure_provider_recipient(&signed.payload.provider_pubkey)?;
        self.verify_rpc(
            PublicRpcPayloadDomain::GetAllocationStatusRequest,
            signed,
            &signed.payload.requester_pubkey,
        )
    }

    fn sign_get_allocation_status_response(
        &self,
        payload: GetAllocationStatusResponse,
    ) -> ServiceResult<Signed<GetAllocationStatusResponse>> {
        self.ensure_provider_recipient(&payload.provider_pubkey)?;
        self.sign_rpc(PublicRpcPayloadDomain::GetAllocationStatusResponse, payload)
    }
}

/// No provider signing key has been installed yet; every operation fails
/// closed.
#[derive(Clone, Debug, Default)]
pub(crate) struct UnconfiguredAuthProvider;

impl PublicAuthProvider for UnconfiguredAuthProvider {
    fn mode(&self) -> AuthModeInfo {
        AuthModeInfo {
            mode: AuthMode::SchnorrUnconfigured,
            signing_ready: false,
            detail: "no provider signing key is installed",
        }
    }

    fn sign_advertisement(
        &self,
        _payload: LiquidityProviderAdvertisement,
    ) -> ServiceResult<Signed<LiquidityProviderAdvertisement>> {
        Err(failed_precondition("provider signing key is not installed"))
    }

    fn verify_get_provider_info_request(
        &self,
        _signed: &Signed<GetProviderInfoRequest>,
    ) -> ServiceResult<Sha256Digest> {
        Err(failed_precondition("provider signing key is not installed"))
    }

    fn sign_get_provider_info_response(
        &self,
        _payload: GetProviderInfoResponse,
    ) -> ServiceResult<Signed<GetProviderInfoResponse>> {
        Err(failed_precondition("provider signing key is not installed"))
    }

    fn verify_request_liquidity_request(
        &self,
        _signed: &Signed<RequestLiquidityRequest>,
    ) -> ServiceResult<Sha256Digest> {
        Err(failed_precondition("provider signing key is not installed"))
    }

    fn sign_request_liquidity_response(
        &self,
        _payload: RequestLiquidityResponse,
    ) -> ServiceResult<Signed<RequestLiquidityResponse>> {
        Err(failed_precondition("provider signing key is not installed"))
    }

    fn verify_get_allocation_status_request(
        &self,
        _signed: &Signed<GetAllocationStatusRequest>,
    ) -> ServiceResult<Sha256Digest> {
        Err(failed_precondition("provider signing key is not installed"))
    }

    fn sign_get_allocation_status_response(
        &self,
        _payload: GetAllocationStatusResponse,
    ) -> ServiceResult<Signed<GetAllocationStatusResponse>> {
        Err(failed_precondition("provider signing key is not installed"))
    }
}

fn verify_schnorr(
    signer: &fedi_decentralized_service_liquidity_manager::Pubkey,
    signature: &Signature,
    payload_hash: Sha256Digest,
) -> ServiceResult<()> {
    let public_key = canonical_public_key(signer)?;
    let signature = SchnorrSignature::from_slice(&signature.0)
        .map_err(|_| invalid_argument("signature is not a valid Schnorr signature"))?;
    let message = Message::from_digest(payload_hash.0);
    SECP256K1
        .verify_schnorr(&signature, &message, &public_key)
        .map_err(|_| invalid_argument("signature does not verify for signed payload"))
}

fn canonical_public_key(
    pubkey: &fedi_decentralized_service_liquidity_manager::Pubkey,
) -> ServiceResult<XOnlyPublicKey> {
    let public_key = PublicKey::parse(&pubkey.0)
        .map_err(|_| invalid_argument("pubkey is not a valid Nostr public key"))?;
    if pubkey.0 != public_key.to_string() {
        return Err(invalid_argument(
            "pubkey must be canonical lowercase Nostr hex",
        ));
    }
    public_key
        .xonly()
        .map_err(|_| invalid_argument("pubkey is not a valid x-only public key"))
}

#[cfg(test)]
#[path = "../tests/auth.rs"]
mod tests;
