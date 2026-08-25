use bitcoin::secp256k1::{PublicKey, SecretKey};
use secp256k1::schnorr::Signature;
use stability_pool_common::{Account, AccountType};

use super::*;
use crate::{QuoteId, SeatId};

fn keypair(byte: u8) -> Keypair {
    Keypair::from_seckey_slice(SECP256K1, &[byte; 32]).unwrap()
}

fn fi_key() -> Keypair {
    keypair(7)
}

fn guardian_fee_account() -> crate::GuardianFeeAccount {
    Account::single(
        PublicKey::from_secret_key(
            bitcoin::secp256k1::SECP256K1,
            &SecretKey::from_slice(&[3; 32]).expect("fixed test scalar is valid"),
        ),
        AccountType::BtcDepositor,
    )
    .try_into()
    .unwrap()
}

fn fi_id(key: &Keypair) -> FiId {
    FiId(key.x_only_public_key().0)
}

fn status_request(ts: u64, key: &Keypair) -> GetStatusRequest {
    GetStatusRequest {
        ts: Timestamp(ts),
        fi_id: fi_id(key),
        seat_id: SeatId::from(QuoteId([0x0a; 32])),
    }
}

#[test]
fn request_round_trips_through_verify() {
    let key = fi_key();
    let request = status_request(1_000, &key);
    let envelope = SignedRequest::create(&request, &key).unwrap();

    let validated = envelope.verify(Timestamp(1_000)).unwrap();
    assert_eq!(*validated, request);
    assert_eq!(validated.into_inner(), request);
}

#[test]
fn external_signer_builds_the_same_verified_envelope() {
    let key = fi_key();
    let request = status_request(1_000, &key);
    let envelope = SignedRequest::create_with_signer(&request, |digest| {
        Ok(FiSignature(SECP256K1.sign_schnorr(&digest, &key)))
    })
    .unwrap();

    assert_eq!(*envelope.verify(Timestamp(1_000)).unwrap(), request);
}

#[test]
fn tampered_payload_and_wrong_key_fail() {
    let key = fi_key();
    let request = status_request(1_000, &key);
    let mut envelope = SignedRequest::create(&request, &key).unwrap();

    let other_key = keypair(9);
    let forged = SignedRequest::<GetStatusRequest> {
        fi_id: request.fi_id,
        payload: envelope.payload.clone(),
        fi_signature: FiSignature(SECP256K1.sign_schnorr(
            &fi_request_signing_digest(GetStatusRequest::LABEL, &envelope.payload),
            &other_key,
        )),
        marker: PhantomData,
    };
    assert!(matches!(
        forged.verify(Timestamp(1_000)),
        Err(AuthError::BadSignature)
    ));

    envelope.payload = serde_json::to_vec(&status_request(1_001, &key)).unwrap();
    assert!(matches!(
        envelope.verify(Timestamp(1_000)),
        Err(AuthError::BadSignature)
    ));
}

#[test]
fn per_verb_labels_are_unique_and_nul_free() {
    // Cross-verb replay protection is exactly as strong as label
    // uniqueness, and the `\0` delimiter after the label is unambiguous
    // only if no label contains NUL. The rosters come from the impl
    // macros, so a verb added there is checked here automatically.
    for labels in [FI_REQUEST_LABELS, MANAGER_RESPONSE_LABELS] {
        let unique: std::collections::BTreeSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "duplicate label in {labels:?}");
        for label in labels {
            assert!(!label.contains('\0'), "label {label:?} contains NUL");
        }
    }
}

#[test]
fn payload_signer_id_must_match_envelope_signer_id() {
    // An attacker with their own valid key signs a payload claiming a
    // victim's fi_id: the signature verifies (their key), so only the
    // inner/outer id comparison stops the impersonation.
    let attacker_key = fi_key();
    let victim_id = fi_id(&keypair(9));
    let mut request = status_request(1_000, &attacker_key);
    request.fi_id = victim_id;
    let payload = serde_json::to_vec(&request).unwrap();
    let envelope = SignedRequest::<GetStatusRequest> {
        fi_id: fi_id(&attacker_key),
        fi_signature: FiSignature(SECP256K1.sign_schnorr(
            &fi_request_signing_digest(GetStatusRequest::LABEL, &payload),
            &attacker_key,
        )),
        payload,
        marker: PhantomData,
    };

    assert!(matches!(
        envelope.verify(Timestamp(1_000)),
        Err(AuthError::SignerMismatch)
    ));
}

#[test]
fn validly_signed_garbage_payload_fails_as_payload_error() {
    let key = fi_key();
    let payload = b"not json".to_vec();
    let envelope = SignedRequest::<GetStatusRequest> {
        fi_id: fi_id(&key),
        fi_signature: FiSignature(SECP256K1.sign_schnorr(
            &fi_request_signing_digest(GetStatusRequest::LABEL, &payload),
            &key,
        )),
        payload,
        marker: PhantomData,
    };

    assert!(matches!(
        envelope.verify(Timestamp(1_000)),
        Err(AuthError::Payload { .. })
    ));
}

#[test]
fn payload_error_does_not_echo_rejected_field_name() {
    let key = fi_key();
    let rejected = "forged\n2026-08-18T00:00:00Z WARN accepted request";
    let mut payload = serde_json::Map::new();
    payload.insert(rejected.to_owned(), serde_json::Value::Null);
    let payload = serde_json::to_vec(&payload).unwrap();
    let envelope = SignedRequest::<RestartDkgRequest> {
        fi_id: fi_id(&key),
        fi_signature: FiSignature(SECP256K1.sign_schnorr(
            &fi_request_signing_digest(RestartDkgRequest::LABEL, &payload),
            &key,
        )),
        payload,
        marker: PhantomData,
    };

    let error = envelope.verify(Timestamp(1_000)).unwrap_err();

    assert!(matches!(error, AuthError::Payload { .. }));
    assert_eq!(
        error.to_string(),
        "payload is not a valid restart_dkg request"
    );
    assert!(!error.to_string().contains(rejected));
}

#[test]
fn cross_verb_replay_is_rejected() {
    // GetStatusRequest and GetInviteCodeRequest share the exact same
    // field shape; the per-verb label must keep their signatures apart.
    let key = fi_key();
    let envelope = SignedRequest::create(&status_request(1_000, &key), &key).unwrap();

    let replayed = SignedRequest::<GetInviteCodeRequest> {
        fi_id: envelope.fi_id,
        payload: envelope.payload.clone(),
        fi_signature: envelope.fi_signature.clone(),
        marker: PhantomData,
    };
    assert!(matches!(
        replayed.verify(Timestamp(1_000)),
        Err(AuthError::BadSignature)
    ));
}

#[test]
fn bad_signature_is_rejected_before_payload_parse() {
    let key = fi_key();
    let envelope = SignedRequest::<GetStatusRequest> {
        fi_id: fi_id(&key),
        payload: b"not json".to_vec(),
        fi_signature: FiSignature(Signature::from_byte_array([0; 64])),
        marker: PhantomData,
    };

    assert!(matches!(
        envelope.verify(Timestamp(1_000)),
        Err(AuthError::BadSignature)
    ));
}

#[test]
fn freshness_window_is_one_hour_both_ways() {
    let key = fi_key();
    let envelope = SignedRequest::create(&status_request(10_000, &key), &key).unwrap();

    assert!(envelope.verify(Timestamp(10_000 + 3_600)).is_ok());
    assert!(envelope.verify(Timestamp(10_000 - 3_600)).is_ok());
    assert!(matches!(
        envelope.verify(Timestamp(10_000 + 3_601)),
        Err(AuthError::Stale)
    ));
    assert!(matches!(
        envelope.verify(Timestamp(10_000 - 3_601)),
        Err(AuthError::Stale)
    ));
}

#[test]
fn response_round_trips_and_replays_from_parts() {
    let manager_key = keypair(5);
    let pubkey = manager_key.x_only_public_key().0;
    let response = CreateSeatResponse {
        quote_id: crate::QuoteId([8; 32]),
        outcome: crate::CreateSeatOutcome::Accepted {
            seat_id: crate::SeatId::from(crate::QuoteId([0x0b; 32])),
            guardian_fee_account: guardian_fee_account(),
        },
    };

    let envelope = SignedResponse::create(&response, &manager_key).unwrap();
    assert_eq!(envelope.verify(&pubkey).unwrap().into_inner(), response);

    let replayed = SignedResponse::<CreateSeatResponse>::from_parts(
        envelope.payload.clone(),
        envelope.manager_signature.clone(),
    );
    assert_eq!(replayed.verify(&pubkey).unwrap().into_inner(), response);

    let wrong_key = keypair(6).x_only_public_key().0;
    assert!(matches!(
        replayed.verify(&wrong_key),
        Err(AuthError::BadSignature)
    ));
}
