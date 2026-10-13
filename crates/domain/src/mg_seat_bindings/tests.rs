//! Tests for the `fedi:fman_seat_bindings` consensus-metadata container.

use bitcoin::secp256k1::{PublicKey, SECP256K1, SecretKey};
use nostr::secp256k1::schnorr::Signature;
use nostr::{Keys, secp256k1::Message};
use stability_pool_common::{Account, AccountType};

use super::*;
use crate::federation_config::{FederationSeat, federation_seats};
use crate::test_support::test_config;
use crate::{
    FederationId, HashBytes, MgPeerAttestationStatement, SchnorrSignatureProof, Timestamp,
};

fn guardian_fee_account(byte: u8) -> Account {
    Account::single(
        PublicKey::from_secret_key(
            SECP256K1,
            &SecretKey::from_slice(&[byte; 32]).expect("fixed test scalar is valid"),
        ),
        AccountType::BtcDepositor,
    )
}

/// Fixed proof bytes for tests that only exercise structural validation.
///
/// Structural validation never inspects a signature, so these stay
/// deterministic — which is what lets the canonical value be pinned.
fn stub_proof() -> SchnorrSignatureProof {
    SchnorrSignatureProof {
        signature: Signature::from_slice(&[7_u8; 64]).expect("64 bytes is a schnorr signature"),
    }
}

fn federation(peers: usize) -> FederationSeats {
    federation_seats(&test_config(peers)).expect("fixture config derives seats")
}

/// Copy federation facts with the seat list replaced.
fn with_seats(base: &FederationSeats, seats: Vec<FederationSeat>) -> FederationSeats {
    FederationSeats::from_parts(
        base.federation_id().clone(),
        base.federation_config_hash().clone(),
        base.consensus_threshold(),
        seats,
    )
}

fn statement(
    federation: &FederationSeats,
    seat_index: usize,
    mg_pubkey: Pubkey,
) -> MgPeerAttestationStatement {
    let seat = &federation.seats()[seat_index];

    MgPeerAttestationStatement {
        mg_pubkey,
        federation_id: federation.federation_id().clone(),
        federation_config_hash: federation.federation_config_hash().clone(),
        peer_id: seat.peer_id.clone(),
        guardian_identity: seat.guardian_identity.clone(),
        guardian_fee_account: guardian_fee_account(
            u8::try_from(seat_index + 1).expect("test seat fits account fixture"),
        ),
        issued_at: Timestamp(1_700_000_000),
    }
}

/// Build a structurally valid binding whose signature is a stub.
fn unsigned_binding(
    federation: &FederationSeats,
    seat_index: usize,
    mg_pubkey: &str,
) -> MgPeerAttestation {
    MgPeerAttestation {
        version: ProtocolV1,
        attestation: statement(federation, seat_index, Pubkey(mg_pubkey.to_owned())),
        proof: stub_proof(),
    }
}

/// Build a binding actually signed by `keys`.
fn signed_binding(
    federation: &FederationSeats,
    seat_index: usize,
    keys: &Keys,
) -> MgPeerAttestation {
    let attestation = statement(
        federation,
        seat_index,
        Pubkey(keys.public_key().to_string()),
    );
    let message = Message::from_digest(attestation.digest().expect("statement canonicalizes"));

    MgPeerAttestation {
        version: ProtocolV1,
        attestation,
        proof: SchnorrSignatureProof {
            signature: keys.sign_schnorr(&message),
        },
    }
}

fn resign(binding: &mut MgPeerAttestation, keys: &Keys) {
    let message = Message::from_digest(
        binding
            .attestation
            .digest()
            .expect("mutated statement canonicalizes"),
    );
    binding.proof.signature = keys.sign_schnorr(&message);
}

/// Build a directory covering the first `keys.len()` seats of `federation`.
fn signed_directory(federation: &FederationSeats, keys: &[Keys]) -> MgSeatBindings {
    MgSeatBindings::new(
        keys.iter()
            .enumerate()
            .map(|(seat_index, keys)| signed_binding(federation, seat_index, keys)),
    )
    .expect("signed directory is structurally valid")
}

/// Committed cross-implementation conformance vectors.
const CONFORMANCE_VECTORS: &str = include_str!("../../conformance/fman-seat-bindings-v1.json");

#[derive(serde::Deserialize)]
struct ConformanceVectorFile {
    meta_field_key: String,
    max_seat_bindings: usize,
    max_value_bytes: usize,
    vectors: Vec<ConformanceVector>,
}

#[derive(serde::Deserialize)]
struct ConformanceVector {
    name: String,
    peer_id_order: Vec<String>,
    canonical_value: String,
}

#[test]
fn committed_conformance_vectors_reproduce() {
    // The directory is written by one implementation and read byte for byte by
    // others, so these committed values are the interop contract: the caps, the
    // metadata key, and the exact canonical bytes for a given binding set.
    let file: ConformanceVectorFile =
        serde_json::from_str(CONFORMANCE_VECTORS).expect("conformance vectors parse");

    assert_eq!(file.meta_field_key, MG_SEAT_BINDINGS_META_FIELD_KEY);
    assert_eq!(file.max_seat_bindings, MG_SEAT_BINDINGS_MAX_COUNT);
    assert_eq!(file.max_value_bytes, MG_SEAT_BINDINGS_MAX_VALUE_BYTES);
    assert!(!file.vectors.is_empty());

    for vector in &file.vectors {
        let parsed = MgSeatBindings::parse_canonical(&vector.canonical_value)
            .unwrap_or_else(|err| panic!("vector {} parses: {err}", vector.name));

        assert_eq!(
            parsed.canonical_string().unwrap(),
            vector.canonical_value,
            "{}",
            vector.name
        );
        assert_eq!(
            parsed
                .seat_bindings()
                .iter()
                .map(|binding| binding.attestation.peer_id.0.clone())
                .collect::<Vec<_>>(),
            vector.peer_id_order,
            "{}",
            vector.name
        );
    }

    // Pin the construction path too, not just the bytes' own round trip.
    let federation = federation(1);
    let single = MgSeatBindings::new([unsigned_binding(&federation, 0, "fman-0")])
        .expect("one binding is valid");
    assert_eq!(
        single.canonical_string().unwrap(),
        file.vectors
            .iter()
            .find(|vector| vector.name == "single-seat")
            .expect("single-seat vector")
            .canonical_value
    );
}

#[test]
fn canonical_value_round_trips() {
    let federation = federation(4);
    let bindings = MgSeatBindings::new(
        (0..4).map(|seat| unsigned_binding(&federation, seat, &format!("fman-{seat}"))),
    )
    .expect("four bindings are valid");
    let value = bindings.canonical_string().unwrap();

    assert_eq!(MgSeatBindings::parse_canonical(&value).unwrap(), bindings);
    assert_eq!(bindings.version(), ProtocolV1);
    assert_eq!(bindings.seat_bindings().len(), 4);
}

#[test]
fn new_orders_bindings_by_numeric_peer_id() {
    // Lexicographic ordering would put peer 10 before peer 2. The container has
    // to agree with the config's own `BTreeMap<PeerId, _>` order, which is
    // numeric, or a verifier zipping the two sees a mismatch.
    let federation = federation(4);
    let mut wide = unsigned_binding(&federation, 0, "fman-wide");
    wide.attestation.peer_id = PeerId("10".to_owned());
    let bindings = MgSeatBindings::new([
        wide,
        unsigned_binding(&federation, 2, "fman-2"),
        unsigned_binding(&federation, 0, "fman-0"),
    ])
    .expect("mixed-width peer ids are valid");

    assert_eq!(
        bindings
            .seat_bindings()
            .iter()
            .map(|binding| binding.attestation.peer_id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["0", "2", "10"]
    );
}

#[test]
fn new_rejects_an_empty_directory() {
    assert_eq!(
        MgSeatBindings::new([]).unwrap_err(),
        MgSeatBindingsError::Empty
    );
}

#[test]
fn new_rejects_a_duplicate_peer_id() {
    // Two bindings for one seat are a conflict about who operates it, not a
    // duplicate to collapse — even when they name different MGs.
    let federation = federation(4);

    assert_eq!(
        MgSeatBindings::new([
            unsigned_binding(&federation, 1, "fman-a"),
            unsigned_binding(&federation, 1, "fman-b"),
        ])
        .unwrap_err(),
        MgSeatBindingsError::DuplicatePeerId(PeerId("1".to_owned()))
    );
}

#[test]
fn new_rejects_a_non_canonical_peer_id() {
    let federation = federation(4);
    let mut padded = unsigned_binding(&federation, 1, "fman-a");
    padded.attestation.peer_id = PeerId("01".to_owned());

    assert_eq!(
        MgSeatBindings::new([padded]).unwrap_err(),
        MgSeatBindingsError::NonCanonicalPeerId
    );
}

#[test]
fn non_canonical_peer_id_error_does_not_echo_rejected_text() {
    let federation = federation(4);
    let rejected = "1\n2026-08-18T00:00:00Z WARN forged record";
    let mut binding = unsigned_binding(&federation, 1, "fman-a");
    binding.attestation.peer_id = PeerId(rejected.to_owned());

    let error = MgSeatBindings::new([binding]).unwrap_err();

    assert_eq!(error, MgSeatBindingsError::NonCanonicalPeerId);
    assert_eq!(
        error.to_string(),
        "MG seat binding peer id is not canonical"
    );
    assert!(!error.to_string().contains(rejected));
}

#[test]
fn new_rejects_more_bindings_than_the_cap() {
    let federation = federation(1);
    let bindings = (0..=MG_SEAT_BINDINGS_MAX_COUNT)
        .map(|index| {
            let mut binding = unsigned_binding(&federation, 0, "fman-0");
            binding.attestation.peer_id = PeerId(index.to_string());
            binding
        })
        .collect::<Vec<_>>();

    assert_eq!(bindings.len(), MG_SEAT_BINDINGS_MAX_COUNT + 1);
    assert_eq!(
        MgSeatBindings::new(bindings).unwrap_err(),
        MgSeatBindingsError::TooManySeatBindings
    );
}

#[test]
fn new_rejects_a_value_over_the_size_cap() {
    let federation = federation(1);
    let mut bloated = unsigned_binding(&federation, 0, "fman-0");
    bloated.attestation.guardian_identity =
        GuardianIdentity("g".repeat(MG_SEAT_BINDINGS_MAX_VALUE_BYTES));

    assert_eq!(
        MgSeatBindings::new([bloated]).unwrap_err(),
        MgSeatBindingsError::ValueTooLarge
    );
}

#[test]
fn parse_canonical_rejects_a_reordered_value() {
    let federation = federation(4);
    let canonical = MgSeatBindings::new([
        unsigned_binding(&federation, 0, "fman-0"),
        unsigned_binding(&federation, 1, "fman-1"),
    ])
    .unwrap()
    .canonical_string()
    .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&canonical).unwrap();
    value["seat_bindings"].as_array_mut().unwrap().swap(0, 1);
    let reordered = serde_json::to_string(&value).unwrap();

    assert_ne!(reordered, canonical);
    assert_eq!(
        MgSeatBindings::parse_canonical(&reordered).unwrap_err(),
        MgSeatBindingsError::NonCanonical
    );
}

#[test]
fn parse_canonical_rejects_non_canonical_json() {
    let federation = federation(1);
    let canonical = MgSeatBindings::new([unsigned_binding(&federation, 0, "fman-0")])
        .unwrap()
        .canonical_string()
        .unwrap();

    assert!(MgSeatBindings::parse_canonical(&canonical).is_ok());
    assert_eq!(
        MgSeatBindings::parse_canonical(&format!(" {canonical}")).unwrap_err(),
        MgSeatBindingsError::NonCanonical
    );
    assert_eq!(
        MgSeatBindings::parse_canonical(&canonical.replace(",\"version\":1}", ", \"version\":1}"))
            .unwrap_err(),
        MgSeatBindingsError::NonCanonical
    );
}

#[test]
fn parse_canonical_rejects_malformed_and_unknown_shapes() {
    assert_eq!(
        MgSeatBindings::parse_canonical("not json").unwrap_err(),
        MgSeatBindingsError::MalformedJson
    );
    assert_eq!(
        MgSeatBindings::parse_canonical("{\"seat_bindings\":[],\"version\":2}").unwrap_err(),
        MgSeatBindingsError::MalformedJson
    );
    assert_eq!(
        MgSeatBindings::parse_canonical("{\"extra\":true,\"seat_bindings\":[],\"version\":1}")
            .unwrap_err(),
        MgSeatBindingsError::MalformedJson
    );
    assert_eq!(
        MgSeatBindings::parse_canonical("{\"seat_bindings\":[],\"version\":1}").unwrap_err(),
        MgSeatBindingsError::Empty
    );
}

#[test]
fn parse_canonical_rejects_an_oversized_value_without_parsing_it() {
    let oversized = "x".repeat(MG_SEAT_BINDINGS_MAX_VALUE_BYTES + 1);

    assert_eq!(
        MgSeatBindings::parse_canonical(&oversized).unwrap_err(),
        MgSeatBindingsError::ValueTooLarge
    );
}

#[test]
fn deserialize_rejects_a_reordered_binding_list() {
    let federation = federation(4);
    let bindings = MgSeatBindings::new([
        unsigned_binding(&federation, 0, "fman-0"),
        unsigned_binding(&federation, 1, "fman-1"),
    ])
    .unwrap();
    let mut value = serde_json::to_value(&bindings).unwrap();
    value["seat_bindings"].as_array_mut().unwrap().swap(0, 1);

    assert!(
        serde_json::from_value::<MgSeatBindings>(value)
            .unwrap_err()
            .to_string()
            .contains("not canonical")
    );
}

#[test]
fn verify_for_federation_accepts_a_full_directory() {
    let federation = federation(4);
    let keys = (0..4).map(|_| Keys::generate()).collect::<Vec<_>>();
    let bindings = signed_directory(&federation, &keys);

    let verified = bindings
        .verify_for_federation(&federation)
        .expect("a full, signed directory verifies");

    assert_eq!(
        verified
            .iter()
            .map(|binding| (binding.peer_id.clone(), binding.mg_pubkey.clone()))
            .collect::<Vec<_>>(),
        (0..4)
            .map(|index| (
                PeerId(index.to_string()),
                Pubkey(keys[index].public_key().to_string())
            ))
            .collect::<Vec<_>>()
    );
}

#[test]
fn verify_for_federation_accepts_one_mg_on_several_seats() {
    // One operator running several guardians is legitimate; policy counts it
    // once, but the directory itself must not reject it.
    let federation = federation(4);
    let keys = Keys::generate();
    let bindings = signed_directory(
        &federation,
        &[keys.clone(), keys.clone(), keys.clone(), keys],
    );

    let verified = bindings.verify_for_federation(&federation).unwrap();
    let distinct = verified
        .iter()
        .map(|binding| binding.mg_pubkey.clone())
        .collect::<BTreeSet<_>>();

    assert_eq!(verified.len(), 4);
    assert_eq!(distinct.len(), 1);
}

#[test]
fn verify_for_federation_rejects_an_unverifiable_signature() {
    let federation = federation(1);
    let mut binding = signed_binding(&federation, 0, &Keys::generate());
    binding.attestation.issued_at = Timestamp(1);
    let bindings = MgSeatBindings::new([binding]).unwrap();

    assert_eq!(
        bindings.verify_for_federation(&federation).unwrap_err(),
        MgSeatBindingsError::InvalidSeatBindingSignature
    );
}

#[test]
fn verify_for_federation_rejects_a_non_depositor_fee_account() {
    let federation = federation(1);
    let keys = Keys::generate();
    let mut binding = signed_binding(&federation, 0, &keys);
    binding.attestation.guardian_fee_account = Account::single(
        PublicKey::from_secret_key(
            SECP256K1,
            &SecretKey::from_slice(&[9; 32]).expect("fixed test scalar is valid"),
        ),
        AccountType::Provider,
    );
    resign(&mut binding, &keys);

    assert_eq!(
        MgSeatBindings::new([binding])
            .unwrap()
            .verify_for_federation(&federation)
            .unwrap_err(),
        MgSeatBindingsError::InvalidGuardianFeeAccount(PeerId("0".to_owned()))
    );
}

#[test]
fn verify_for_federation_rejects_a_fee_account_claimed_by_two_seats() {
    let federation = federation(4);
    let keys = [Keys::generate(), Keys::generate()];
    let mut first = signed_binding(&federation, 0, &keys[0]);
    let mut second = signed_binding(&federation, 1, &keys[1]);
    second.attestation.guardian_fee_account = first.attestation.guardian_fee_account.clone();
    resign(&mut first, &keys[0]);
    resign(&mut second, &keys[1]);
    assert_eq!(
        MgSeatBindings::new([first, second])
            .unwrap()
            .verify_for_federation(&with_seats(&federation, federation.seats()[..2].to_vec()))
            .unwrap_err(),
        MgSeatBindingsError::DuplicateGuardianFeeAccount(PeerId("1".to_owned()))
    );
}

#[test]
fn verify_for_federation_rejects_a_foreign_federation_id() {
    let federation = federation(1);
    let elsewhere = FederationSeats::from_parts(
        FederationId("f".repeat(64)),
        federation.federation_config_hash().clone(),
        federation.consensus_threshold(),
        federation.seats().to_vec(),
    );
    let bindings = signed_directory(&elsewhere, &[Keys::generate()]);

    assert_eq!(
        bindings.verify_for_federation(&federation).unwrap_err(),
        MgSeatBindingsError::FederationMismatch(PeerId("0".to_owned()))
    );
}

#[test]
fn verify_for_federation_rejects_a_foreign_config_hash() {
    // Same federation id, different config: an attestation signed against an
    // earlier config revision must not carry over.
    let federation = federation(1);
    let other_config = FederationSeats::from_parts(
        federation.federation_id().clone(),
        HashBytes(vec![9; 32]),
        federation.consensus_threshold(),
        federation.seats().to_vec(),
    );
    let bindings = signed_directory(&other_config, &[Keys::generate()]);

    assert_eq!(
        bindings.verify_for_federation(&federation).unwrap_err(),
        MgSeatBindingsError::ConfigHashMismatch(PeerId("0".to_owned()))
    );
}

#[test]
fn verify_for_federation_rejects_a_binding_for_a_non_peer() {
    let federation = federation(4);
    let bindings = signed_directory(
        &federation,
        &[
            Keys::generate(),
            Keys::generate(),
            Keys::generate(),
            Keys::generate(),
        ],
    );
    let shrunk = with_seats(&federation, federation.seats()[..3].to_vec());

    assert_eq!(
        bindings.verify_for_federation(&shrunk).unwrap_err(),
        MgSeatBindingsError::UnknownPeerId(PeerId("3".to_owned()))
    );
}

#[test]
fn verify_for_federation_rejects_a_wrong_guardian_identity() {
    let federation = federation(4);
    let mut impostor_seats = federation.seats().to_vec();
    impostor_seats[0].guardian_identity = GuardianIdentity("not-the-guardian".to_owned());
    let impostor = with_seats(&federation, impostor_seats);
    let bindings = signed_directory(&impostor, &[Keys::generate()]);

    assert_eq!(
        bindings.verify_for_federation(&federation).unwrap_err(),
        MgSeatBindingsError::GuardianIdentityMismatch(PeerId("0".to_owned()))
    );
}

#[test]
fn verify_for_federation_rejects_an_unbound_seat() {
    let federation = federation(4);
    let bindings = signed_directory(&federation, &[Keys::generate(), Keys::generate()]);

    assert_eq!(
        bindings.verify_for_federation(&federation).unwrap_err(),
        MgSeatBindingsError::UnboundPeerId(PeerId("2".to_owned()))
    );
}
