use crate::db::{Db, NewSeat, now_ms};
use crate::facts::{SeatFacts, SeatNo};

/// Build an authenticated transfer call for lifecycle tests with a mock verifier.
///
/// Issuance is deliberately fake: these tests control trust through that mock.
pub(crate) fn transfer_call(
    request: &fedi_decentralized_service_manifold_guardian::TransferSeatRequest,
    destination: &secp256k1::Keypair,
) -> fedi_decentralized_service_manifold_guardian::TransferSeatCall {
    use fedi_decentralized_service_manifold_guardian::{
        DestinationAdmissionProof, ManagerSignature, MgSignedRequest, TransferSeatCall,
    };
    use peerbadge_protocol::{
        Credential, CredentialProof, HolderAuthorizationRequest, HolderContext, IssuerId,
        ProtocolV1, SignedCredential, SubjectPubkey,
    };
    let holder = HolderContext::generate();
    let subject = nostr_sdk::Keys::generate();
    let credential = SignedCredential {
        version: ProtocolV1,
        credential: Credential {
            issuer_id_pubkey: IssuerId(subject.public_key()),
            info: serde_json::json!({"schema": "fedi-trust-score-v1.0", "trust_level": 9}),
            blind_msg: serde_json::json!(holder.public_key().to_string()),
        },
        proof: CredentialProof {
            signature: blind_rsa_signatures::Signature(vec![1, 2, 3, 4]),
        },
    };
    let envelope = fedi_decentralized_domain::HolderAuthorizationEnvelope {
        holder_authorization: holder
            .authorize_credential_use(
                HolderAuthorizationRequest {
                    subject_pubkey: SubjectPubkey(subject.public_key()),
                },
                &credential,
            )
            .unwrap(),
        signed_credential: credential,
    };
    let signed = MgSignedRequest::create(request, destination).unwrap();
    let digest = DestinationAdmissionProof::signing_digest(&signed, &envelope).unwrap();
    let signature = subject.sign_schnorr(&nostr_sdk::secp256k1::Message::from_digest(digest));
    TransferSeatCall::new(
        signed,
        DestinationAdmissionProof {
            version: 1,
            holder_authorization: envelope,
            signature: ManagerSignature(
                secp256k1::schnorr::Signature::from_slice(signature.as_ref()).unwrap(),
            ),
        },
    )
}

/// Seed the bare seat row needed by tests that exercise post-admission work.
pub(crate) async fn insert_test_seat(db: &Db, new_seat: NewSeat) -> SeatFacts {
    let created_at_ms = now_ms();
    let plan = serde_json_canonicalizer::to_string(&new_seat.plan)
        .expect("Plan always serializes to canonical JSON");
    let mut tx = db.pool().begin().await.unwrap();
    let seat_no = sqlx::query_scalar::<_, i64>(
        "INSERT INTO seats (quote_id, seat_no, fi_id, plan, federation_size, created_at_ms) \
         VALUES (?, (SELECT COALESCE(MAX(seat_no) + 1, 0) FROM seats), ?, ?, ?, ?) \
         RETURNING seat_no",
    )
    .bind(new_seat.seat_id.as_bytes().as_slice())
    .bind(new_seat.fa_id.0.to_string())
    .bind(plan)
    .bind(i64::from(new_seat.federation_size.0))
    .bind(created_at_ms)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    if let Some(payment) = &new_seat.payment {
        let mut evidence = Vec::new();
        ciborium::into_writer(&payment.evidence, &mut evidence)
            .expect("claim evidence serializes as CBOR");
        sqlx::query("INSERT INTO ecash_claims (quote_id, evidence) VALUES (?, ?)")
            .bind(new_seat.seat_id.as_bytes().as_slice())
            .bind(evidence)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();

    SeatFacts {
        seat_id: new_seat.seat_id,
        seat_no: SeatNo(u32::try_from(seat_no).expect("seat_no fits u32")),
        fa_id: new_seat.fa_id,
        plan: new_seat.plan,
        federation_size: new_seat.federation_size,
        created_at_ms,
    }
}

/// A seat data directory in the legacy encrypted layout, as releases before
/// the plaintext one wrote it: `private.encrypt`
/// is hex ChaCha20-Poly1305 under an Argon2 key from the password and salt,
/// beside the salt and a plaintext copy of the password.
pub(crate) async fn write_legacy_layout(
    seat_dir: &std::path::Path,
    api_auth: &str,
    private_json: &str,
) {
    let salt = "c2FsdHNhbHRzYWx0c2E";
    let key = fedimint_aead::get_encryption_key(api_auth, salt).unwrap();
    let encrypted =
        hex::encode(fedimint_aead::encrypt(private_json.as_bytes().to_vec(), &key).unwrap());
    for (name, contents) in [
        ("private.encrypt", encrypted.as_str()),
        ("private.salt", salt),
        ("password.private", &format!("{api_auth}\n")),
        ("local.json", r#"{"api_bind":"127.0.0.1:8174"}"#),
        ("consensus.json", r#"{"api_endpoints":{"0":"wss://peer"}}"#),
    ] {
        tokio::fs::write(seat_dir.join(name), contents)
            .await
            .unwrap();
    }
}
