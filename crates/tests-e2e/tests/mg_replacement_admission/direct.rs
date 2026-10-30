//! Direct admission exercises real issuer revocation but never registry discovery.

use super::*;
use fedi_decentralized_service_manifold_guardian::{
    DestinationAdmission, DestinationAdmissionProof, FaId, FederationId, Locator, ManagerSignature,
    MgSignedRequest, QuoteId, SeatId, SeatTransferGrant, SignedRequest, Timestamp,
    TransferSeatCall, TransferSeatRequest,
};
use secp256k1::{Keypair, SECP256K1};

const CHILD_DIRECT: &str = "MG_DIRECT_ADMISSION_CHILD_RELAY";

#[tokio::test]
async fn request_bound_admission_never_discovers_and_rechecks_revocation() {
    if let Ok(relay) = std::env::var(CHILD_DIRECT) {
        check_direct(&relay).await;
        return;
    }
    let mut client = AsyncDefeClient::connect_from_env().await.unwrap();
    let lease = client
        .request_nostr_relay(SharingMode::Exclusive)
        .await
        .unwrap();
    let ResourceDescriptor::NostrRelay(relay) = &lease.descriptor else {
        panic!("expected relay");
    };
    let probe = relay_probe::RelayProbe::start(relay.url.clone()).await;
    let status = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "direct::request_bound_admission_never_discovers_and_rechecks_revocation",
                "--nocapture",
            ])
            .env(CHILD_DIRECT, &relay.url)
            .env(DEV_NOSTR_RELAYS_ENV, &probe.url)
            .kill_on_drop(true)
            .status(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(status.success());
    assert_eq!(
        probe
            .discovery_reads
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        probe
            .revocation_reads
            .load(std::sync::atomic::Ordering::SeqCst),
        5
    );
}

fn signed_request(destination: &Keypair, timestamp: u64) -> MgSignedRequest<TransferSeatRequest> {
    let fa = Keypair::from_seckey_slice(SECP256K1, &[21; 32]).unwrap();
    let grant = SeatTransferGrant {
        ts: Timestamp(timestamp),
        fa_id: FaId(fa.x_only_public_key().0),
        federation_id: FederationId("direct-test".into()),
        source: Locator::new(
            fedi_iroh_rpc::iroh::EndpointAddr::new(
                fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[22; 32]).public(),
            ),
            fa.x_only_public_key().0,
        ),
        source_seat_id: SeatId::from(QuoteId([23; 32])),
        destination_service_pubkey: destination.x_only_public_key().0,
        destination_seat_id: SeatId::from(QuoteId([24; 32])),
    };
    MgSignedRequest::create(
        &TransferSeatRequest {
            ts: Timestamp(timestamp),
            destination_service_pubkey: destination.x_only_public_key().0,
            grant: SignedRequest::create(&grant, &fa).unwrap(),
        },
        destination,
    )
    .unwrap()
}

fn call(
    request: MgSignedRequest<TransferSeatRequest>,
    mg: &Keys,
    envelope: &HolderAuthorizationEnvelope,
) -> TransferSeatCall {
    let digest = DestinationAdmissionProof::signing_digest(&request, envelope).unwrap();
    let signature = mg.sign_schnorr(&nostr_sdk::secp256k1::Message::from_digest(digest));
    TransferSeatCall::new(
        request,
        DestinationAdmissionProof {
            version: 1,
            holder_authorization: envelope.clone(),
            signature: ManagerSignature(
                secp256k1::schnorr::Signature::from_slice(signature.as_ref()).unwrap(),
            ),
        },
    )
}

fn admission(call: TransferSeatCall, now: u64) -> DestinationAdmission {
    call.verify(Timestamp(now)).unwrap().1
}

async fn check_direct(relay: &str) {
    let profile = ManifoldEnvironment::Development.profile().unwrap();
    let issuer = IssuerContext::import_secret_key(
        &serde_json::from_str(profile.test_issuer_secret_keys().unwrap()).unwrap(),
    )
    .unwrap();
    let verifier = NostrReplacementGuardianVerifier::new(profile).unwrap();
    let holder = HolderContext::generate();
    let mg = Keys::generate();
    let service = Keypair::from_seckey_slice(SECP256K1, &[25; 32]).unwrap();
    let now = nostr_sdk::Timestamp::now().as_secs();
    let envelope = issue(&issuer, &holder, mg.public_key(), 9);
    let request = signed_request(&service, now);
    let signed_call = call(request.clone(), &mg, &envelope);
    // No advertisement exists. Full/closed purchase availability cannot matter.
    for _ in 0..2 {
        verifier
            .verify(&admission(signed_call.clone(), now))
            .await
            .unwrap();
    }
    // A new service-signed request does not inherit a subject proof.
    let mut value = serde_json::to_value(&signed_call).unwrap();
    let changed =
        serde_json::to_value(call(signed_request(&service, now + 1), &mg, &envelope)).unwrap();
    value["request"] = changed["request"].clone();
    let mismatched: TransferSeatCall = serde_json::from_value(value).unwrap();
    assert!(verifier.verify(&admission(mismatched, now)).await.is_err());
    let borrowed = call(request.clone(), &Keys::generate(), &envelope);
    assert!(verifier.verify(&admission(borrowed, now)).await.is_err());
    let other_envelope = issue(&issuer, &holder, mg.public_key(), 9);
    let mut swapped = serde_json::to_value(&signed_call).unwrap();
    swapped["destination_proof"]["holder_authorization"] =
        serde_json::to_value(other_envelope).unwrap();
    let swapped: TransferSeatCall = serde_json::from_value(swapped).unwrap();
    assert!(verifier.verify(&admission(swapped, now)).await.is_err());
    let mut unsupported = serde_json::to_value(&signed_call).unwrap();
    unsupported["destination_proof"]["version"] = json!(2);
    let unsupported: TransferSeatCall = serde_json::from_value(unsupported).unwrap();
    assert!(verifier.verify(&admission(unsupported, now)).await.is_err());
    let mut oversized = envelope.clone();
    oversized.signed_credential.credential.blind_msg = json!(
        "x".repeat(fedi_decentralized_service_manifold_guardian::DESTINATION_ADMISSION_MAX_BYTES)
    );
    assert!(
        verifier
            .verify(&admission(call(request.clone(), &mg, &oversized), now))
            .await
            .is_err()
    );
    let mut forged = envelope.clone();
    forged.signed_credential.proof.signature.0[0] ^= 1;
    assert!(
        verifier
            .verify(&admission(call(request.clone(), &mg, &forged), now))
            .await
            .is_err()
    );
    let low = issue(&issuer, &holder, mg.public_key(), 8);
    assert!(
        verifier
            .verify(&admission(call(request.clone(), &mg, &low), now))
            .await
            .is_err()
    );
    // A later lower-trust authorization is not issuer revocation of this badge.
    verifier
        .verify(&admission(call(request.clone(), &mg, &envelope), now))
        .await
        .unwrap();
    let revocation = issuer
        .revoke_credential(&envelope.signed_credential)
        .unwrap();
    let publisher = NostrRelayClient::connect(
        relay,
        Keys::parse(&issuer.export_secret_key().unwrap().issuer_id_secret_key).unwrap(),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    publisher
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
    assert!(verifier.verify(&admission(signed_call, now)).await.is_err());
}
