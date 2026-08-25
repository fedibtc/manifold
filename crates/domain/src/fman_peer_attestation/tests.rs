//! Tests for FMan peer-attestation signed-object types.

use bitcoin::secp256k1::{PublicKey, SECP256K1, SecretKey};
use nostr::{Keys, nips::nip19::ToBech32, secp256k1::Message};
use stability_pool_common::{Account, AccountType};

use super::*;

fn guardian_fee_account(byte: u8) -> Account {
    Account::single(
        PublicKey::from_secret_key(
            SECP256K1,
            &SecretKey::from_slice(&[byte; 32]).expect("fixed test scalar is valid"),
        ),
        AccountType::BtcDepositor,
    )
}

fn example_statement() -> FmanPeerAttestationStatement {
    FmanPeerAttestationStatement {
        fman_pubkey: Pubkey("fman1".to_owned()),
        federation_id: FederationId("fed1".to_owned()),
        federation_config_hash: HashBytes(vec![1, 2, 3]),
        peer_id: PeerId("0".to_owned()),
        guardian_identity: GuardianIdentity("guardian1".to_owned()),
        guardian_fee_account: guardian_fee_account(1),
        issued_at: Timestamp(42),
    }
}

fn signed_example(keys: &Keys) -> FmanPeerAttestation {
    let mut statement = example_statement();
    statement.fman_pubkey = Pubkey(keys.public_key().to_string());
    let message = Message::from_digest(statement.digest().unwrap());

    FmanPeerAttestation {
        version: ProtocolV1,
        attestation: statement,
        proof: SchnorrSignatureProof {
            signature: keys.sign_schnorr(&message),
        },
    }
}

#[test]
fn canonical_bytes_are_type_tagged_and_versioned() {
    let canonical = String::from_utf8(example_statement().canonical_bytes().unwrap()).unwrap();

    assert_eq!(
        canonical,
        "{\"attestation\":{\"federation_config_hash\":[1,2,3],\"federation_id\":\"fed1\",\"fman_pubkey\":\"fman1\",\"guardian_fee_account\":{\"acc_type\":\"BtcDepositor\",\"pub_keys\":[\"031b84c5567b126440995d3ed5aaba0565d71e1834604819ff9c17f5e9d5dd078f\"],\"threshold\":1},\"guardian_identity\":\"guardian1\",\"issued_at\":42,\"peer_id\":\"0\"},\"type\":\"fedi.fman.peer-attestation\",\"version\":1}"
    );
}

#[test]
fn digest_uses_fman_peer_attestation_domain() {
    assert_eq!(
        example_statement().digest().unwrap(),
        [
            209, 218, 151, 207, 163, 211, 94, 59, 231, 131, 126, 126, 17, 35, 72, 39, 93, 80, 209,
            69, 189, 82, 116, 74, 227, 101, 160, 163, 23, 238, 26, 190,
        ]
    );
}

#[test]
fn seat_endpoint_message_is_domain_then_attestation_digest() {
    assert_eq!(
        example_statement().seat_endpoint_proof_message().unwrap(),
        [
            102, 101, 100, 105, 45, 102, 109, 97, 110, 45, 115, 101, 97, 116, 45, 101, 110, 100,
            112, 111, 105, 110, 116, 45, 112, 114, 111, 111, 102, 47, 118, 49, 0, 209, 218, 151,
            207, 163, 211, 94, 59, 231, 131, 126, 126, 17, 35, 72, 39, 93, 80, 209, 69, 189, 82,
            116, 74, 227, 101, 160, 163, 23, 238, 26, 190,
        ]
    );
}

#[test]
fn verify_accepts_valid_fman_signature() {
    let attestation = signed_example(&Keys::generate());

    assert_eq!(attestation.verify().unwrap(), attestation.attestation);
}

#[test]
fn verify_rejects_wrong_fman_key() {
    let mut attestation = signed_example(&Keys::generate());
    attestation.attestation.fman_pubkey = Pubkey(Keys::generate().public_key().to_string());

    assert_eq!(
        attestation.verify().unwrap_err(),
        FmanPeerAttestationVerificationError::InvalidSignature
    );
}

#[test]
fn verify_rejects_wrong_signature() {
    let keys = Keys::generate();
    let wrong_keys = Keys::generate();
    let mut attestation = signed_example(&keys);
    let message = Message::from_digest([7_u8; 32]);
    attestation.proof.signature = wrong_keys.sign_schnorr(&message);

    assert_eq!(
        attestation.verify().unwrap_err(),
        FmanPeerAttestationVerificationError::InvalidSignature
    );
}

#[test]
fn verify_rejects_an_account_changed_after_signing() {
    let keys = Keys::generate();
    let mut attestation = signed_example(&keys);
    attestation.attestation.guardian_fee_account = guardian_fee_account(2);

    assert_eq!(
        attestation.verify().unwrap_err(),
        FmanPeerAttestationVerificationError::InvalidSignature
    );
}

#[test]
fn verify_rejects_malformed_fman_pubkey() {
    let mut attestation = signed_example(&Keys::generate());
    attestation.attestation.fman_pubkey = Pubkey("not-a-pubkey".to_owned());

    assert_eq!(
        attestation.verify().unwrap_err(),
        FmanPeerAttestationVerificationError::InvalidFmanPubkey
    );
}

#[test]
fn verify_rejects_non_canonical_fman_pubkey_encodings() {
    let keys = Keys::generate();
    let attestation = signed_example(&keys);

    let mut npub_attestation = attestation.clone();
    npub_attestation.attestation.fman_pubkey =
        Pubkey(keys.public_key().to_bech32().expect("npub encodes"));
    assert_eq!(
        npub_attestation.verify().unwrap_err(),
        FmanPeerAttestationVerificationError::InvalidFmanPubkey
    );

    let mut nip21_attestation = attestation.clone();
    nip21_attestation.attestation.fman_pubkey = Pubkey(format!(
        "nostr:{}",
        keys.public_key().to_bech32().expect("npub encodes")
    ));
    assert_eq!(
        nip21_attestation.verify().unwrap_err(),
        FmanPeerAttestationVerificationError::InvalidFmanPubkey
    );

    let mut uppercase_attestation = attestation;
    uppercase_attestation.attestation.fman_pubkey =
        Pubkey(keys.public_key().to_string().to_uppercase());
    assert_eq!(
        uppercase_attestation.verify().unwrap_err(),
        FmanPeerAttestationVerificationError::InvalidFmanPubkey
    );
}
