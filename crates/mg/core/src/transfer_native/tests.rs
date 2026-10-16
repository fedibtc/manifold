use std::collections::BTreeMap;
use std::path::Path;

use fedimint_server::config::{
    PeerIrohEndpoints, ServerConfigConsensus, ServerConfigLocal, ServerConfigPrivate,
};

use super::*;
use crate::backup::GuardianArchive;
use crate::facts::SeatNo;
use crate::facts::SeatPorts;
use crate::seat_process::BitcoinBackend;
use crate::transfer_state::ImportedKeys;
use fedimint_core::PeerId;

/// This module-empty fixture exercises native core validation, not the binary's
/// module/network admission policy (which has its own real-module tests).
pub(crate) struct CoreConfigValidator;

impl GuardianConfigValidator for CoreConfigValidator {
    fn validate(&self, config: &ServerConfig, _: bitcoin::Network) -> anyhow::Result<()> {
        config.validate_config(&config.local.identity, &Default::default())
    }
}

pub(crate) fn native_config() -> ServerConfig {
    let peer = PeerId::from(0);
    let api = iroh::SecretKey::from_bytes(&[31; 32]);
    let p2p = iroh::SecretKey::from_bytes(&[32; 32]);
    let broadcast = fedimint_core::secp256k1::SecretKey::from_slice(&[33; 32]).unwrap();
    ServerConfig {
        consensus: ServerConfigConsensus {
            code_version: "0.12.0".into(),
            version: fedimint_core::module::CORE_CONSENSUS_VERSION,
            broadcast_public_keys: BTreeMap::from([(
                peer,
                broadcast.public_key(&fedimint_core::secp256k1::Secp256k1::new()),
            )]),
            broadcast_rounds_per_session: 200,
            api_endpoints: BTreeMap::new(),
            iroh_endpoints: BTreeMap::from([(
                peer,
                PeerIrohEndpoints {
                    name: "transfer-native-test".into(),
                    api_pk: api.public().to_string().parse().unwrap(),
                    p2p_pk: p2p.public().to_string().parse().unwrap(),
                },
            )]),
            tls_certs: BTreeMap::new(),
            modules: BTreeMap::new(),
            meta: BTreeMap::new(),
        },
        local: ServerConfigLocal {
            p2p_endpoints: BTreeMap::new(),
            identity: peer,
            max_connections: 100,
            broadcast_round_delay_ms: 50,
        },
        private: ServerConfigPrivate {
            tls_key: None,
            iroh_api_sk: Some(serde_json::from_value(serde_json::to_value(api).unwrap()).unwrap()),
            iroh_p2p_sk: Some(serde_json::from_value(serde_json::to_value(p2p).unwrap()).unwrap()),
            broadcast_secret_key: broadcast,
            modules: BTreeMap::new(),
        },
    }
}

pub(crate) fn material(config: &ServerConfig) -> TransferMaterial {
    TransferMaterial {
        archive: GuardianArchive {
            private_json: serde_json::to_string(&config.private).unwrap(),
            local_json: serde_json::to_string(&config.local).unwrap(),
            consensus_json: serde_json::to_string(&config.consensus).unwrap(),
        },
        // A config without a key still needs a wire payload to be refused.
        keys: ImportedKeys {
            iroh_api: iroh::SecretKey::from_bytes(
                &config
                    .private
                    .iroh_api_sk
                    .as_ref()
                    .map_or([0; 32], |key| key.to_bytes()),
            ),
            iroh_p2p: iroh::SecretKey::from_bytes(
                &config
                    .private
                    .iroh_p2p_sk
                    .as_ref()
                    .map_or([0; 32], |key| key.to_bytes()),
            ),
        },
    }
}

fn process(root: &Path) -> SeatProcessConfig {
    SeatProcessConfig {
        data_root: root.to_owned(),
        fedimintd: "/unused".into(),
        bitcoin_network: bitcoin::Network::Regtest,
        bitcoin_backend: BitcoinBackend::Esplora("http://127.0.0.1:1".parse().unwrap()),
        iroh_dns: "https://dns.iroh.link/pkarr".parse().unwrap(),
    }
}

#[tokio::test]
async fn native_material_preserves_identity_and_checks_installed_bytes() {
    let root = tempfile::tempdir().unwrap();
    let process = process(root.path());
    let config = native_config();
    let material = material(&config);
    let validated = validate_material(
        &process,
        material.clone(),
        FederationId(config.calculate_federation_id().to_string()),
        Arc::new(CoreConfigValidator),
    )
    .await
    .unwrap();
    let keys = validated.material.keys.seat_keys("derived-auth".into());
    assert_eq!(keys.api_auth, "derived-auth");
    assert!(keys.preserve_guardian_identity);
    let ports = SeatPorts::from_base(crate::facts::PortBase::new(31000).unwrap());
    assert_eq!(
        crate::seat_process::effective_iroh_api_key(&keys, ports)
            .public()
            .to_string(),
        config.private.iroh_api_sk.unwrap().public().to_string(),
    );
    assert_eq!(
        crate::seat_process::effective_iroh_p2p_key(&keys, ports),
        config.private.iroh_p2p_sk.unwrap().to_bytes(),
    );
    let data = crate::seat_process::seat_data_dir(&process, SeatNo(0));
    crate::backup::write_restored_seat_dir(&data, &material.archive, &material.archive.digest())
        .await
        .unwrap();
    assert!(
        TransferMaterial::read(&data, material.keys.clone())
            .await
            .unwrap()
            .keys
            .same_as(&material.keys)
    );
}

#[tokio::test]
async fn native_identity_and_policy_errors_are_sanitized() {
    let root = tempfile::tempdir().unwrap();
    let process = process(root.path());
    let original = native_config();
    let federation = FederationId(original.calculate_federation_id().to_string());
    for case in 0..10 {
        let mut config = original.clone();
        match case {
            0 => config.local.identity = PeerId::from(1),
            1 => config.private.iroh_api_sk = config.private.iroh_p2p_sk.clone(),
            2 => config.private.iroh_p2p_sk = None,
            3 => config.consensus.iroh_endpoints.clear(),
            4 => config.consensus.broadcast_public_keys.clear(),
            5 => {
                config.private.broadcast_secret_key =
                    fedimint_core::secp256k1::SecretKey::from_slice(&[35; 32]).unwrap()
            }
            _ => {}
        }
        let mut package = material(&config);
        if case == 6 {
            // The handed-over secret must be the one the config names.
            package.keys.iroh_api = iroh::SecretKey::from_bytes(&[36; 32]);
        }
        if case == 7 {
            package.archive.local_json = "secret-invalid-json".into();
        }
        if case == 8 {
            package.keys.iroh_p2p = package.keys.iroh_api.clone();
        }
        if case == 9 {
            package.archive.private_json = "secret-invalid-json".into();
        }
        let error = validate_material(
            &process,
            package,
            federation.clone(),
            Arc::new(CoreConfigValidator),
        )
        .await
        .err()
        .expect("reject malformed native config");
        assert_eq!(
            format!("{error:?}: {error}"),
            "InvalidGuardian: invalid guardian transfer material"
        );
    }
    assert!(
        validate_material(
            &process,
            material(&original),
            FederationId("wrong federation".into()),
            Arc::new(CoreConfigValidator),
        )
        .await
        .is_err()
    );
    assert!(
        validate_material(
            &process,
            material(&original),
            federation,
            Arc::new(RejectGuardianConfig),
        )
        .await
        .is_err()
    );
}
