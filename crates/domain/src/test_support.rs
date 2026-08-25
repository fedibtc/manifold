//! Test-support fixtures for the canonical federation-config derivations.
//!
//! The config hash and peer-set derivation are a cross-component agreement:
//! the FMan signs a hash the FI and FLIP must reproduce exactly. Tests in
//! other crates build against this one fixture rather than their own copy, so
//! a change to the canonical shape cannot pass in one crate while failing in
//! another.

use std::collections::BTreeMap;

use fedimint_core::PeerId as FedimintPeerId;
use fedimint_core::config::{ClientConfig, ClientModuleConfig, GlobalClientConfig, PeerUrl};
use fedimint_core::core::ModuleKind;
use fedimint_core::encoding::DynRawFallback;
use fedimint_core::module::{CoreConsensusVersion, ModuleConsensusVersion};
use fedimint_core::secp256k1::PublicKey;
use fedimint_core::util::SafeUrl;

/// Deterministic guardian consensus keys: successive multiples of the
/// secp256k1 generator, in compressed lowercase hex.
///
/// The first four are load-bearing — `test_config(4)` feeds the pinned
/// config-hash vector and the committed conformance file — so extend this
/// array only at the end. The rest exist because `fi-client` forms
/// `MIN_FEDERATION_SIZE` (7) seat federations.
pub const GUARDIAN_KEYS: [&str; 7] = [
    "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
    "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
    "02f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
    "02e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd13",
    "022f8bde4d1a07209355b4a7250a5c5128e88b84bddc619ab7cba8d569b240efe4",
    "03fff97bd5755eeea420453a14355235d382f6472f8568a18b2f057a1460297556",
    "025cbdf0646e5db4eaa398f365f2ea7a0e3d419b7e0330e39ce92bddedcac4f9bc",
];

/// Build a deterministic final config with `peers` guardian seats.
///
/// # Panics
///
/// Panics if `peers` exceeds the number of fixture keys.
#[must_use]
pub fn test_config(peers: usize) -> ClientConfig {
    assert!(
        peers <= GUARDIAN_KEYS.len(),
        "only {} fixture keys exist",
        GUARDIAN_KEYS.len()
    );

    let api_endpoints = (0..peers)
        .map(|index| {
            let peer = FedimintPeerId::from(u16::try_from(index).expect("small peer index"));
            let url = PeerUrl {
                url: SafeUrl::parse(&format!("wss://guardian-{index}.example:5000"))
                    .expect("fixture guardian URL parses"),
                name: format!("guardian-{index}"),
            };

            (peer, url)
        })
        .collect::<BTreeMap<_, _>>();
    let broadcast_public_keys = (0..peers)
        .map(|index| {
            let peer = FedimintPeerId::from(u16::try_from(index).expect("small peer index"));
            let key = GUARDIAN_KEYS[index]
                .parse::<PublicKey>()
                .expect("fixture guardian key parses");

            (peer, key)
        })
        .collect::<BTreeMap<_, _>>();

    ClientConfig {
        global: GlobalClientConfig {
            api_endpoints,
            broadcast_public_keys: Some(broadcast_public_keys),
            consensus_version: CoreConsensusVersion::new(2, 1),
            meta: BTreeMap::new(),
        },
        modules: BTreeMap::from([(
            0,
            ClientModuleConfig {
                kind: ModuleKind::from_static_str("mint"),
                version: ModuleConsensusVersion::new(2, 0),
                config: DynRawFallback::Raw {
                    module_instance_id: 0,
                    raw: vec![0xde, 0xad, 0xbe, 0xef],
                },
            },
        )]),
    }
}
