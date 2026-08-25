use super::*;
use fedi_decentralized_service_liquidity_manager::ServiceErrorCode;
use fedi_decentralized_service_liquidity_manager::{
    GetProviderInfoRequest, ProtocolVersion, Pubkey, Timestamp,
};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn boot_without_key_is_unconfigured_and_fails_closed() -> anyhow::Result<()> {
    let data_dir = test_data_dir("auth-unconfigured");
    let sqlite_path = data_dir.join("flip.sqlite");
    let database = Database::connect(&sqlite_path).await?;
    let secret_store = SecretStore::load_or_create(
        data_dir.join("secret-store.key"),
        Some(&SecretStore::generate_hex_key()),
    )?;
    let args = daemon_args(data_dir, sqlite_path, None)?;

    let provider = provider_from_args(&database, &secret_store, &args).await?;
    let mode = provider.mode();
    assert_eq!(mode.mode, AuthMode::SchnorrUnconfigured);
    assert_eq!(mode.mode.to_string(), "schnorr_unconfigured");
    assert!(!mode.signing_ready);

    let error = provider
        .sign_advertisement(advertisement_payload(Pubkey("provider-pubkey".to_owned())))
        .expect_err("boot without key must refuse signing");
    assert_eq!(error.code(), ServiceErrorCode::FailedPrecondition);

    Ok(())
}

#[test]
fn schnorr_auth_rejects_deterministic_hash_proof() -> anyhow::Result<()> {
    let provider_keys = Keys::generate();
    let requester_keys = Keys::generate();
    let provider = schnorr_provider(&provider_keys)?;
    let request = provider_info_request(&provider_keys, &requester_keys);
    // A payload "signed" with its own deterministic hash (the deleted
    // development-mode proof shape) must never verify as Schnorr.
    let hash = public_rpc_payload_hash(PublicRpcPayloadDomain::GetProviderInfoRequest, &request)?;
    let signed = Signed {
        payload: request,
        proof: PayloadProof {
            signature: Signature(hash.0.to_vec()),
        },
    };

    let error = provider
        .verify_get_provider_info_request(&signed)
        .expect_err("deterministic hash proof must not verify as Schnorr");

    assert_eq!(error.code(), ServiceErrorCode::InvalidArgument);
    Ok(())
}

#[test]
fn production_auth_verifies_requester_signature_and_rejects_wrong_domain() -> anyhow::Result<()> {
    let provider_keys = Keys::generate();
    let requester_keys = Keys::generate();
    let provider = schnorr_provider(&provider_keys)?;
    let request = provider_info_request(&provider_keys, &requester_keys);

    let signed = sign_rpc_with_keys(
        PublicRpcPayloadDomain::GetProviderInfoRequest,
        request.clone(),
        &requester_keys,
    )?;
    provider.verify_get_provider_info_request(&signed)?;

    let wrong_domain = sign_rpc_with_keys(
        PublicRpcPayloadDomain::GetProviderInfoResponse,
        request,
        &requester_keys,
    )?;
    let error = provider
        .verify_get_provider_info_request(&wrong_domain)
        .expect_err("signature under wrong domain must fail");
    assert_eq!(error.code(), ServiceErrorCode::InvalidArgument);
    Ok(())
}

#[test]
fn production_auth_signs_provider_responses() -> anyhow::Result<()> {
    let provider_keys = Keys::generate();
    let requester_keys = Keys::generate();
    let provider = schnorr_provider(&provider_keys)?;
    let response = GetProviderInfoResponse {
        version: ProtocolVersion(1),
        provider_pubkey: Pubkey(provider_keys.public_key().to_hex()),
        issued_at: Timestamp(1_700_000_001),
        advertisement_hash: Sha256Digest([1; 32]),
        supported_sources: Vec::new(),
        policy: fedi_decentralized_service_liquidity_manager::ProviderPolicy {
            accepted_attester_policies: Vec::new(),
            supported_networks: Vec::new(),
        },
        api_endpoint_id: fedi_decentralized_service_liquidity_manager::RpcEndpointId(
            "endpoint-1".to_owned(),
        ),
        api_version: ProtocolVersion(1),
        outcome: fedi_decentralized_service_liquidity_manager::ProviderInfoOutcome::Available,
    };

    let signed = provider.sign_get_provider_info_response(response)?;
    let hash = public_rpc_payload_hash(
        PublicRpcPayloadDomain::GetProviderInfoResponse,
        &signed.payload,
    )?;

    verify_schnorr(
        &Pubkey(provider_keys.public_key().to_hex()),
        &signed.proof.signature,
        hash,
    )?;

    let requester = Pubkey(requester_keys.public_key().to_hex());
    let error = verify_schnorr(&requester, &signed.proof.signature, hash)
        .expect_err("provider signature must not verify under requester key");
    assert_eq!(error.code(), ServiceErrorCode::InvalidArgument);
    Ok(())
}

fn schnorr_provider(keys: &Keys) -> anyhow::Result<SchnorrAuthProvider> {
    SchnorrAuthProvider::new(identity::production_identity_from_secret(
        &keys.secret_key().to_secret_hex(),
    )?)
}

fn daemon_args(
    data_dir: PathBuf,
    sqlite_path: PathBuf,
    provider_nostr_secret_key: Option<String>,
) -> anyhow::Result<DaemonArgs> {
    Ok(DaemonArgs {
        manifold_environment:
            fedi_decentralized_manifold_environment::ManifoldEnvironment::Development,
        data_dir,
        sqlite_path,
        admin_bind_address: "127.0.0.1:0".parse()?,
        public_bind_address: "127.0.0.1:0".parse()?,
        bootstrap_admin_token: Some("test-admin-token".to_owned()),
        secret_store_key: Some(SecretStore::generate_hex_key()),
        allow_bootstrap_token_fallback: false,
        mode: crate::config::DaemonMode::Normal,
        provider_nostr_secret_key,
        trust_fixtures_dir: None,
        max_open_target_clients: crate::target_fedimint::DEFAULT_MAX_OPEN_TARGET_CLIENTS,
        allow_private_federation_endpoints: false,
    })
}

fn advertisement_payload(provider_pubkey: Pubkey) -> LiquidityProviderAdvertisement {
    LiquidityProviderAdvertisement {
        version: ProtocolVersion(1),
        provider_pubkey,
        issued_at: Timestamp(1_700_000_000),
        expires_at: Timestamp(1_700_000_600),
        supported_sources: Vec::new(),
        holder_authorizations: Vec::new(),
        policy: fedi_decentralized_service_liquidity_manager::ProviderPolicy {
            accepted_attester_policies: Vec::new(),
            supported_networks: Vec::new(),
        },
        display: None,
        api_endpoints: Vec::new(),
        api_versions: vec![ProtocolVersion(1)],
        relay_hints: Vec::new(),
    }
}

fn test_data_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("flip-auth-{name}-{nanos}"))
}

fn provider_info_request(provider_keys: &Keys, requester_keys: &Keys) -> GetProviderInfoRequest {
    GetProviderInfoRequest {
        version: ProtocolVersion(1),
        requester_pubkey: Pubkey(requester_keys.public_key().to_hex()),
        provider_pubkey: Pubkey(provider_keys.public_key().to_hex()),
        issued_at: Timestamp(1_700_000_000),
        advertisement_hash: Sha256Digest([1; 32]),
        client_supported_versions: vec![ProtocolVersion(1)],
    }
}

fn sign_rpc_with_keys<T>(
    domain: PublicRpcPayloadDomain,
    payload: T,
    keys: &Keys,
) -> anyhow::Result<Signed<T>>
where
    T: Serialize,
{
    let hash = public_rpc_payload_hash(domain, &payload)?;
    let message = Message::from_digest(hash.0);
    Ok(Signed {
        payload,
        proof: PayloadProof {
            signature: Signature(keys.sign_schnorr(&message).serialize().to_vec()),
        },
    })
}
