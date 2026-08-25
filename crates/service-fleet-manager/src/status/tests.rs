use bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
use sha2::{Digest as _, Sha256};
use stability_pool_common::{Account, AccountType};

use super::*;

fn formation_binding() -> FormationSeatBinding {
    use fedi_decentralized_services::domain::{
        FederationId, FmanPeerAttestationStatement, GuardianIdentity, HashBytes, PeerId,
        ProtocolV1, Pubkey, SchnorrSignatureProof,
    };
    use nostr::secp256k1::Message;

    let keys = nostr::Keys::generate();
    let attestation = FmanPeerAttestationStatement {
        fman_pubkey: Pubkey(keys.public_key().to_string()),
        federation_id: FederationId("federation".to_owned()),
        federation_config_hash: HashBytes(vec![1; 32]),
        peer_id: PeerId("0".to_owned()),
        guardian_identity: GuardianIdentity("guardian".to_owned()),
        guardian_fee_account: raw_account(1),
        issued_at: Timestamp(1_700_000_000),
    };
    let signature = keys.sign_schnorr(&Message::from_digest(attestation.digest().unwrap()));
    FormationSeatBinding {
        endpoint_proof: SeatEndpointProof {
            signature: vec![0; 64],
        },
        attestation: FmanPeerAttestation {
            version: ProtocolV1,
            attestation,
            proof: SchnorrSignatureProof { signature },
        },
    }
}

fn raw_account(byte: u8) -> Account {
    Account::single(
        PublicKey::from_secret_key(
            &Secp256k1::new(),
            &SecretKey::from_slice(&[byte; 32]).expect("fixed test scalar is valid"),
        ),
        AccountType::BtcDepositor,
    )
}

fn account(byte: u8) -> GuardianFeeAccount {
    raw_account(byte).try_into().unwrap()
}

fn manifold_4_1_1(guardian_count: u8) -> Vec<GuardianFeeRecipient> {
    let mut recipients = (1..=guardian_count)
        .map(|byte| GuardianFeeRecipient::new(account(byte), GUARDIAN_GUARDIAN_FEE_WEIGHT))
        .chain([
            GuardianFeeRecipient::new(account(30), FI_GUARDIAN_FEE_WEIGHT),
            GuardianFeeRecipient::new(account(31), FEDI_GUARDIAN_FEE_WEIGHT),
        ])
        .collect::<Vec<_>>();
    recipients.sort_by_key(|recipient| recipient.account.as_account().id());
    recipients
}

#[test]
fn canonical_4_1_1_vectors_pin_the_fedi_wire() {
    let expected = [
        (
            7,
            "090e4fb9be3eb36b6c28fb03f87632f422b8400e10a3813fa1a762d8c84020e2",
        ),
        (
            10,
            "19e15c457a7b022bea5b4cde2768c2734fdf491660be75c84b84e7270317ba68",
        ),
        (
            13,
            "9e111a41b2939368c0cb97faf9bb97eadd829aff127f60a43d331b08881535fd",
        ),
        (
            20,
            "1d57a306dd1ddf21ffd939849a47fc2abea9a87906a877c33f7bdfee3462397e",
        ),
    ];
    let actual = expected
        .iter()
        .map(|(guardian_count, _)| {
            let recipients = manifold_4_1_1(*guardian_count);
            let value = canonical_guardian_fee_recipient_list(&recipients).unwrap();
            assert_eq!(
                recipients.iter().map(|entry| entry.weight).sum::<u64>(),
                u64::from(*guardian_count) + FI_GUARDIAN_FEE_WEIGHT + FEDI_GUARDIAN_FEE_WEIGHT,
            );
            (
                *guardian_count,
                hex::encode(Sha256::digest(value.as_bytes())),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        expected
            .into_iter()
            .map(|(count, digest)| (count, digest.to_owned()))
            .collect::<Vec<_>>(),
    );
}

#[test]
fn formation_meta_response_is_only_a_vote_acknowledgement() {
    assert_eq!(
        serde_json::to_value(ProposeFormationMetaResponse).unwrap(),
        serde_json::Value::Null,
    );
}

#[test]
fn formation_meta_request_rejects_too_many_bindings_during_deserialization() {
    let secret = secp256k1::SecretKey::from_slice(&[2; 32]).unwrap();
    let fi_id = FiId(
        secp256k1::PublicKey::from_secret_key(&secp256k1::SECP256K1, &secret)
            .x_only_public_key()
            .0,
    );
    let request = ProposeFormationMetaRequest {
        ts: Timestamp(1_700_000_000),
        fi_id,
        seat_id: SeatId::from(crate::QuoteId([3; 32])),
        expected_base: MetaConsensusBase::Absent,
        seat_bindings: vec![formation_binding(); FMAN_SEAT_BINDINGS_MAX_COUNT],
        fi_fee_account: account(2),
        fedi_fee_account: account(3),
        send_ppm: 5_000,
    };
    let mut value = serde_json::to_value(request).unwrap();
    assert!(
        serde_json::from_value::<ProposeFormationMetaRequest>(value.clone()).is_ok(),
        "the exact count limit must remain admissible"
    );
    value["seat_bindings"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::to_value(formation_binding()).unwrap());
    let error = serde_json::from_value::<ProposeFormationMetaRequest>(value).unwrap_err();
    assert!(
        error.to_string().contains("exceed maximum count"),
        "{error}"
    );
}

#[test]
fn semantic_account_rejects_wrong_type_and_multisig() {
    let key = *raw_account(1).as_single().unwrap();
    assert_eq!(
        GuardianFeeAccount::try_from(Account::single(key, AccountType::Provider)),
        Err(GuardianFeeAccountError),
    );

    let multisig: Result<GuardianFeeAccount, _> = serde_json::from_value(serde_json::json!({
        "acc_type": "BtcDepositor",
        "pub_keys": [raw_account(1).as_single().unwrap(), raw_account(2).as_single().unwrap()],
        "threshold": 2,
    }));
    assert!(multisig.is_err());
}

#[test]
fn repeated_account_id_weight_and_canonical_order_are_enforced() {
    let mut recipients = manifold_4_1_1(7);
    recipients[0].account_id = recipients[1].account_id.clone();
    assert_eq!(
        canonical_guardian_fee_recipient_list(&recipients),
        Err(GuardianFeeRecipientListError::AccountIdMismatch),
    );

    let mut recipients = manifold_4_1_1(7);
    recipients.swap(0, 1);
    assert_eq!(
        canonical_guardian_fee_recipient_list(&recipients),
        Err(GuardianFeeRecipientListError::NotCanonical),
    );

    let mut recipients = manifold_4_1_1(7);
    recipients[0].weight = 0;
    assert_eq!(
        canonical_guardian_fee_recipient_list(&recipients),
        Err(GuardianFeeRecipientListError::InvalidWeight),
    );
}

#[test]
fn duplicate_full_accounts_are_refused_as_non_canonical() {
    let mut recipients = manifold_4_1_1(7);
    let fi_index = recipients
        .iter()
        .position(|recipient| recipient.weight == FI_GUARDIAN_FEE_WEIGHT)
        .unwrap();
    let guardian_index = recipients
        .iter()
        .position(|recipient| recipient.weight == GUARDIAN_GUARDIAN_FEE_WEIGHT)
        .unwrap();
    recipients[fi_index] = GuardianFeeRecipient::new(
        recipients[guardian_index].account.clone(),
        FI_GUARDIAN_FEE_WEIGHT,
    );
    recipients.sort_by_key(|recipient| recipient.account.as_account().id());
    assert_eq!(
        canonical_guardian_fee_recipient_list(&recipients),
        Err(GuardianFeeRecipientListError::NotCanonical),
    );
}

#[test]
fn dkg_status_has_only_peer_connections() {
    let info: DkgStatusInfo = serde_json::from_value(serde_json::json!({
        "peer_connections": [],
    }))
    .unwrap();

    assert_eq!(
        serde_json::to_value(info).unwrap(),
        serde_json::json!({ "peer_connections": [] }),
    );
}
