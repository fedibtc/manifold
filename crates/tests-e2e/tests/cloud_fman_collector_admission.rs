use std::time::Duration;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose};
use defe_api::{ResourceDescriptor, SharingMode};
use defe_client::AsyncDefeClient;
use fedi_credential_sdk_protocol::{
    HolderAuthorizationRequest, HolderContext, IssuerContext, IssuerSecretKeys, PendingIssuance,
    RevocationLocation, SubjectPubkey,
};
use fedi_decentralized_cloud_fman_telemetry::registration_router_for_test;
use fedi_decentralized_domain::{HolderAuthorizationEnvelope, ProtocolV1};
use fedi_decentralized_nostr::attester::{
    ISSUER_AUTHORITY_D_TAG, ISSUER_AUTHORITY_EVENT_KIND, ISSUER_AUTHORITY_HASHTAG,
};
use fedi_decentralized_nostr_clients::NostrRelayClient;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedi_decentralized_service_fleet_manager::{
    GuardianTelemetryRegistrationRequest, GuardianTelemetryRegistrationResponse,
    TelemetryCapability,
};
use fedi_iroh_rpc::iroh::SecretKey as IrohSecretKey;
use nostr_sdk::{EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tower::ServiceExt as _;

const ORIGIN: &str = "https://collector.test";
const REGISTRATION_PATH: &str = "/v1/telemetry/registrations";

#[tokio::test]
async fn valid_badge_crosses_real_registration_router_and_persists_target() {
    let mut client = AsyncDefeClient::connect_from_env()
        .await
        .expect("connect to defe");
    let lease = client
        .request_nostr_relay(SharingMode::Exclusive)
        .await
        .expect("allocate relay");
    let ResourceDescriptor::NostrRelay(relay) = &lease.descriptor else {
        panic!("expected relay, got {:?}", lease.descriptor);
    };

    let issuer =
        IssuerContext::import_secret_key(&test_issuer_secret_keys()).expect("import test issuer");
    let authority = issuer
        .issuer_authority(vec![RevocationLocation {
            protocol: "nostr".into(),
            location: relay.url.clone(),
        }])
        .expect("build authority");
    let issuer_metadata = authority.verify().expect("verify authority");
    let issuer_keys = Keys::parse(
        &issuer
            .export_secret_key()
            .expect("export issuer")
            .issuer_id_secret_key,
    )
    .expect("parse issuer keys");
    NostrRelayClient::connect(&relay.url, issuer_keys, Duration::from_secs(5))
        .await
        .expect("connect authority publisher")
        .publish_event(
            EventBuilder::new(
                Kind::Custom(ISSUER_AUTHORITY_EVENT_KIND),
                serde_json::to_string(&authority).expect("serialize authority"),
            )
            .tags([
                Tag::identifier(ISSUER_AUTHORITY_D_TAG),
                Tag::hashtag(ISSUER_AUTHORITY_HASHTAG),
            ]),
        )
        .await
        .expect("publish authority");

    let holder = HolderContext::generate();
    let badge_info = json!({
        "schema": "fedi-trust-score-v1.0",
        "trust_level": 9,
    });
    let (issuance_request, pending) = PendingIssuance::create_request(
        &issuer_metadata.issuance_key,
        issuer_metadata.issuer_id_pubkey.clone(),
        badge_info,
        json!(holder.public_key().to_string()),
    )
    .expect("create issuance");
    let issuance_response = issuer
        .issue_credential(pending.info.clone(), &issuance_request)
        .expect("issue badge");
    let badge = pending
        .finalize(&issuer_metadata.issuance_key, &issuance_response)
        .expect("finalize badge");
    let fman = Keys::generate();
    let holder_authorization = holder
        .authorize_credential_use(
            HolderAuthorizationRequest {
                subject_pubkey: fman
                    .public_key()
                    .to_string()
                    .parse::<SubjectPubkey>()
                    .expect("parse subject"),
            },
            &badge,
        )
        .expect("authorize FMan");
    let envelope = HolderAuthorizationEnvelope {
        holder_authorization,
        signed_credential: badge,
    };

    let verifier = PeerBadgeVerifier::new_for_test(
        [issuer_metadata.issuer_id_pubkey.0],
        [relay.url.parse().expect("parse relay URL")],
        9,
    )
    .expect("construct concrete verifier");
    let directory = tempfile::tempdir().expect("tempdir");
    let data_dir = directory.path().join("collector");
    let router = registration_router_for_test(&data_dir, ORIGIN, verifier)
        .await
        .expect("build real router");
    let body = serde_json::to_vec(&GuardianTelemetryRegistrationRequest {
        version: ProtocolV1,
        generation: 7,
        iroh_endpoint_id: IrohSecretKey::from_bytes(&[8; 32]).public().to_string(),
        capability: TelemetryCapability::from_bytes([9; 32]),
        holder_authorization: envelope,
    })
    .expect("serialize registration");
    let authorization = nip98(&fman, &body);
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(REGISTRATION_PATH)
                .header(header::AUTHORIZATION, authorization)
                .body(Body::from(body))
                .expect("build request"),
        )
        .await
        .expect("route request");
    assert_eq!(response.status(), StatusCode::OK);
    let response_body = to_bytes(response.into_body(), 1024)
        .await
        .expect("read response");
    let _: GuardianTelemetryRegistrationResponse =
        serde_json::from_slice(&response_body).expect("shared response DTO");

    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(data_dir.join("state.sqlite"));
    let pool = sqlx::SqlitePool::connect_with(options)
        .await
        .expect("open collector DB");
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM targets WHERE fman_pubkey = ? AND generation = 7")
            .bind(fman.public_key().to_string())
            .fetch_one(&pool)
            .await
            .expect("query persisted target");
    assert_eq!(count, 1);
}

fn nip98(keys: &Keys, body: &[u8]) -> String {
    let payload = hex::encode(Sha256::digest(body));
    let event = EventBuilder::new(Kind::HttpAuth, "")
        .custom_created_at(Timestamp::now())
        .tag(Tag::parse(["u", &format!("{ORIGIN}{REGISTRATION_PATH}")]).unwrap())
        .tag(Tag::parse(["method", "POST"]).unwrap())
        .tag(Tag::parse(["payload", &payload]).unwrap())
        .sign_with_keys(keys)
        .expect("sign NIP-98");
    format!(
        "Nostr {}",
        general_purpose::STANDARD.encode(serde_json::to_vec(&event).expect("serialize NIP-98"))
    )
}

fn test_issuer_secret_keys() -> IssuerSecretKeys {
    serde_json::from_str(include_str!(
        "../../domain/src/trust_score/issuer-secret-keys.json"
    ))
    .expect("parse test issuer fixture")
}
