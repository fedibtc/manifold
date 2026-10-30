use super::*;
use crate::{FaId, FederationId, Locator, QuoteId, SeatId, SeatTransferGrant, SignedRequest};
use peerbadge_protocol::{
    HolderAuthorizationRequest, HolderContext, IssuerContext, PendingIssuance,
};
use secp256k1::Keypair;

/// Frozen pre-extension envelope: do not replace with the current call type.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct LegacyCall {
    service_pubkey: XOnlyPublicKey,
    payload: Vec<u8>,
    manager_signature: ManagerSignature,
}

fn key(byte: u8) -> Keypair {
    Keypair::from_seckey_slice(SECP256K1, &[byte; 32]).unwrap()
}

fn request() -> MgSignedRequest<TransferSeatRequest> {
    let destination = key(2);
    let fa = key(3);
    let grant = SeatTransferGrant {
        ts: Timestamp(1_000),
        fa_id: FaId(fa.x_only_public_key().0),
        federation_id: FederationId("test-federation".into()),
        source: Locator::new(
            iroh::EndpointAddr::new(iroh::SecretKey::from_bytes(&[4; 32]).public()),
            key(5).x_only_public_key().0,
        ),
        source_seat_id: SeatId::from(QuoteId([6; 32])),
        destination_service_pubkey: destination.x_only_public_key().0,
        destination_seat_id: SeatId::from(QuoteId([7; 32])),
    };
    let mut signed = MgSignedRequest::create(
        &TransferSeatRequest {
            ts: Timestamp(1_000),
            destination_service_pubkey: destination.x_only_public_key().0,
            grant: SignedRequest::create(&grant, &fa).unwrap(),
        },
        &destination,
    )
    .unwrap();
    // A valid noncanonical payload proves conversion cannot parse/reserialize.
    let value: serde_json::Value = serde_json::from_slice(&signed.payload).unwrap();
    signed.payload = serde_json::to_vec_pretty(&value).unwrap();
    signed.manager_signature = ManagerSignature(SECP256K1.sign_schnorr(
        &manager_request_signing_digest("transfer_seat", &signed.payload),
        &destination,
    ));
    signed
}

fn envelope() -> HolderAuthorizationEnvelope {
    let issuer = IssuerContext::import_secret_key(
        &serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../domain/src/trust_score/issuer-secret-keys.json"
        )))
        .unwrap(),
    )
    .unwrap();
    let authority = issuer.issuer_authority(Vec::new()).unwrap();
    let holder = HolderContext::generate();
    let info = serde_json::json!({"schema":"fedi-trust-score-v1.0","trust_level":9});
    let (request, pending) = PendingIssuance::create_request(
        &authority.issuer.issuance_key,
        authority.issuer.issuer_id_pubkey.clone(),
        info.clone(),
        serde_json::json!(holder.public_key().to_string()),
    )
    .unwrap();
    let response = issuer.issue_credential(info, &request).unwrap();
    let credential = pending
        .finalize(&authority.issuer.issuance_key, &response)
        .unwrap();
    let subject = key(8).x_only_public_key().0.to_string().parse().unwrap();
    HolderAuthorizationEnvelope {
        holder_authorization: holder
            .authorize_credential_use_at_time(
                HolderAuthorizationRequest {
                    subject_pubkey: subject,
                },
                &credential,
                1_000,
            )
            .unwrap(),
        signed_credential: credential,
    }
}

fn proof(request: &MgSignedRequest<TransferSeatRequest>) -> DestinationAdmissionProof {
    let envelope = envelope();
    DestinationAdmissionProof {
        version: 1,
        signature: ManagerSignature(SECP256K1.sign_schnorr(
            &DestinationAdmissionProof::signing_digest(request, &envelope).unwrap(),
            &key(8),
        )),
        holder_authorization: envelope,
    }
}

fn cbor<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).unwrap();
    bytes
}

#[test]
fn old_and_new_cbor_preserve_exact_signed_bytes_and_binary_fields() {
    let signed = request();
    let legacy = LegacyCall {
        service_pubkey: signed.service_pubkey,
        payload: signed.payload.clone(),
        manager_signature: signed.manager_signature.clone(),
    };
    let old_bytes = cbor(&legacy);
    let call: TransferSeatCall = ciborium::from_reader(&old_bytes[..]).unwrap();
    assert_eq!(call.payload, signed.payload);
    assert!(call.verify(Timestamp(1_000)).unwrap().1.is_none());
    assert_eq!(old_bytes, cbor(&TransferSeatCall::from(signed.clone())));
    let call = TransferSeatCall::new(signed.clone(), Some(proof(&signed)));
    let new_bytes = cbor(&call);
    let old: LegacyCall = ciborium::from_reader(&new_bytes[..]).unwrap();
    assert_eq!(old.payload, signed.payload);
    SECP256K1
        .verify_schnorr(
            &old.manager_signature.0,
            &manager_request_signing_digest("transfer_seat", &old.payload),
            &old.service_pubkey,
        )
        .unwrap();
    let value: ciborium::Value = ciborium::from_reader(&new_bytes[..]).unwrap();
    let fields = value.as_map().unwrap();
    let key = &fields
        .iter()
        .find(|(k, _)| k.as_text() == Some("service_pubkey"))
        .unwrap()
        .1;
    assert_eq!(key.as_array().unwrap().len(), 32);
    let call: TransferSeatCall = ciborium::from_reader(&new_bytes[..]).unwrap();
    call.verify(Timestamp(1_000))
        .unwrap()
        .1
        .unwrap()
        .verify_subject()
        .unwrap();
}

#[test]
fn proof_binds_exact_request_and_envelope_and_never_repairs_service_auth() {
    let signed = request();
    let proof = proof(&signed);
    TransferSeatCall::new(signed.clone(), Some(proof.clone()))
        .verify(Timestamp(1_000))
        .unwrap()
        .1
        .unwrap()
        .verify_subject()
        .unwrap();
    let mut changed = signed.clone();
    let mut payload = changed.verify(Timestamp(1_000)).unwrap();
    payload.ts = Timestamp(1_001);
    changed = MgSignedRequest::create(&payload, &key(2)).unwrap();
    assert!(
        TransferSeatCall::new(changed, Some(proof.clone()))
            .verify(Timestamp(1_001))
            .unwrap()
            .1
            .unwrap()
            .verify_subject()
            .is_err()
    );
    let mut modified = proof.clone();
    modified
        .holder_authorization
        .holder_authorization
        .authorization
        .subject_pubkey
        .0 = key(9).x_only_public_key().0.to_string().parse().unwrap();
    assert!(
        TransferSeatCall::new(signed.clone(), Some(modified))
            .verify(Timestamp(1_000))
            .unwrap()
            .1
            .unwrap()
            .verify_subject()
            .is_err()
    );
    let mut oversized = proof.clone();
    oversized
        .holder_authorization
        .signed_credential
        .credential
        .blind_msg = serde_json::json!("x".repeat(DESTINATION_ADMISSION_MAX_BYTES));
    oversized.signature = ManagerSignature(
        SECP256K1.sign_schnorr(
            &DestinationAdmissionProof::signing_digest(&signed, &oversized.holder_authorization)
                .unwrap(),
            &key(8),
        ),
    );
    assert!(
        TransferSeatCall::new(signed.clone(), Some(oversized))
            .verify(Timestamp(1_000))
            .unwrap()
            .1
            .unwrap()
            .verify_subject()
            .is_err()
    );
    let mut modified = proof.clone();
    modified.version = 2;
    assert!(
        TransferSeatCall::new(signed.clone(), Some(modified))
            .verify(Timestamp(1_000))
            .unwrap()
            .1
            .unwrap()
            .verify_subject()
            .is_err()
    );
    assert!(
        TransferSeatCall::new(signed.clone(), Some(proof.clone()))
            .verify(Timestamp(4_601))
            .is_err()
    );
    let mut invalid = signed.clone();
    invalid.manager_signature = ManagerSignature(SECP256K1.sign_schnorr(&[0; 32], &key(2)));
    assert!(
        TransferSeatCall::new(invalid, Some(proof))
            .verify(Timestamp(1_000))
            .is_err()
    );
}

#[test]
fn duplicate_fields_reject_and_null_proof_is_legacy() {
    let signed = request();
    let call = TransferSeatCall::new(signed.clone(), Some(proof(&signed)));
    let value: ciborium::Value = ciborium::from_reader(&cbor(&call)[..]).unwrap();
    for name in [
        "service_pubkey",
        "payload",
        "manager_signature",
        "destination_proof",
    ] {
        let mut fields = value.as_map().unwrap().clone();
        let duplicate = fields
            .iter()
            .find(|(k, _)| k.as_text() == Some(name))
            .unwrap()
            .clone();
        fields.push(duplicate);
        assert!(
            ciborium::from_reader::<TransferSeatCall, _>(&cbor(&ciborium::Value::Map(fields))[..])
                .is_err()
        );
    }
    let mut fields = value.as_map().unwrap().clone();
    fields
        .iter_mut()
        .find(|(k, _)| k.as_text() == Some("destination_proof"))
        .unwrap()
        .1 = ciborium::Value::Null;
    let call: TransferSeatCall =
        ciborium::from_reader(&cbor(&ciborium::Value::Map(fields))[..]).unwrap();
    assert!(call.verify(Timestamp(1_000)).unwrap().1.is_none());
}

/// Frozen generated legacy method: same name, request and response as deployed.
#[fedi_iroh_rpc::service]
trait LegacyTransferService {
    async fn transfer_seat(
        &self,
        request: LegacyCall,
    ) -> crate::FmResult<crate::TransferSeatResponse>;
}

#[derive(Clone)]
struct LegacySource;

impl LegacyTransferService for LegacySource {
    async fn transfer_seat(
        &self,
        request: LegacyCall,
    ) -> crate::FmResult<crate::TransferSeatResponse> {
        SECP256K1
            .verify_schnorr(
                &request.manager_signature.0,
                &manager_request_signing_digest("transfer_seat", &request.payload),
                &request.service_pubkey,
            )
            .unwrap();
        Err(crate::ManifoldGuardianError::AwaitingBackup)
    }
}

#[fedi_iroh_rpc::service]
trait NewTransferService {
    async fn transfer_seat(
        &self,
        request: TransferSeatCall,
    ) -> crate::FmResult<crate::TransferSeatResponse>;
}

#[derive(Clone)]
struct NewSource;

impl NewTransferService for NewSource {
    async fn transfer_seat(
        &self,
        request: TransferSeatCall,
    ) -> crate::FmResult<crate::TransferSeatResponse> {
        if let Some(admission) = request.verify(Timestamp(1_000))?.1 {
            admission.verify_subject()?;
        }
        Err(crate::ManifoldGuardianError::AwaitingBackup)
    }
}

#[tokio::test]
async fn mixed_version_generated_rpc_uses_same_method_and_response() {
    use fedi_iroh_rpc::IrohProtocol;
    use iroh::{Endpoint, endpoint::presets, protocol::Router};
    const OLD: &[u8] = b"test-old-transfer";
    const NEW: &[u8] = b"test-new-transfer";
    let endpoint = Endpoint::bind(presets::N0DisableRelay).await.unwrap();
    let router = Router::builder(endpoint)
        .accept(
            OLD,
            IrohProtocol::new(LegacyTransferServiceServer::new(LegacySource)),
        )
        .accept(
            NEW,
            IrohProtocol::new(NewTransferServiceServer::new(NewSource)),
        )
        .spawn();
    let client = Endpoint::bind(presets::N0DisableRelay).await.unwrap();
    let old_connection = client.connect(router.endpoint().addr(), OLD).await.unwrap();
    // The real generated new MG client calls the frozen old server.
    let signed = request();
    let response = crate::ManifoldGuardianServiceClient::new(old_connection)
        .transport()
        .transfer_seat(TransferSeatCall::new(signed.clone(), Some(proof(&signed))))
        .await
        .unwrap();
    assert_eq!(
        response.unwrap_err(),
        crate::ManifoldGuardianError::AwaitingBackup
    );
    let new_connection = client.connect(router.endpoint().addr(), NEW).await.unwrap();
    let response = LegacyTransferServiceClient::new(new_connection)
        .transport()
        .transfer_seat(LegacyCall {
            service_pubkey: signed.service_pubkey,
            payload: signed.payload,
            manager_signature: signed.manager_signature,
        })
        .await
        .unwrap();
    assert_eq!(
        response.unwrap_err(),
        crate::ManifoldGuardianError::AwaitingBackup
    );
    router.shutdown().await.unwrap();
    client.close().await;
}
