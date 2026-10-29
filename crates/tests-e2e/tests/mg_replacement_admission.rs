//! The source's concrete relay-backed replacement admission, without FA input.

use std::time::Duration;

use defe_api::{ResourceDescriptor, SharingMode};
use defe_client::AsyncDefeClient;
use fedi_decentralized_domain::{HolderAuthorizationEnvelope, ProtocolV1};
use fedi_decentralized_manifold_environment::{DEV_NOSTR_RELAYS_ENV, ManifoldEnvironment};
use fedi_decentralized_nostr::attester::{
    CREDENTIAL_REVOCATION_EVENT_KIND, CREDENTIAL_REVOCATION_HASHTAG, credential_revocation_d_tag,
};
use fedi_decentralized_nostr::mg::{
    AdvertisementDocument, AdvertisementPayload, Availability, MG_ADVERTISEMENT_D_TAG,
    MG_ADVERTISEMENT_EVENT_KIND, MG_ADVERTISEMENT_HASHTAG, sign_advertisement,
};
use fedi_decentralized_nostr_clients::NostrRelayClient;
use mg_core::replacement_guardian::ReplacementGuardianVerifier;
use mg_nostr::replacement_guardian::NostrReplacementGuardianVerifier;
use nostr_sdk::{EventBuilder, Keys, Kind, PublicKey, Tag};
use peerbadge_protocol::{
    HolderAuthorizationRequest, HolderContext, IssuerContext, PendingIssuance, SubjectPubkey,
};
use serde_json::json;

#[path = "mg_replacement_admission/relay_probe.rs"]
mod relay_probe;

const CHILD_RELAY: &str = "MG_REPLACEMENT_ADMISSION_CHILD_RELAY";

#[tokio::test]
async fn replacement_requires_current_peerbadge_for_exact_destination() {
    if let Ok(relay) = std::env::var(CHILD_RELAY) {
        check_admission(&relay).await;
        return;
    }
    let mut client = AsyncDefeClient::connect_from_env().await.unwrap();
    let lease = client
        .request_nostr_relay(SharingMode::Exclusive)
        .await
        .unwrap();
    let ResourceDescriptor::NostrRelay(relay) = &lease.descriptor else {
        panic!("expected Nostr relay");
    };
    let probe = relay_probe::RelayProbe::start(relay.url.clone()).await;
    // Resolve the canonical development profile in an isolated process, never
    // mutate process-global environment alongside other test runtimes.
    let status = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "replacement_requires_current_peerbadge_for_exact_destination",
                "--nocapture",
            ])
            .env(CHILD_RELAY, &relay.url)
            .env(DEV_NOSTR_RELAYS_ENV, &probe.url)
            .kill_on_drop(true)
            .status(),
    )
    .await
    .expect("replacement admission child completes")
    .unwrap();
    assert!(status.success());
    assert_eq!(
        probe
            .revocation_reads
            .load(std::sync::atomic::Ordering::SeqCst),
        4,
        "only the four authentic, subject-bound candidates may fetch revocation"
    );
}

fn service_key(key: PublicKey) -> secp256k1::XOnlyPublicKey {
    secp256k1::XOnlyPublicKey::from_slice(key.as_bytes()).unwrap()
}

fn issue(
    issuer: &IssuerContext,
    holder: &HolderContext,
    destination: PublicKey,
    trust_level: u64,
) -> HolderAuthorizationEnvelope {
    let authority = issuer.issuer_authority(Vec::new()).unwrap();
    let info = json!({"schema": "fedi-trust-score-v1.0", "trust_level": trust_level});
    let (request, pending) = PendingIssuance::create_request(
        &authority.issuer.issuance_key,
        authority.issuer.issuer_id_pubkey.clone(),
        info.clone(),
        json!(holder.public_key().to_string()),
    )
    .unwrap();
    let response = issuer.issue_credential(info, &request).unwrap();
    let signed_credential = pending
        .finalize(&authority.issuer.issuance_key, &response)
        .unwrap();
    let holder_authorization = holder
        .authorize_credential_use(
            HolderAuthorizationRequest {
                subject_pubkey: SubjectPubkey(destination),
            },
            &signed_credential,
        )
        .unwrap();
    HolderAuthorizationEnvelope {
        holder_authorization,
        signed_credential,
    }
}

fn advertisement(
    mg: &Keys,
    service: PublicKey,
    envelope: &HolderAuthorizationEnvelope,
) -> AdvertisementDocument {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    sign_advertisement(
        AdvertisementPayload {
            version: ProtocolV1,
            mg_id_pubkey: mg.public_key().to_string(),
            service_pubkey: service.to_string(),
            issued_at: now,
            expires_at: now + 3600,
            api_endpoints: Vec::new(),
            availability: Availability {
                fedimintd_version: "0.1.0".parse().unwrap(),
                federation_sizes: Vec::new(),
            },
            plans: Vec::new(),
            holder_authorizations: vec![envelope.clone()],
        },
        mg,
    )
    .unwrap()
}

async fn publish(relay: &str, mg: &Keys, document: &AdvertisementDocument) {
    NostrRelayClient::connect(relay, mg.clone(), Duration::from_secs(5))
        .await
        .unwrap()
        .publish_event(
            EventBuilder::new(
                Kind::Custom(MG_ADVERTISEMENT_EVENT_KIND),
                serde_json::to_string(document).unwrap(),
            )
            .tags([
                Tag::identifier(MG_ADVERTISEMENT_D_TAG),
                Tag::hashtag(MG_ADVERTISEMENT_HASHTAG),
            ]),
        )
        .await
        .unwrap();
}

async fn check_admission(relay: &str) {
    let profile = ManifoldEnvironment::Development.profile().unwrap();
    let issuer = IssuerContext::import_secret_key(
        &serde_json::from_str(profile.test_issuer_secret_keys().unwrap()).unwrap(),
    )
    .unwrap();
    let verifier = NostrReplacementGuardianVerifier::new(profile).unwrap();
    let holder = HolderContext::generate();
    // Badge/Nostr identity and RPC commitment key are distinct in production.
    let mg = Keys::generate();
    let destination = Keys::generate().public_key();
    assert!(
        verifier.verify(service_key(destination)).await.is_err(),
        "an FA-nominated key alone is insufficient"
    );

    let another_destination = Keys::generate().public_key();
    let other_mg = Keys::generate();
    let wrong_service = issue(&issuer, &holder, other_mg.public_key(), 9);
    publish(
        relay,
        &other_mg,
        &advertisement(&other_mg, another_destination, &wrong_service),
    )
    .await;
    verifier
        .verify(service_key(another_destination))
        .await
        .unwrap();
    assert!(
        verifier.verify(service_key(destination)).await.is_err(),
        "a verified advertisement for a different commitment key is insufficient"
    );

    let attacker = Keys::generate();
    publish(
        relay,
        &attacker,
        &advertisement(&attacker, destination, &wrong_service),
    )
    .await;
    assert!(
        verifier.verify(service_key(destination)).await.is_err(),
        "a malicious advertisement cannot borrow another MG's badge"
    );

    let rebound_mg = Keys::generate();
    let rebound_badge = issue(&issuer, &holder, rebound_mg.public_key(), 9);
    let mut rebound = advertisement(&rebound_mg, another_destination, &rebound_badge);
    rebound.payload.service_pubkey = destination.to_string();
    publish(relay, &rebound_mg, &rebound).await;
    assert!(
        verifier.verify(service_key(destination)).await.is_err(),
        "rewriting the commitment key invalidates the portable advertisement proof"
    );

    let low_destination = Keys::generate().public_key();
    let low_mg = Keys::generate();
    let low = issue(&issuer, &holder, low_mg.public_key(), 8);
    publish(
        relay,
        &low_mg,
        &advertisement(&low_mg, low_destination, &low),
    )
    .await;
    assert!(
        verifier.verify(service_key(low_destination)).await.is_err(),
        "trusted issuance below the profile minimum is insufficient"
    );

    let forged_destination = Keys::generate().public_key();
    let forged_mg = Keys::generate();
    let mut forged = issue(&issuer, &holder, forged_mg.public_key(), 9);
    forged.signed_credential.proof.signature.0[0] ^= 1;
    publish(
        relay,
        &forged_mg,
        &advertisement(&forged_mg, forged_destination, &forged),
    )
    .await;
    assert!(
        verifier
            .verify(service_key(forged_destination))
            .await
            .is_err(),
        "a holder signature cannot bless a forged issuer proof"
    );

    let accepted = issue(&issuer, &holder, mg.public_key(), 9);
    publish(relay, &mg, &advertisement(&mg, destination, &accepted)).await;
    // Every attacker advert precedes the honest one in discovery order. Half
    // borrow its public envelope; half rewrite its subject without a holder
    // signature. Neither may consume any revocation reads.
    let publisher = NostrRelayClient::connect(relay, Keys::generate(), Duration::from_secs(5))
        .await
        .unwrap();
    for index in 0..32 {
        let attacker = Keys::generate();
        let mut envelope = accepted.clone();
        if index % 2 == 0 {
            envelope.holder_authorization.authorization.subject_pubkey =
                SubjectPubkey(attacker.public_key());
        }
        let document = advertisement(&attacker, destination, &envelope);
        let event = EventBuilder::new(
            Kind::Custom(MG_ADVERTISEMENT_EVENT_KIND),
            serde_json::to_string(&document).unwrap(),
        )
        .tags([
            Tag::identifier(MG_ADVERTISEMENT_D_TAG),
            Tag::hashtag(MG_ADVERTISEMENT_HASHTAG),
        ])
        .custom_created_at(nostr_sdk::Timestamp::from(document.payload.issued_at + 1))
        .sign_with_keys(&attacker)
        .unwrap();
        publisher.publish_signed_event(&event).await.unwrap();
    }
    verifier.verify(service_key(destination)).await.unwrap();

    let revocation = issuer
        .revoke_credential(&accepted.signed_credential)
        .unwrap();
    let issuer_publisher = NostrRelayClient::connect(
        relay,
        Keys::parse(&issuer.export_secret_key().unwrap().issuer_id_secret_key).unwrap(),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    issuer_publisher
        .publish_event(
            EventBuilder::new(
                Kind::Custom(CREDENTIAL_REVOCATION_EVENT_KIND),
                serde_json::to_string(&revocation).unwrap(),
            )
            .tags([
                Tag::identifier(credential_revocation_d_tag(
                    serde_json::to_value(&revocation.revocation.credential_digest)
                        .unwrap()
                        .as_str()
                        .unwrap(),
                )),
                Tag::hashtag(CREDENTIAL_REVOCATION_HASHTAG),
            ]),
        )
        .await
        .unwrap();
    assert!(
        verifier.verify(service_key(destination)).await.is_err(),
        "prior success cannot authorize a later release after revocation"
    );
}
