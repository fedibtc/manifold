use fedi_credential_sdk_protocol::{
    Credential, CredentialDigest, CredentialProof, HolderAuthorization,
    HolderAuthorizationStatement, HolderId, IssuerId, ProtocolV1, SchnorrSignatureProof,
    SubjectPubkey, Timestamp,
};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use nostr_sdk::EventBuilder;

use fedi_decentralized_service_fleet_manager::Plan;
use fman_core::directory::AdvertisementSnapshot;

use super::*;

// The advertisement document's signing round-trip and exact wire-shape tests
// live with the shared document types in `fedi_decentralized_nostr::fman`.

fn authorization_event_at(holder: &Keys, subject: nostr_sdk::PublicKey, issued_at: u64) -> Event {
    let credential = Credential {
        issuer_id_pubkey: IssuerId(Keys::generate().public_key()),
        info: serde_json::json!({
            "schema": "fedi-trust-score-v1.0",
            "trust_level": 6,
        }),
        blind_msg: serde_json::json!(holder.public_key().to_string()),
    };
    let statement = HolderAuthorizationStatement {
        holder_id_pubkey: HolderId(holder.public_key()),
        subject_pubkey: SubjectPubkey(subject),
        credential_digest: CredentialDigest(credential.digest().unwrap()),
        issued_at: Timestamp(issued_at),
    };
    let signature = holder.sign_schnorr(&nostr_sdk::secp256k1::Message::from_digest(
        statement.digest().unwrap().into(),
    ));
    let envelope = serde_json::json!({
        "version": 1,
        "holder_id_pubkey": holder.public_key().to_string(),
        "holder_authorization": HolderAuthorization {
            version: ProtocolV1,
            authorization: statement,
            proof: SchnorrSignatureProof { signature },
        },
        "signed_credential": fedi_credential_sdk_protocol::SignedCredential {
            version: ProtocolV1,
            credential,
            proof: CredentialProof {
                signature: blind_rsa_signatures::Signature(vec![1, 2, 3, 4]),
            },
        },
    });
    EventBuilder::new(
        nostr_sdk::Kind::Custom(fedi_decentralized_nostr::fman::HOLDER_AUTHORIZATION_EVENT_KIND),
        envelope.to_string(),
    )
    .sign_with_keys(holder)
    .unwrap()
}

fn authorization_event(holder: &Keys, subject: nostr_sdk::PublicKey) -> Event {
    authorization_event_at(holder, subject, 1_730_000_000)
}

#[test]
fn candidate_verification_accepts_our_authorizations_and_rejects_others() {
    let holder = Keys::generate();
    let fman = Keys::generate();

    let event = authorization_event(&holder, fman.public_key());
    let embedded = verify_candidate(&event, &fman.public_key()).unwrap();
    assert_eq!(
        embedded.holder_authorization.authorization.subject_pubkey.0,
        fman.public_key()
    );
    assert!(matches!(
        observed_status(std::slice::from_ref(&embedded), Some(1_000)),
        OnboardingStatus::AuthorizationObserved {
            authorizations: 1,
            ..
        }
    ));

    let mut unsupported_content: serde_json::Value = serde_json::from_str(&event.content).unwrap();
    unsupported_content["version"] = serde_json::json!(2);
    let unsupported = EventBuilder::new(
        nostr_sdk::Kind::Custom(fedi_decentralized_nostr::fman::HOLDER_AUTHORIZATION_EVENT_KIND),
        unsupported_content.to_string(),
    )
    .sign_with_keys(&holder)
    .unwrap();
    assert!(
        verify_candidate(&unsupported, &fman.public_key()).is_err(),
        "unsupported event-content versions must be rejected"
    );

    // An authorization for a different subject is not ours to embed.
    let other = authorization_event(&holder, Keys::generate().public_key());
    let err = verify_candidate(&other, &fman.public_key()).unwrap_err();
    assert!(err.to_string().contains("subject"), "{err}");
}

#[test]
fn retained_authorizations_are_reverified_before_reuse() {
    let holder = Keys::generate();
    let fman = Keys::generate();
    let event = authorization_event(&holder, fman.public_key());

    assert_eq!(
        decode_retained_holder_authorizations(
            vec![event.as_json()],
            fman.public_key(),
            1_730_000_000,
        )
        .unwrap()
        .len(),
        1
    );
    assert!(
        decode_retained_holder_authorizations(
            vec![event.as_json()],
            Keys::generate().public_key(),
            1_730_000_000,
        )
        .is_err()
    );
}

#[test]
fn candidate_verification_enforces_the_exact_receiver_time_boundary() {
    let holder = Keys::generate();
    let fman = Keys::generate();
    let boundary = authorization_event_at(&holder, fman.public_key(), 10_000);
    verify_candidate_at(&boundary, &fman.public_key(), 10_000)
        .expect("the exact receiver boundary is admissible");

    let future = authorization_event_at(&holder, fman.public_key(), 10_001);
    let err = verify_candidate_at(&future, &fman.public_key(), 10_000).unwrap_err();
    assert_eq!(
        err.to_string(),
        "authorization issue time exceeds the receiver limit"
    );
}

#[test]
fn receiver_time_overflow_fails_closed() {
    assert_eq!(
        holder_authorization_max_issued_at(u64::MAX)
            .unwrap_err()
            .to_string(),
        "receiver time cannot represent authorization skew"
    );
}

const TEST_SERVICE_PUBKEY: &str =
    "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";

fn test_snapshot() -> AdvertisementSnapshot {
    AdvertisementSnapshot {
        iroh_endpoint_id: "endpoint".to_owned(),
        service_pubkey: TEST_SERVICE_PUBKEY
            .parse()
            .expect("test service pubkey parses"),
        plans: vec![Plan::InfiniteBestEffort {
            price_msats: 250000,
        }],
    }
}

#[tokio::test]
async fn built_payload_advertises_the_service_pubkey() {
    let keys = Keys::generate();
    let payload = build_payload(test_snapshot(), &keys);

    assert_eq!(payload.version, ProtocolV1);
    assert_eq!(
        payload.expires_at - payload.issued_at,
        60 * 60,
        "30-minute publications remain valid through one missed cycle",
    );
    assert_eq!(payload.fman_id_pubkey, keys.public_key().to_string());
    assert_eq!(
        payload.service_pubkey, TEST_SERVICE_PUBKEY,
        "the advertisement must carry the commitment-signing service pubkey \
         in canonical lowercase hex",
    );
    assert_eq!(
        payload.api_endpoints,
        vec![ApiEndpoint {
            transport: IROH_API_ENDPOINT_TRANSPORT.to_owned(),
            url: format!("{IROH_API_ENDPOINT_URL_SCHEME}endpoint"),
        }],
    );
    let document = sign_advertisement(payload, &keys).expect("built payload signs");
    verify_advertisement_self_signature(&document).expect("built payload verifies");
}

#[tokio::test]
async fn service_exposes_onboarding_info_and_status_watcher() {
    let keys = Keys::generate();
    let service = FleetManagerNostr::new(
        keys.clone(),
        Some(Keys::generate().public_key()),
        Vec::new(),
        None,
        ManifoldEnvironment::Development.profile().unwrap(),
    );

    assert_eq!(
        *service.presence().borrow(),
        DirectoryPresence {
            service_nostr_pubkey: keys.public_key(),
            // No read has completed, and nothing is retained.
            onboarding: OnboardingStatus::Checking,
            latest_fman_version: None,
        }
    );
    assert!(
        service
            .subscribe_setup_payment_federations()
            .borrow()
            .is_none()
    );
}

#[test]
fn setup_payment_admission_retains_only_the_highest_admitted_event() {
    let keys = Keys::generate();
    let older = EventBuilder::new(
        nostr_sdk::Kind::Custom(
            fedi_decentralized_nostr::setup_payment_federations::
                SETUP_PAYMENT_FEDERATIONS_EVENT_KIND,
        ),
        r#"{"version":1,"fman_version":"0.1.0","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations"}"#,
    )
    .tag(nostr_sdk::Tag::identifier(
        fedi_decentralized_nostr::setup_payment_federations::SETUP_PAYMENT_FEDERATIONS_D_TAG,
    ))
    .custom_created_at(nostr_sdk::Timestamp::from_secs(100))
    .sign_with_keys(&keys)
    .unwrap();
    let newer = EventBuilder::new(older.kind, older.content.clone())
        .tag(nostr_sdk::Tag::identifier(
            fedi_decentralized_nostr::setup_payment_federations::SETUP_PAYMENT_FEDERATIONS_D_TAG,
        ))
        .custom_created_at(nostr_sdk::Timestamp::from_secs(101))
        .sign_with_keys(&keys)
        .unwrap();
    let first = admit(
        None,
        keys.public_key(),
        vec![newer.clone(), older],
        nostr_sdk::Timestamp::from_secs(101),
    )
    .unwrap();

    let (stored, _) = first.retain.expect("the winner must be retained");
    assert_eq!(serde_json::from_str::<Event>(&stored).unwrap().id, newer.id);

    // Re-admitting the retained event with nothing new restores it and asks
    // for no write: retention is only paid when the winner changes.
    let restored = admit(
        Some(stored.clone()),
        keys.public_key(),
        Vec::new(),
        nostr_sdk::Timestamp::from_secs(102),
    )
    .unwrap();
    assert_eq!(restored.admitted.unwrap().event().id, newer.id);
    assert!(restored.retain.is_none());

    // A rotated publisher invalidates the retained event: revalidation fails
    // loudly (daemon startup refuses) rather than silently trusting a stored
    // event the current profile no longer authenticates
    // (SPEC-setup-payment-federations *replacement and retention*).
    admit(
        Some(stored),
        Keys::generate().public_key(),
        Vec::new(),
        nostr_sdk::Timestamp::from_secs(103),
    )
    .expect_err("stored event from a previous publisher must not restore");
}

/// A completed read that finds nothing is a different fact from not having
/// looked. The dashboard writes different sentences for the two, so the
/// projection has to tell them apart.
#[test]
fn a_completed_empty_read_is_not_the_same_as_no_read() {
    assert_eq!(observed_status(&[], None), OnboardingStatus::Checking);
    assert_eq!(
        observed_status(&[], Some(1_760_000_000)),
        OnboardingStatus::NotObserved {
            checked_at: 1_760_000_000
        }
    );
}

/// A retained authorization reports itself before any read, and says so by
/// carrying no check time rather than borrowing one.
#[test]
fn a_retained_authorization_reports_itself_with_no_check_time() {
    let holder = Keys::generate();
    let fman = Keys::generate();
    let event = authorization_event(&holder, fman.public_key());
    let embedded = verify_candidate(&event, &fman.public_key()).unwrap();

    assert_eq!(
        observed_status(std::slice::from_ref(&embedded), None),
        OnboardingStatus::AuthorizationObserved {
            authorizations: 1,
            holders: vec![holder.public_key()],
            checked_at: None,
        }
    );
}
