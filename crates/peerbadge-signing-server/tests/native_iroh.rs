use std::time::Duration;

use fedi_decentralized_domain::HolderAuthorizationEnvelope;
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedi_decentralized_peerbadge_signing_server::{
    Signer, SigningServer, issuer::IssuerMaterial, spawn_router,
};
use fedi_decentralized_service_peerbadge_signing::*;
use iroh::{Endpoint, RelayMode, endpoint::presets};
use nostr_sdk::{Keys, secp256k1::Message};
use peerbadge_protocol::{
    HolderAuthorizationRequest, HolderContext, IssuanceResponse, IssuerAuthority, PendingIssuance,
    SubjectPubkey,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_holder_redeems_once_and_development_verifier_accepts() -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(60), issuance_loop()).await?
}

async fn issuance_loop() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let profile = ManifoldEnvironment::Development.profile()?;
    let signer = Keys::generate();
    let issuer = IssuerMaterial::load(&profile, None)?;
    let server = SigningServer::new(
        issuer,
        vec![Signer {
            pubkey: signer.public_key().to_hex(),
            max_level: 9,
            max_sessions_per_hour: 10,
        }],
        &directory.path().join("audit.jsonl"),
    )?;
    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    let router = spawn_router(endpoint, server);
    let client_endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    let signer_client = PeerBadgeSigningServiceClient::new(
        client_endpoint
            .connect(router.endpoint().addr(), PEERBADGE_SIGNING_ALPN)
            .await?,
    );
    let challenge = signer_client.challenge(ChallengeRequest {}).await?;
    let issuer_id: [u8; 32] = hex::decode(&challenge.issuer_id_pubkey)?
        .try_into()
        .unwrap();
    let nonce: [u8; 32] = challenge.nonce.as_slice().try_into()?;
    let message = Message::from_digest(challenge_digest(&nonce, 9, &issuer_id));
    let opened = signer_client
        .open_session(OpenSessionRequest {
            signer_pubkey: signer.public_key().to_hex(),
            level: 9,
            nonce: challenge.nonce,
            signature: signer.sign_schnorr(&message).as_ref().to_vec(),
        })
        .await?;
    let status_request = SessionStatusRequest {
        session_id: opened.session_id.clone(),
    };
    assert_eq!(
        signer_client
            .session_status(status_request.clone())
            .await?
            .state,
        SessionState::Open
    );
    let authority: IssuerAuthority = serde_json::from_str(&opened.issuer_authority)?;
    let metadata = authority.verify()?;
    let holder = HolderContext::generate();
    let (request, pending) = PendingIssuance::create_request(
        &metadata.issuance_key,
        metadata.issuer_id_pubkey.clone(),
        opened.info.clone(),
        serde_json::json!(holder.public_key().to_hex()),
    )?;
    let request_json = serde_json::to_string(&request)?;
    // A distinct transport identity redeems only with the unguessable session
    // capability. No signer or holder identity is transmitted in the request.
    let holder_endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    let holder_client = PeerBadgeSigningServiceClient::new(
        holder_endpoint
            .connect(router.endpoint().addr(), PEERBADGE_SIGNING_ALPN)
            .await?,
    );
    let redeem = RedeemSessionRequest {
        session_id: opened.session_id.clone(),
        request: request_json.clone(),
    };
    let (first, second) = tokio::join!(
        holder_client.redeem_session(redeem.clone()),
        holder_client.redeem_session(redeem)
    );
    let issued = first?;
    assert_eq!(issued.response, second?.response);
    assert_eq!(
        holder_client
            .redeem_session(RedeemSessionRequest {
                session_id: opened.session_id.clone(),
                request: format!("{request_json} "),
            })
            .await
            .unwrap_err(),
        SigningError::SessionAlreadyRedeemed
    );
    let response: IssuanceResponse = serde_json::from_str(&issued.response)?;
    let credential = pending.finalize(&metadata.issuance_key, &response)?;
    let subject = Keys::generate().public_key();
    let authorization = holder.authorize_credential_use_at_time(
        HolderAuthorizationRequest {
            subject_pubkey: SubjectPubkey(subject),
        },
        &credential,
        1000,
    )?;
    let envelope = HolderAuthorizationEnvelope {
        holder_authorization: authorization,
        signed_credential: credential,
    };
    PeerBadgeVerifier::try_from_profile(&profile)?.verify_issuance_at(&envelope, 1001)?;
    assert_eq!(
        signer_client.session_status(status_request).await?.state,
        SessionState::Redeemed
    );
    let audit = std::fs::read_to_string(directory.path().join("audit.jsonl"))?;
    assert!(!audit.contains(&request_json));
    assert!(!audit.contains(&holder.public_key().to_hex()));
    assert_eq!(audit.matches("session_redeemed").count(), 1);
    assert_eq!(audit.matches("session_redeem_replayed").count(), 1);
    router.shutdown().await?;
    client_endpoint.close().await;
    holder_endpoint.close().await;
    Ok(())
}
