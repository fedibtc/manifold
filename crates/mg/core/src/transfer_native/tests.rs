use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

use fedimint_server::config::{
    PeerIrohEndpoints, ServerConfigConsensus, ServerConfigLocal, ServerConfigPrivate,
};

use super::*;
use crate::backup::GuardianArchive;
use crate::facts::SeatPorts;
use crate::seat_process::BitcoinBackend;
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
    material_with_auth(config, "original-guardian-auth".to_owned())
}

pub(crate) fn material_with_auth(config: &ServerConfig, api_auth: String) -> TransferMaterial {
    let salt = fedimint_aead::random_salt();
    let key = fedimint_aead::get_encryption_key(&api_auth, &salt).unwrap();
    TransferMaterial {
        archive: GuardianArchive {
            private_encrypt: hex::encode(
                fedimint_aead::encrypt(serde_json::to_vec(&config.private).unwrap(), &key).unwrap(),
            ),
            private_salt: salt,
            local_json: serde_json::to_string(&config.local).unwrap(),
            consensus_json: serde_json::to_string(&config.consensus).unwrap(),
        },
        api_auth,
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
async fn encrypted_native_material_preserves_identity_and_checks_installed_bytes() {
    let root = tempfile::tempdir().unwrap();
    let db = crate::db::Db::open(root.path()).await.unwrap();
    let process = process(root.path());
    let config = native_config();
    let material = material(&config);
    let encoded = material.encode().unwrap();
    let validated = validate_material(
        &db,
        &process,
        SeatNo(0),
        TransferMaterial::decode(&encoded).unwrap(),
        FederationId(config.calculate_federation_id().to_string()),
        Arc::new(CoreConfigValidator),
    )
    .await
    .unwrap();
    assert_eq!(validated.keys.api_auth, material.api_auth);
    assert!(validated.keys.preserve_guardian_identity);
    let ports = SeatPorts::from_base(crate::facts::PortBase::new(31000).unwrap());
    assert_eq!(
        crate::seat_process::effective_iroh_api_key(&validated.keys, ports)
            .public()
            .to_string(),
        config.private.iroh_api_sk.unwrap().public().to_string(),
    );
    assert_eq!(
        crate::seat_process::effective_iroh_p2p_key(&validated.keys, ports),
        config.private.iroh_p2p_sk.unwrap().to_bytes(),
    );
    let work = transfer_work_dir(&process, SeatNo(0));
    assert_eq!(
        std::fs::metadata(&work).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(std::fs::read_dir(&work).unwrap().count(), 0);

    assert!(
        check_installed_material(&process, SeatNo(0), &encoded)
            .await
            .is_err()
    );
    let data = crate::seat_process::seat_data_dir(&process, SeatNo(0));
    crate::backup::write_restored_seat_dir(
        &data,
        &material.archive,
        &material.archive.digest(),
        &material.api_auth,
    )
    .await
    .unwrap();
    check_installed_material(&process, SeatNo(0), &encoded)
        .await
        .unwrap();
    tokio::fs::write(data.join("private.json"), "{}")
        .await
        .unwrap();
    assert!(
        check_installed_material(&process, SeatNo(0), &encoded)
            .await
            .is_err()
    );
    tokio::fs::remove_file(data.join("private.json"))
        .await
        .unwrap();
    tokio::fs::write(data.join("password.private"), "destination-derived-auth")
        .await
        .unwrap();
    assert!(
        check_installed_material(&process, SeatNo(0), &encoded)
            .await
            .is_err()
    );
    tokio::fs::write(data.join("password.private"), &material.api_auth)
        .await
        .unwrap();
    tokio::fs::write(data.join("consensus.json"), "{}")
        .await
        .unwrap();
    assert!(
        check_installed_material(&process, SeatNo(0), &encoded)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn native_identity_decryption_and_policy_errors_are_sanitized() {
    let root = tempfile::tempdir().unwrap();
    let db = crate::db::Db::open(root.path()).await.unwrap();
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
            package.api_auth = "wrong-original-secret".into();
        }
        if case == 7 {
            package.archive.local_json = "secret-invalid-json".into();
        }
        if case == 8 {
            package.api_auth.push('\n');
        }
        if case == 9 {
            // The encryption password is valid for Argon2, but cannot be
            // represented in the child's environment.
            package = material_with_auth(&config, "original\0auth".to_owned());
        }
        let error = validate_material(
            &db,
            &process,
            SeatNo(0),
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
        assert_eq!(
            std::fs::read_dir(transfer_work_dir(&process, SeatNo(0)))
                .unwrap()
                .count(),
            0
        );
    }
    assert!(
        validate_material(
            &db,
            &process,
            SeatNo(0),
            material(&original),
            FederationId("wrong federation".into()),
            Arc::new(CoreConfigValidator),
        )
        .await
        .is_err()
    );
    assert!(
        validate_material(
            &db,
            &process,
            SeatNo(0),
            material(&original),
            federation,
            Arc::new(RejectGuardianConfig),
        )
        .await
        .is_err()
    );
    clear_transfer_work(&process, SeatNo(0)).await.unwrap();
    assert!(!transfer_work_dir(&process, SeatNo(0)).exists());
}
