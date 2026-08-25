//! Tests for canonical federation-config derivations.

use std::collections::BTreeMap;

use fedimint_core::config::{ClientConfig, ClientModuleConfig};
use fedimint_core::core::ModuleKind;
use fedimint_core::encoding::{Decodable, DynRawFallback};
use fedimint_core::module::ModuleConsensusVersion;
use fedimint_core::module::registry::ModuleDecoderRegistry;

use super::*;
use crate::test_support::{GUARDIAN_KEYS, test_config};

/// Committed cross-implementation conformance vectors.
const CONFORMANCE_VECTORS: &str = include_str!("../../conformance/federation-config-hash-v1.json");

fn hash_hex(config: &ClientConfig) -> String {
    federation_config_hash(config)
        .0
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn config_hash_is_pinned_for_the_fixture_config() {
    // Changing this value changes the protocol: every attestation an FMan has
    // already signed stops matching what a verifier derives.
    assert_eq!(
        hash_hex(&test_config(4)),
        "f6764956a36c9a0426aee6bedc67aacc033a281c3a4555933f3c0de5b77c424a"
    );
}

#[test]
fn config_hash_ignores_consensus_metadata() {
    // The whole protocol rests on this: guardians sign the hash before the FI
    // publishes `fedi:fman_seat_bindings` into consensus metadata, so a hash
    // that moved when metadata moved would invalidate every attestation.
    let config = test_config(4);
    let mut with_metadata = config.clone();
    with_metadata.global.meta = BTreeMap::from([
        (
            "fedi:fman_seat_bindings".to_owned(),
            "{\"seat_bindings\":[],\"version\":1}".to_owned(),
        ),
        ("federation_name".to_owned(), "Example".to_owned()),
    ]);

    assert_eq!(
        federation_config_hash(&with_metadata),
        federation_config_hash(&config)
    );
}

#[test]
fn config_hash_covers_the_peer_set() {
    let config = test_config(4);
    let mut renamed = config.clone();
    renamed
        .global
        .api_endpoints
        .get_mut(&FedimintPeerId::from(0))
        .expect("fixture peer 0")
        .name = "renamed".to_owned();

    assert_ne!(
        federation_config_hash(&renamed),
        federation_config_hash(&config)
    );
    assert_ne!(
        federation_config_hash(&test_config(3)),
        federation_config_hash(&config)
    );
}

#[test]
fn config_hash_covers_guardian_consensus_keys() {
    // Two federations can share a federation id — which hashes only the API
    // endpoints — while running under different guardian keys. The config hash
    // is what separates them.
    let config = test_config(4);
    let mut swapped = config.clone();
    let keys = swapped
        .global
        .broadcast_public_keys
        .as_mut()
        .expect("fixture broadcast keys");
    let peer_0 = keys[&FedimintPeerId::from(0)];
    let peer_1 = keys[&FedimintPeerId::from(1)];
    keys.insert(FedimintPeerId::from(0), peer_1);
    keys.insert(FedimintPeerId::from(1), peer_0);

    assert_eq!(
        swapped.calculate_federation_id(),
        config.calculate_federation_id()
    );
    assert_ne!(
        federation_config_hash(&swapped),
        federation_config_hash(&config)
    );
}

#[test]
fn config_hash_covers_module_configs() {
    let config = test_config(4);
    let mut altered = config.clone();
    altered.modules.insert(
        0,
        ClientModuleConfig {
            kind: ModuleKind::from_static_str("mint"),
            version: ModuleConsensusVersion::new(2, 0),
            config: DynRawFallback::Raw {
                module_instance_id: 0,
                raw: vec![0xde, 0xad, 0xbe, 0xee],
            },
        },
    );

    assert_ne!(
        federation_config_hash(&altered),
        federation_config_hash(&config)
    );
}

#[test]
fn config_hash_survives_a_consensus_encoding_round_trip() {
    // A verifier reaches the config over the wire, not by cloning a struct.
    let config = test_config(4);
    let encoded = config.consensus_encode_to_vec();
    let decoded = ClientConfig::consensus_decode_whole(&encoded, &ModuleDecoderRegistry::default())
        .expect("fixture config decodes");

    assert_eq!(
        federation_config_hash(&decoded),
        federation_config_hash(&config)
    );
}

#[test]
fn config_hash_is_domain_separated() {
    let config = test_config(4);
    let mut undomained = config.clone();
    undomained.global.meta = BTreeMap::new();
    let bare = sha2::Sha256::digest(undomained.consensus_encode_to_vec()).to_vec();

    assert_ne!(federation_config_hash(&config).0, bare);
    assert_eq!(
        FEDERATION_CONFIG_HASH_DOMAIN_SEPARATOR,
        b"fedi-federation-config-hash/v1\0"
    );
}

#[test]
fn federation_id_is_the_fedimint_lowercase_hex_form() {
    let config = test_config(4);

    assert_eq!(
        federation_id(&config).0,
        config.calculate_federation_id().to_string()
    );
    assert_eq!(federation_id(&config).0.len(), 64);
    assert!(
        federation_id(&config)
            .0
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    );
}

#[test]
fn consensus_threshold_matches_fedimint_num_peers() {
    assert_eq!(consensus_threshold(0), None);
    for total in 1_usize..=64 {
        assert_eq!(
            consensus_threshold(total),
            Some(u32::try_from(fedimint_core::NumPeers::from(total).threshold()).unwrap())
        );
    }
    // Spot-check the shape a reader can verify by hand: 3f + 1 peers tolerate
    // f faults, so the threshold is 2f + 1.
    assert_eq!(consensus_threshold(1), Some(1));
    assert_eq!(consensus_threshold(4), Some(3));
    assert_eq!(consensus_threshold(7), Some(5));
    assert_eq!(consensus_threshold(10), Some(7));
}

#[test]
fn peer_ids_round_trip_only_in_canonical_form() {
    assert_eq!(protocol_peer_id(FedimintPeerId::from(0)).0, "0");
    assert_eq!(protocol_peer_id(FedimintPeerId::from(12)).0, "12");
    assert_eq!(
        parse_protocol_peer_id(&PeerId("12".to_owned())),
        Some(FedimintPeerId::from(12))
    );
    assert_eq!(parse_protocol_peer_id(&PeerId("012".to_owned())), None);
    assert_eq!(parse_protocol_peer_id(&PeerId("+1".to_owned())), None);
    assert_eq!(parse_protocol_peer_id(&PeerId(" 1".to_owned())), None);
    assert_eq!(parse_protocol_peer_id(&PeerId("65536".to_owned())), None);
    assert_eq!(parse_protocol_peer_id(&PeerId(String::new())), None);
}

#[test]
fn federation_seats_derive_guardian_consensus_identities() {
    let config = test_config(4);
    let seats = federation_seats(&config).expect("fixture config derives seats");

    assert_eq!(seats.federation_id(), &federation_id(&config));
    assert_eq!(
        seats.federation_config_hash(),
        &federation_config_hash(&config)
    );
    assert_eq!(seats.consensus_threshold(), 3);
    assert_eq!(
        seats
            .seats()
            .iter()
            .map(|seat| (seat.peer_id.0.as_str(), seat.guardian_identity.0.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("0", GUARDIAN_KEYS[0]),
            ("1", GUARDIAN_KEYS[1]),
            ("2", GUARDIAN_KEYS[2]),
            ("3", GUARDIAN_KEYS[3]),
        ]
    );
    assert_eq!(
        seats
            .seat(&PeerId("2".to_owned()))
            .map(|seat| &seat.peer_id),
        Some(&PeerId("2".to_owned()))
    );
    assert_eq!(seats.seat(&PeerId("9".to_owned())), None);
}

#[test]
fn federation_seats_reject_a_config_without_peers() {
    let mut config = test_config(1);
    config.global.api_endpoints.clear();

    assert_eq!(
        federation_seats(&config).unwrap_err(),
        FederationConfigError::NoPeers
    );
}

#[test]
fn federation_seats_reject_a_config_without_broadcast_keys() {
    let mut config = test_config(4);
    config.global.broadcast_public_keys = None;

    assert_eq!(
        federation_seats(&config).unwrap_err(),
        FederationConfigError::MissingBroadcastPublicKeys
    );
}

#[test]
fn federation_seats_reject_a_peer_without_a_guardian_key() {
    let mut config = test_config(4);
    config
        .global
        .broadcast_public_keys
        .as_mut()
        .expect("fixture broadcast keys")
        .remove(&FedimintPeerId::from(2));

    assert_eq!(
        federation_seats(&config).unwrap_err(),
        FederationConfigError::MissingGuardianIdentity(PeerId("2".to_owned()))
    );
}

#[derive(serde::Deserialize)]
struct ConformanceVectorFile {
    domain_separator_hex: String,
    vectors: Vec<ConformanceVector>,
}

#[derive(serde::Deserialize)]
struct ConformanceVector {
    name: String,
    client_config_hex: String,
    federation_id: String,
    federation_config_hash: String,
    consensus_threshold: u32,
    guardian_identities: BTreeMap<String, String>,
}

#[test]
fn committed_conformance_vectors_reproduce() {
    // These vectors are the cross-implementation contract: anything that can
    // decode a Fedimint client config must reach the same hash from the same
    // bytes. They are decoded with an empty decoder registry on purpose, so
    // they also pin that the derivation needs no module crates.
    let file: ConformanceVectorFile =
        serde_json::from_str(CONFORMANCE_VECTORS).expect("conformance vectors parse");
    let separator = file
        .domain_separator_hex
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("hex is ASCII"), 16)
                .expect("hex byte")
        })
        .collect::<Vec<_>>();
    assert_eq!(separator, FEDERATION_CONFIG_HASH_DOMAIN_SEPARATOR);
    assert!(!file.vectors.is_empty());

    for vector in &file.vectors {
        let config = ClientConfig::consensus_decode_hex(
            &vector.client_config_hex,
            &ModuleDecoderRegistry::default(),
        )
        .unwrap_or_else(|err| panic!("vector {} decodes: {err}", vector.name));
        let seats = federation_seats(&config)
            .unwrap_or_else(|err| panic!("vector {} derives seats: {err}", vector.name));

        assert_eq!(
            hash_hex(&config),
            vector.federation_config_hash,
            "{}",
            vector.name
        );
        assert_eq!(
            seats.federation_id().0,
            vector.federation_id,
            "{}",
            vector.name
        );
        assert_eq!(
            seats.consensus_threshold(),
            vector.consensus_threshold,
            "{}",
            vector.name
        );
        assert_eq!(
            seats
                .seats()
                .iter()
                .map(|seat| (seat.peer_id.0.clone(), seat.guardian_identity.0.clone()))
                .collect::<BTreeMap<_, _>>(),
            vector.guardian_identities,
            "{}",
            vector.name
        );
    }
}
