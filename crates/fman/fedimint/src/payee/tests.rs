use super::*;

use bitcoin_hashes::Hash as _;
use fedi_decentralized_service_fleet_manager::InviteCode as WireInviteCode;
use fedi_decentralized_service_fleet_manager::MintGeneration;
use fedimint_client_module::oplog::{JsonStringed, OperationLogEntry};
use fedimint_core::PeerId;
use fedimint_core::{IdxRange, OutPointRange, TransactionId};

fn federation_id(byte: u8) -> FederationId {
    format!("{byte:02x}").repeat(32).parse().unwrap()
}

fn federation_invite(federation_id: FederationId) -> WireInviteCode {
    WireInviteCode(
        fedimint_core::invite_code::InviteCode::new(
            fedimint_core::util::SafeUrl::parse("https://payment.example").unwrap(),
            PeerId::from(0),
            federation_id,
            None,
        )
        .to_string(),
    )
}

fn payment_terms(generation: MintGeneration, issuance_count: usize) -> PaymentTerms {
    let federation_id = WireFederationId("payment-federation".to_owned());
    match generation {
        MintGeneration::MintV1 => PaymentTerms::MintV1 {
            federation_id,
            issuance: (0..issuance_count)
                .map(|_| LockedIssuanceRequest {
                    amount_msats: 1_000,
                    blind_nonce: vec![],
                })
                .collect(),
        },
        MintGeneration::MintV2 => PaymentTerms::MintV2 {
            federation_id,
            issuance: (0..issuance_count)
                .map(|_| LockedIssuanceRequestV2 {
                    amount_msats: 1_000,
                    blind_nonce: vec![],
                    tweak: [0; 16],
                })
                .collect(),
        },
    }
}

fn signatures(count: usize) -> Vec<LockedBlindedSignature> {
    (0..count).map(|_| LockedBlindedSignature(vec![])).collect()
}

#[test]
fn remote_signature_count_mismatches_skip_the_decoder() {
    for (generation, signature_count) in [
        (MintGeneration::MintV1, 1),
        (MintGeneration::MintV1, 3),
        (MintGeneration::MintV2, 1),
        (MintGeneration::MintV2, 3),
    ] {
        let payment = payment_terms(generation, 2);
        let mut decoded = 0;

        let result =
            decode_remote_payment_signatures(&payment, &signatures(signature_count), |_| {
                decoded += 1;
                Ok(())
            });

        assert!(matches!(result, Err(LockedPaymentPrepareError::Invalid)));
        assert_eq!(decoded, 0);
    }
}

#[test]
fn remote_matching_signature_counts_decode_every_signature() {
    for generation in [MintGeneration::MintV1, MintGeneration::MintV2] {
        let payment = payment_terms(generation, 2);

        let decoded =
            decode_remote_payment_signatures(&payment, &signatures(2), |_| Ok(())).unwrap();

        assert_eq!(decoded.len(), 2);
    }
}

#[test]
fn retained_signature_count_mismatches_are_corrupt_state() {
    for (generation, signature_count) in [
        (MintGeneration::MintV1, 1),
        (MintGeneration::MintV1, 3),
        (MintGeneration::MintV2, 1),
        (MintGeneration::MintV2, 3),
    ] {
        let federation_invite = federation_invite(federation_id(42));
        let claim = match payment_terms(generation, 2) {
            PaymentTerms::MintV1 { issuance, .. } => EcashClaimEvidence::MintV1 {
                federation_invite,
                module_id: 1,
                quote_nonce: [0; 32],
                issuance,
                signatures: signatures(signature_count),
            },
            PaymentTerms::MintV2 { issuance, .. } => EcashClaimEvidence::MintV2 {
                federation_invite,
                module_id: 1,
                issuance,
                signatures: signatures(signature_count),
            },
        };
        assert!(
            validate_claim_evidence(&claim)
                .unwrap_err()
                .to_string()
                .contains("signature count mismatch")
        );
    }
}

#[test]
fn claim_evidence_is_minimal_and_retains_recovery_fields() {
    let federation_invite = federation_invite(federation_id(42));
    let evidence = EcashClaimEvidence::MintV1 {
        federation_invite: federation_invite.clone(),
        module_id: 17,
        quote_nonce: [9; 32],
        issuance: Vec::new(),
        signatures: Vec::new(),
    };
    let serialized = serde_json::to_string(&evidence).unwrap();
    assert!(serialized.contains("\"module_id\":17"));
    assert!(serialized.contains(&federation_invite.0));
    for quote_only_field in [
        "offer_epoch",
        "fedimintd_version",
        "federation_size",
        "refund_issuance",
        "price_msats",
        "plan",
    ] {
        assert!(!serialized.contains(quote_only_field), "{quote_only_field}");
    }

    let decoded: EcashClaimEvidence = serde_json::from_str(&serialized).unwrap();
    assert!(matches!(
        decoded,
        EcashClaimEvidence::MintV1 { module_id: 17, .. }
    ));
}

#[test]
fn claim_evidence_rejects_unknown_variants_fields_and_malformed_records() {
    for serialized in [
        r#"{"mint":"mint_v3"}"#,
        r#"{"mint":"mint_v1","commercial_terms":"must not be retained","federation_invite":"test-invite","module_id":1,"quote_nonce":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"issuance":[],"signatures":[]}"#,
        "not json",
    ] {
        assert!(serde_json::from_str::<EcashClaimEvidence>(serialized).is_err());
    }
}

#[test]
fn claim_evidence_requires_the_federation_invite() {
    let serialized = r#"{"mint":"mint_v1","module_id":1,"quote_nonce":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"issuance":[],"signatures":[]}"#;
    assert!(serde_json::from_str::<EcashClaimEvidence>(serialized).is_err());
}

#[test]
fn claim_evidence_rejects_an_unparseable_invite() {
    let claim = EcashClaimEvidence::MintV1 {
        federation_invite: WireInviteCode("not-an-invite".to_owned()),
        module_id: 1,
        quote_nonce: [0; 32],
        issuance: Vec::new(),
        signatures: Vec::new(),
    };
    assert!(validate_claim_evidence(&claim).is_err());
}

#[test]
fn mint_v1_reissue_operation_id_derivation_is_pinned() {
    let notes = OOBNotes::new(federation_id(42).to_prefix(), TieredMulti::default());
    let actual = crate::reissue_operation_id(&notes).0;
    assert_eq!(
        actual,
        [
            0x59, 0x8e, 0x91, 0xbe, 0x0b, 0x83, 0x4e, 0xc0, 0xa8, 0x50, 0xd6, 0xa9, 0xc2, 0xb6,
            0xd0, 0xe6, 0x1c, 0x8f, 0xfc, 0xf6, 0x2c, 0x5b, 0x70, 0x0f, 0xa8, 0x83, 0x30, 0xf8,
            0xf7, 0x20, 0xa3, 0x39,
        ]
    );
}

fn mint_v2_operation(
    kind: &str,
    metadata: serde_json::Value,
) -> fedimint_client_module::oplog::OperationLogEntry {
    fedimint_client_module::oplog::OperationLogEntry::new(
        kind.to_owned(),
        JsonStringed(metadata),
        None,
    )
}

fn mint_v2_receive_operation(encoded_ecash: &str) -> OperationLogEntry {
    let metadata = MintV2OperationMeta::Receive {
        change_outpoint_range: OutPointRange::new(TransactionId::all_zeros(), IdxRange::from(0..1)),
        ecash: encoded_ecash.to_owned(),
        custom_meta: serde_json::Value::Null,
    };
    mint_v2_operation(
        fedimint_mintv2_common::KIND.as_str(),
        serde_json::to_value(metadata).unwrap(),
    )
}

#[tokio::test]
async fn initial_mint_v2_receive_and_exact_replay_converge_on_one_operation() {
    let ecash = MintV2Ecash::new(federation_id(42), vec![]);
    let operation_id = OperationId::from_encodable(&ecash);
    let encoded_ecash = base32::encode_prefixed(FEDIMINT_PREFIX, &ecash);
    let existing = mint_v2_receive_operation(&encoded_ecash);

    assert_eq!(
        handoff_mint_v2_receive(
            &ecash,
            || async { Ok(operation_id) },
            |_| async { panic!("successful initial receive must not read the operation log") },
        )
        .await,
        Ok(operation_id)
    );
    assert_eq!(
        handoff_mint_v2_receive(
            &ecash,
            || async { Err(MintV2ReceiveError::AlreadyReceived) },
            |derived_operation_id| async move {
                assert_eq!(derived_operation_id, operation_id);
                Some(existing)
            },
        )
        .await,
        Ok(operation_id)
    );
}

#[tokio::test]
async fn mint_v2_replay_requires_exact_receive_metadata() {
    let ecash = MintV2Ecash::new(federation_id(42), vec![]);
    let encoded_ecash = base32::encode_prefixed(FEDIMINT_PREFIX, &ecash);
    let receive_other_ecash = mint_v2_receive_operation("fedimint-other-ecash");
    let malformed = mint_v2_operation(
        fedimint_mintv2_common::KIND.as_str(),
        serde_json::json!({"Receive": "malformed"}),
    );
    let wrong_kind = mint_v2_operation(
        fedimint_mint_common::KIND.as_str(),
        serde_json::to_value(MintV2OperationMeta::Receive {
            change_outpoint_range: OutPointRange::new(
                TransactionId::all_zeros(),
                IdxRange::from(0..1),
            ),
            ecash: encoded_ecash.clone(),
            custom_meta: serde_json::Value::Null,
        })
        .unwrap(),
    );
    let send = MintV2OperationMeta::Send {
        ecash: encoded_ecash,
        custom_meta: serde_json::Value::Null,
    };
    let send = mint_v2_operation(
        fedimint_mintv2_common::KIND.as_str(),
        serde_json::to_value(send).unwrap(),
    );

    for existing in [receive_other_ecash, malformed, wrong_kind, send] {
        assert_eq!(
            handoff_mint_v2_receive(
                &ecash,
                || async { Err(MintV2ReceiveError::AlreadyReceived) },
                |_| async { Some(existing) },
            )
            .await,
            Err(MintV2ReceiveError::AlreadyReceived)
        );
    }
    assert_eq!(
        handoff_mint_v2_receive(
            &ecash,
            || async { Err(MintV2ReceiveError::AlreadyReceived) },
            |_| async { None },
        )
        .await,
        Err(MintV2ReceiveError::AlreadyReceived)
    );
}
