//! Credential SDK fixtures used only by external integration-test crates.

use fedi_credential_sdk_protocol::{
    HolderAuthorization, HolderAuthorizationRequest, HolderContext, IssuerAuthority, IssuerContext,
    IssuerSecretKeys, PendingIssuance, SignedCredential, SubjectPubkey,
};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_service_liquidity_manager::{AttestationPayload, Pubkey};
use serde::Serialize;
use serde_json::json;

pub fn test_issuer_context() -> IssuerContext {
    IssuerContext::import_secret_key(&test_issuer_secret_keys())
        .expect("fixed integration-test issuer secret keys import")
}

fn test_issuer_secret_keys() -> IssuerSecretKeys {
    serde_json::from_str(include_str!("../fixtures/issuer-secret-keys.json"))
        .expect("fixed integration-test issuer keys deserialize")
}

pub fn test_issuer_authority(
    issuer: &IssuerContext,
    revocation_relay_url: &str,
) -> anyhow::Result<IssuerAuthority> {
    Ok(
        issuer.issuer_authority(vec![fedi_credential_sdk_protocol::RevocationLocation {
            protocol: "nostr".to_owned(),
            location: revocation_relay_url.to_owned(),
        }])?,
    )
}

pub fn issue_credential_for_holder(
    issuer: &IssuerContext,
    authority: &IssuerAuthority,
    holder: &HolderContext,
) -> anyhow::Result<SignedCredential> {
    let minimum_trust_level = ManifoldEnvironment::Development
        .profile()?
        .minimum_peer_badge_trust_level();
    let credential_info = json!({
        "schema": "fedi-trust-score-v1.0",
        "trust_level": minimum_trust_level,
    });
    let (request, pending) = PendingIssuance::create_request(
        &authority.issuer.issuance_key,
        authority.issuer.issuer_id_pubkey.clone(),
        credential_info.clone(),
        json!(holder.public_key().to_string()),
    )?;
    let response = issuer.issue_credential(credential_info, &request)?;
    Ok(pending.finalize(&authority.issuer.issuance_key, &response)?)
}

pub fn holder_authorization_for_provider(
    holder: &HolderContext,
    credential: &SignedCredential,
    provider_pubkey: &Pubkey,
) -> anyhow::Result<HolderAuthorization> {
    Ok(holder.authorize_credential_use(
        HolderAuthorizationRequest {
            subject_pubkey: provider_pubkey.0.parse::<SubjectPubkey>()?,
        },
        credential,
    )?)
}

pub fn attestation_payload<T: Serialize>(value: &T) -> anyhow::Result<AttestationPayload> {
    Ok(AttestationPayload(serde_json::to_vec(value)?))
}
