use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use nostr_sdk::EventBuilder;
use peerbadge_protocol::{
    Credential, CredentialDigest, CredentialProof, HolderAuthorization,
    HolderAuthorizationStatement, HolderId, IssuerId, ProtocolV1, SchnorrSignatureProof,
    SubjectPubkey, Timestamp,
};

use fedi_decentralized_service_manifold_guardian::Plan;
use mg_core::directory::AdvertisementSnapshot;

use super::*;

// The advertisement document's signing round-trip and exact wire-shape tests
// live with the shared document types in `fedi_decentralized_nostr::mg`.

fn authorization_event_at(holder: &Keys, subject: nostr_sdk::PublicKey, issued_at: u64) -> Event {
    let credential = Credential {
        issuer_id_pubkey: IssuerId(Keys::generate().public_key()),
        info: serde_json::json!({
            "schema": "fedi-trust-score-v1.0",
            "trust_level": 6,
        }),
        blind_msg: serde_json::json!(holder.public_key().to_string()),
    };
    let signed_credential = peerbadge_protocol::SignedCredential {
        version: ProtocolV1,
        credential,
        proof: CredentialProof {
            signature: blind_rsa_signatures::Signature(vec![1, 2, 3, 4]),
        },
    };
    authorization_event_for(holder, subject, issued_at, signed_credential)
}

/// An authorization whose badge the development environment's trusted issuer
/// really issued to `holder`.
fn issued_authorization_event_at(
    holder: &Keys,
    subject: nostr_sdk::PublicKey,
    issued_at: u64,
) -> Event {
    let profile = ManifoldEnvironment::Development.profile().unwrap();
    let issuer = peerbadge_protocol::IssuerContext::import_secret_key(
        &serde_json::from_str(profile.test_issuer_secret_keys().unwrap()).unwrap(),
    )
    .unwrap();
    let authority = issuer.issuer_authority(Vec::new()).unwrap();
    let info = serde_json::json!({
        "schema": "fedi-trust-score-v1.0",
        "trust_level": 6,
    });
    let (request, pending) = peerbadge_protocol::PendingIssuance::create_request(
        &authority.issuer.issuance_key,
        authority.issuer.issuer_id_pubkey.clone(),
        info.clone(),
        serde_json::json!(holder.public_key().to_string()),
    )
    .unwrap();
    let response = issuer.issue_credential(info, &request).unwrap();
    let signed_credential = pending
        .finalize(&authority.issuer.issuance_key, &response)
        .unwrap();
    authorization_event_for(holder, subject, issued_at, signed_credential)
}

fn authorization_event_for(
    holder: &Keys,
    subject: nostr_sdk::PublicKey,
    issued_at: u64,
    signed_credential: peerbadge_protocol::SignedCredential,
) -> Event {
    let credential = &signed_credential.credential;
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
        "signed_credential": signed_credential,
    });
    EventBuilder::new(
        nostr_sdk::Kind::Custom(fedi_decentralized_nostr::mg::HOLDER_AUTHORIZATION_EVENT_KIND),
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
    let mg = Keys::generate();

    let event = authorization_event(&holder, mg.public_key());
    let embedded = verify_candidate(&event, &mg.public_key()).unwrap();
    assert_eq!(
        embedded.holder_authorization.authorization.subject_pubkey.0,
        mg.public_key()
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
        nostr_sdk::Kind::Custom(fedi_decentralized_nostr::mg::HOLDER_AUTHORIZATION_EVENT_KIND),
        unsupported_content.to_string(),
    )
    .sign_with_keys(&holder)
    .unwrap();
    assert!(
        verify_candidate(&unsupported, &mg.public_key()).is_err(),
        "unsupported event-content versions must be rejected"
    );

    // An authorization for a different subject is not ours to embed.
    let other = authorization_event(&holder, Keys::generate().public_key());
    let err = verify_candidate(&other, &mg.public_key()).unwrap_err();
    assert!(err.to_string().contains("subject"), "{err}");
}

#[test]
fn newest_candidate_is_chosen_among_trusted_issuances_only() {
    let mg = Keys::generate();
    let trusted =
        PeerBadgeVerifier::try_from_profile(&ManifoldEnvironment::Development.profile().unwrap())
            .unwrap();
    // The newest candidate is well formed but its badge names no trusted
    // issuer, so the older trusted issuance wins over it.
    let chosen = newest_issued_candidate(
        [
            issued_authorization_event_at(&Keys::generate(), mg.public_key(), 100),
            authorization_event_at(&Keys::generate(), mg.public_key(), 300),
            issued_authorization_event_at(&Keys::generate(), mg.public_key(), 200),
        ],
        &mg.public_key(),
        &trusted,
        1_000,
    )
    .expect("a trusted issuance is admitted");
    assert_eq!(chosen.authorization_issued_at, 200);

    let staging =
        PeerBadgeVerifier::try_from_profile(&ManifoldEnvironment::Staging.profile().unwrap())
            .unwrap();
    assert!(
        newest_issued_candidate(
            [issued_authorization_event_at(
                &Keys::generate(),
                mg.public_key(),
                100
            )],
            &mg.public_key(),
            &staging,
            1_000,
        )
        .is_none(),
        "another environment's issuer is not trusted here"
    );
}

#[test]
fn retained_authorizations_are_reverified_before_reuse() {
    let holder = Keys::generate();
    let mg = Keys::generate();
    let event = authorization_event(&holder, mg.public_key());

    assert_eq!(
        decode_retained_holder_authorizations(
            Some(event.as_json()),
            mg.public_key(),
            1_730_000_000,
        )
        .unwrap()
        .len(),
        1
    );
    assert!(
        decode_retained_holder_authorizations(
            Some(event.as_json()),
            Keys::generate().public_key(),
            1_730_000_000,
        )
        .is_err()
    );
}

#[test]
fn candidate_verification_enforces_the_exact_receiver_time_boundary() {
    let holder = Keys::generate();
    let mg = Keys::generate();
    let boundary = authorization_event_at(&holder, mg.public_key(), 10_000);
    verify_candidate_at(&boundary, &mg.public_key(), 10_000)
        .expect("the exact receiver boundary is admissible");

    let future = authorization_event_at(&holder, mg.public_key(), 10_001);
    let err = verify_candidate_at(&future, &mg.public_key(), 10_000).unwrap_err();
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
        uptime: Some(mg_core::seat_readiness::UptimePermille {
            day: 998,
            week: 991,
            month: 964,
        }),
    }
}

#[tokio::test]
async fn built_payload_advertises_the_service_pubkey() {
    let keys = Keys::generate();
    let payload = build_payload(test_snapshot(), Vec::new());

    assert_eq!(
        payload.expires_at - payload.issued_at,
        60 * 60,
        "30-minute publications remain valid through one missed cycle",
    );
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
    let document = sign_advertisement(payload.to_legacy(keys.public_key()), &keys)
        .expect("built payload signs");
    verify_advertisement_self_signature(&document).expect("built payload verifies");
}

/// One snapshot yields both formats: the same offer and validity window, the
/// uptime only in v2, and the legacy content in the exact v1 shape.
#[tokio::test]
async fn both_advertisement_formats_publish_one_snapshot() {
    let keys = Keys::generate();
    let payload = build_payload(test_snapshot(), Vec::new());
    let event = |format| {
        advertisement_event(format, &payload, &keys)
            .unwrap()
            .sign_with_keys(&keys)
            .unwrap()
    };

    let legacy = event(MgAdvertisementFormat::Legacy);
    assert_eq!(legacy.kind, Kind::Custom(37701));
    assert!(fedi_decentralized_nostr::has_exact_d_tag(
        &legacy, "fman-ad"
    ));
    let content = serde_json::from_str::<Value>(&legacy.content).unwrap();
    let mut members = content["payload"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    members.sort();
    assert_eq!(
        members,
        [
            "api_endpoints",
            "availability",
            "expires_at",
            "fman_id_pubkey",
            "holder_authorizations",
            "issued_at",
            "plans",
            "service_pubkey",
            "version",
        ],
        "kind 37701 keeps the v1 payload, with no uptime",
    );
    assert_eq!(
        content.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["payload", "proof"],
    );
    let document = serde_json::from_str::<AdvertisementDocument>(&legacy.content).unwrap();
    verify_advertisement_self_signature(&document).unwrap();

    let v2 = event(MgAdvertisementFormat::V2);
    assert_eq!(v2.kind, Kind::Custom(37709));
    assert!(fedi_decentralized_nostr::has_exact_d_tag(&v2, "mg-ad"));
    assert!(
        v2.tags.iter().any(|tag| tag.as_slice() == ["t", "fedi-mg"]),
        "v2 carries its own index hashtag",
    );
    let verified =
        fedi_decentralized_nostr::mg::verify_advertisement_v2(&v2.content, keys.public_key())
            .unwrap();
    assert_eq!(verified, payload);
    assert_eq!(
        serde_json::to_value(verified.uptime_permille).unwrap(),
        serde_json::json!({"1d": 998, "7d": 991, "30d": 964}),
    );
    assert_eq!(
        (document.payload.issued_at, document.payload.expires_at),
        (verified.issued_at, verified.expires_at),
    );
    assert_eq!(AdvertisementV2Payload::from(document.payload), {
        let mut offer = payload.clone();
        offer.uptime_permille = None;
        offer
    });
}

#[tokio::test]
async fn newer_authorization_replaces_durable_and_live_state_without_rollback() {
    let temp = tempfile::TempDir::new().unwrap();
    let db = mg_core::db::Db::open(temp.path()).await.unwrap();
    db.install_identity(&RootMnemonic::generate().unwrap())
        .await
        .unwrap();
    let keys = Keys::generate();
    let holder = Keys::generate();
    let store = Arc::new(FleetHolderAuthorizationStore::new(db.clone()));
    let service = ManifoldGuardianNostr::new(
        keys.clone(),
        None,
        Vec::new(),
        None,
        ManifoldEnvironment::Development.profile().unwrap(),
        store.clone(),
        db,
    );
    // The replacement comes from another holder with another credential: the
    // MG keeps exactly one authorization, not one per credential.
    let original = authorization_event_at(&holder, keys.public_key(), 100);
    let replacement = authorization_event_at(&Keys::generate(), keys.public_key(), 200);
    let mut changes = service.inner.holder_authorizations.subscribe();
    for (event, expected_time) in [(original.clone(), 100), (replacement, 200), (original, 200)] {
        service
            .inner
            .retain_authorization(Some(
                verified_holder_authorization_event(event, &keys.public_key(), now_secs()).unwrap(),
            ))
            .await
            .unwrap();
        assert!(changes.has_changed().unwrap());
        changes.borrow_and_update();
        let live = service.holder_authorizations();
        assert_eq!(live.len(), 1);
        assert_eq!(
            live[0].holder_authorization.authorization.issued_at.0,
            expected_time
        );
        let retained = load_retained_holder_authorizations(&store, keys.public_key())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(live).unwrap(),
            serde_json::to_value(retained).unwrap()
        );
    }
    let _in_progress = service.inner.authorization_refresh.lock().await;
    let error = mg_core::directory::HolderAuthorizationRefresher::refresh(&service)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("already in progress"));
    service.inner.retain_authorization(None).await.unwrap();
    assert_eq!(
        service.holder_authorizations()[0]
            .holder_authorization
            .authorization
            .issued_at
            .0,
        200
    );
    assert!(matches!(
        service.presence().borrow().onboarding,
        OnboardingStatus::AuthorizationObserved {
            authorizations: 1,
            ..
        }
    ));
}

#[tokio::test]
async fn service_exposes_onboarding_info_and_status_watcher() {
    let temp = tempfile::TempDir::new().unwrap();
    let db = mg_core::db::Db::open(temp.path()).await.unwrap();
    let keys = Keys::generate();
    let service = ManifoldGuardianNostr::new(
        keys.clone(),
        Some(Keys::generate().public_key()),
        Vec::new(),
        None,
        ManifoldEnvironment::Development.profile().unwrap(),
        Arc::new(FleetHolderAuthorizationStore::new(db.clone())),
        db,
    );

    assert_eq!(
        *service.presence().borrow(),
        DirectoryPresence {
            service_nostr_pubkey: keys.public_key(),
            // No read has completed, and nothing is retained.
            onboarding: OnboardingStatus::Checking,
            latest_mg_version: None,
        }
    );
    assert!(
        service
            .subscribe_setup_payment_federations()
            .borrow()
            .is_none()
    );
}

#[tokio::test]
async fn transfer_proofs_follow_live_enrollment_and_reloaded_material() {
    use fedi_decentralized_service_manifold_guardian::{
        FaId, FederationId, Locator, MgSignedRequest, QuoteId, SeatId, SeatTransferGrant,
        SignedRequest, Timestamp as RequestTimestamp, TransferSeatCall, TransferSeatRequest,
    };
    let temp = tempfile::TempDir::new().unwrap();
    let db = mg_core::db::Db::open(temp.path()).await.unwrap();
    let keys = Keys::generate();
    let store = Arc::new(FleetHolderAuthorizationStore::new(db.clone()));
    let service = ManifoldGuardianNostr::new(
        keys.clone(),
        None,
        Vec::new(),
        None,
        ManifoldEnvironment::Development.profile().unwrap(),
        store.clone(),
        db.clone(),
    );
    let signing_key =
        secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[31; 32]).unwrap();
    let grant = SeatTransferGrant {
        ts: RequestTimestamp(now_secs()),
        fa_id: FaId(signing_key.x_only_public_key().0),
        federation_id: FederationId("proof-generation-test".into()),
        source: Locator::new(
            RootMnemonic::generate()
                .unwrap()
                .derive_iroh_secret_key()
                .public()
                .into(),
            signing_key.x_only_public_key().0,
        ),
        source_seat_id: SeatId::from(QuoteId([32; 32])),
        destination_service_pubkey: signing_key.x_only_public_key().0,
        destination_seat_id: SeatId::from(QuoteId([33; 32])),
    };
    let request = MgSignedRequest::create(
        &TransferSeatRequest {
            ts: RequestTimestamp(now_secs()),
            destination_service_pubkey: signing_key.x_only_public_key().0,
            grant: SignedRequest::create(&grant, &signing_key).unwrap(),
        },
        &signing_key,
    )
    .unwrap();
    assert!(service.transfer_admission_proof(&request).is_err());
    let mut proofs = Vec::new();
    for issued_at in [now_secs() - 1, now_secs()] {
        let event = issued_authorization_event_at(&Keys::generate(), keys.public_key(), issued_at);
        service
            .inner
            .retain_authorization(Some(
                verified_holder_authorization_event(event, &keys.public_key(), now_secs()).unwrap(),
            ))
            .await
            .unwrap();
        let proof = service.transfer_admission_proof(&request).unwrap();
        assert_eq!(
            proof.holder_authorization,
            service.holder_authorizations()[0]
        );
        TransferSeatCall::new(request.clone(), proof.clone())
            .verify(RequestTimestamp(now_secs()))
            .unwrap()
            .1
            .verify_subject()
            .unwrap();
        proofs.push(proof);
    }
    assert_ne!(
        proofs[0], proofs[1],
        "no enrollment snapshot is cached in the adapter"
    );
    let retained = load_retained_holder_authorizations(&store, keys.public_key())
        .await
        .unwrap();
    drop(service);
    let reloaded = ManifoldGuardianNostr::new(
        keys,
        None,
        retained,
        None,
        ManifoldEnvironment::Development.profile().unwrap(),
        store,
        db,
    );
    assert_eq!(
        reloaded
            .transfer_admission_proof(&request)
            .unwrap()
            .holder_authorization,
        proofs[1].holder_authorization,
    );
    reloaded
        .inner
        .holder_authorizations
        .send_replace(Vec::new());
    assert!(reloaded.transfer_admission_proof(&request).is_err());
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

/// An older release retains content it does not interpret, such as a
/// `min_mg_version` that is not SemVer. After an upgrade that event no longer
/// admits; the MG must still start, with no policy, keep the event as its
/// high-water mark, and take the next higher publication that passes.
#[test]
fn a_retained_policy_this_release_rejects_loads_as_none_and_still_bounds_replacement() {
    let keys = Keys::generate();
    let signed = |created_at: u64, min_mg_version: &str| {
        EventBuilder::new(
            nostr_sdk::Kind::Custom(
                fedi_decentralized_nostr::setup_payment_federations::
                    SETUP_PAYMENT_FEDERATIONS_EVENT_KIND,
            ),
            format!(
                r#"{{"version":1,"fman_version":"0.1.9","federations":[],"telemetry_registration_url":"https://push.fedi.example/v1/telemetry/registrations","min_mg_version":{min_mg_version}}}"#
            ),
        )
        .tag(nostr_sdk::Tag::identifier(
            fedi_decentralized_nostr::setup_payment_federations::SETUP_PAYMENT_FEDERATIONS_D_TAG,
        ))
        .custom_created_at(nostr_sdk::Timestamp::from_secs(created_at))
        .sign_with_keys(&keys)
        .unwrap()
    };
    let stored = signed(200, r#""0.1""#).as_json();
    let now = nostr_sdk::Timestamp::from_secs(1_000);

    let startup = admit(Some(stored.clone()), keys.public_key(), Vec::new(), now)
        .expect("an authentic stored event this release rejects does not stop startup");
    assert!(startup.admitted.is_none());
    assert!(
        startup.retain.is_none(),
        "the stored event stays the high-water mark"
    );

    let older = signed(199, r#""0.1.8""#);
    let rolled_back = admit(Some(stored.clone()), keys.public_key(), vec![older], now).unwrap();
    assert!(
        rolled_back.admitted.is_none(),
        "a publication below the stored one must not replace it"
    );

    let newer = signed(201, r#""0.1.9""#);
    let replaced = admit(
        Some(stored.clone()),
        keys.public_key(),
        vec![newer.clone()],
        now,
    )
    .unwrap();
    assert_eq!(replaced.admitted.unwrap().event().id, newer.id);
    let (retained, _) = replaced.retain.expect("the newer publication is retained");
    assert_eq!(
        serde_json::from_str::<Event>(&retained).unwrap().id,
        newer.id
    );

    // Authentication still fails closed.
    admit(Some(stored), Keys::generate().public_key(), Vec::new(), now)
        .expect_err("a stored event from another publisher must not load");
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
    let mg = Keys::generate();
    let event = authorization_event(&holder, mg.public_key());
    let embedded = verify_candidate(&event, &mg.public_key()).unwrap();

    assert_eq!(
        observed_status(std::slice::from_ref(&embedded), None),
        OnboardingStatus::AuthorizationObserved {
            authorizations: 1,
            holders: vec![holder.public_key()],
            checked_at: None,
        }
    );
}

#[tokio::test]
async fn support_thread_admits_only_the_mg_fedi_room() {
    let me = Keys::generate();
    let fedi = Keys::generate();
    let stranger = Keys::generate();
    // Every wrap here is addressed to this MG, as its inbox reads them.
    let wrap = |author: &Keys, receivers: Vec<PublicKey>, kind: Kind| {
        let author = author.clone();
        let me = me.public_key();
        async move {
            let mut rumor = EventBuilder::new(kind, "hello")
                .tags(receivers.into_iter().map(Tag::public_key))
                .build(author.public_key());
            let id = rumor.id();
            let event = EventBuilder::gift_wrap(&author, &me, rumor, [])
                .await
                .unwrap();
            (id, event)
        }
    };

    // Fedi's reply and this MG's own copy both join, under the rumor id.
    let (id, from_fedi) = wrap(&fedi, vec![me.public_key()], Kind::PrivateDirectMessage).await;
    let admitted = support::admit(&me, fedi.public_key(), &from_fedi)
        .await
        .unwrap();
    assert!(admitted.from_fedi);
    assert_eq!(admitted.rumor_id, id.to_hex());
    assert_eq!(admitted.body, "hello");
    let rumor = EventBuilder::private_msg_rumor(fedi.public_key(), "mine").build(me.public_key());
    let own_copy = EventBuilder::gift_wrap(&me, &me.public_key(), rumor, [])
        .await
        .unwrap();
    assert!(
        !support::admit(&me, fedi.public_key(), &own_copy)
            .await
            .unwrap()
            .from_fedi
    );

    // A stranger, a group room including Fedi, and a reaction stay out.
    for (author, receivers, kind) in [
        (&stranger, vec![me.public_key()], Kind::PrivateDirectMessage),
        (
            &fedi,
            vec![me.public_key(), stranger.public_key()],
            Kind::PrivateDirectMessage,
        ),
        (&fedi, vec![me.public_key()], Kind::Reaction),
    ] {
        let (_, event) = wrap(author, receivers, kind).await;
        assert_eq!(support::admit(&me, fedi.public_key(), &event).await, None);
    }

    // A stranger's seal around a rumor that claims Fedi wrote it.
    let forged =
        EventBuilder::private_msg_rumor(me.public_key(), "trust me").build(fedi.public_key());
    let forged = EventBuilder::gift_wrap(&stranger, &me.public_key(), forged, [])
        .await
        .unwrap();
    assert_eq!(support::admit(&me, fedi.public_key(), &forged).await, None);
}

#[tokio::test]
async fn support_verbs_answer_from_the_fleet_database() {
    let temp = tempfile::TempDir::new().unwrap();
    let db = mg_core::db::Db::open(temp.path()).await.unwrap();
    let service = ManifoldGuardianNostr::new(
        Keys::generate(),
        None,
        Vec::new(),
        None,
        // Production names Fedi support in its profile, so no policy is needed.
        ManifoldEnvironment::Production.profile().unwrap(),
        Arc::new(FleetHolderAuthorizationStore::new(db.clone())),
        db.clone(),
    );
    let send = |body: String| service.answer(AdminRequest::SendSupportMessage { body });

    // The body is checked before anything else, and its length counts
    // characters, not bytes: 4000 two-byte characters pass to the next check.
    for (body, refusal) in [
        ("  \n".to_owned(), "Write a message first."),
        (
            "é".repeat(4001),
            "A message can have at most 4000 characters.",
        ),
        (
            "é".repeat(4000),
            "This host can't reach guardian support yet. Try again in a minute.",
        ),
    ] {
        assert_eq!(send(body).await.unwrap_err().to_string(), refusal);
    }

    // Relays repeat messages, and two can share a second.
    let row = |id: char, from_fedi: bool, created_at: u64| mg_core::db::SupportRow {
        rumor_id: id.to_string().repeat(64),
        from_fedi,
        body: id.to_string(),
        created_at,
        // Storing ignores it: a new Fedi message is always unread.
        unread: false,
    };
    for message in [
        row('c', true, 100),
        row('b', true, 100),
        row('a', false, 50),
        row('c', true, 100),
    ] {
        db.record_support_message(&message).await.unwrap();
    }
    let chat = service.answer(AdminRequest::SupportChat).await.unwrap();
    assert_eq!(chat["available"], true);
    assert_eq!(chat["unread"], 2, "only Fedi's messages are unread");
    assert_eq!(
        chat["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| (
                message["body"].as_str().unwrap(),
                message["author"].as_str().unwrap()
            ))
            .collect::<Vec<_>>(),
        [("a", "operator"), ("c", "fedi"), ("b", "fedi")],
        "oldest first, then in arrival order, each message once"
    );

    let unread_flags = |chat: &serde_json::Value| {
        chat["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| {
                (
                    message["body"].as_str().unwrap().to_owned(),
                    message["unread"] == true,
                )
            })
            .collect::<Vec<_>>()
    };
    let flags = |list: &[(&str, bool)]| {
        list.iter()
            .map(|(body, unread)| ((*body).to_owned(), *unread))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        unread_flags(&chat),
        flags(&[("a", false), ("c", true), ("b", true)])
    );

    // A mark reads exactly the named messages: not another in the same
    // second, nor one stored later that sorts earlier.
    let mark = |ids: &[char]| {
        service.answer(AdminRequest::MarkSupportRead {
            ids: ids.iter().map(|id| id.to_string().repeat(64)).collect(),
        })
    };
    assert_eq!(
        mark(&['x']).await.unwrap()["unread"],
        2,
        "an unknown id marks nothing"
    );
    assert_eq!(
        mark(&['c', 'a']).await.unwrap()["unread"],
        1,
        "b shares c's second"
    );
    db.record_support_message(&row('d', true, 90))
        .await
        .unwrap();
    assert_eq!(mark(&['b']).await.unwrap()["unread"], 1, "d sorts before b");
    let chat = service.answer(AdminRequest::SupportChat).await.unwrap();
    assert_eq!(
        unread_flags(&chat),
        flags(&[("a", false), ("d", true), ("c", false), ("b", false)])
    );
    assert_eq!(mark(&[]).await.unwrap()["unread"], 1);
}

/// Admit a setup-payment policy that names `support`, or no support key.
fn admit_support_policy(service: &ManifoldGuardianNostr, support: Option<PublicKey>) {
    let mut policy = serde_json::json!({
        "version": 1,
        "fman_version": "0.1.0",
        "federations": [],
        "telemetry_registration_url": "https://push.fedi.example/v1/telemetry/registrations",
    });
    if let Some(support) = support {
        policy["support_nostr_pubkey"] = support.to_hex().into();
    }
    service.inner.setup_payment_federations.send_replace(Some(
        AdmittedSetupPaymentFederations::parse(policy.to_string().as_bytes()).unwrap(),
    ));
}

#[tokio::test]
async fn the_policy_support_key_overrides_the_profile_key() {
    let temp = tempfile::TempDir::new().unwrap();
    let db = mg_core::db::Db::open(temp.path()).await.unwrap();
    let profile = ManifoldEnvironment::Development.profile().unwrap();
    let pinned = *profile.support().expect("development pins a support key");
    let service = ManifoldGuardianNostr::new(
        Keys::generate(),
        None,
        Vec::new(),
        None,
        profile,
        Arc::new(FleetHolderAuthorizationStore::new(db.clone())),
        db,
    );
    assert_eq!(service.inner.support(), Some(pinned), "no policy yet");
    admit_support_policy(&service, None);
    assert_eq!(
        service.inner.support(),
        Some(pinned),
        "the policy names no key"
    );
    let rotated = Keys::generate().public_key();
    admit_support_policy(&service, Some(rotated));
    assert_eq!(service.inner.support(), Some(rotated));
}

/// Scores outside 0..=1000 cannot be advertised; the payload reports none
/// instead of panicking or failing the cycle.
#[test]
fn out_of_range_uptime_is_not_advertised() {
    let mut snapshot = test_snapshot();
    snapshot.uptime = Some(mg_core::seat_readiness::UptimePermille {
        day: 1001,
        week: 0,
        month: 0,
    });
    assert_eq!(build_payload(snapshot, Vec::new()).uptime_permille, None);
    let reported = build_payload(test_snapshot(), Vec::new()).uptime_permille;
    assert_eq!(reported.map(|uptime| uptime.day.get()), Some(998));
}
