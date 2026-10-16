use std::collections::BTreeMap;

use fedimint_core::PeerId;
use fedimint_core::config::TypedServerModuleConfig;
use fedimint_core::core::{ModuleInstanceId, ModuleKind};
use fedimint_core::setup_code::{PeerEndpoints, PeerSetupCode};
use fedimint_server::config::ConfigGenParams;

use super::*;

fn fixture() -> ServerConfig {
    let registry = crate::manifold_modules();
    let peer = PeerId::from(0);
    let api = iroh::SecretKey::from_bytes(&[41; 32]);
    let p2p = iroh::SecretKey::from_bytes(&[42; 32]);
    let enabled_modules = registry.iter().map(|(kind, _)| kind.clone()).collect();
    let params = ConfigGenParams {
        identity: peer,
        tls_key: None,
        iroh_api_sk: Some(serde_json::from_value(serde_json::to_value(&api).unwrap()).unwrap()),
        iroh_p2p_sk: Some(serde_json::from_value(serde_json::to_value(&p2p).unwrap()).unwrap()),
        peers: BTreeMap::from([(
            peer,
            PeerSetupCode {
                name: "native-module-test".into(),
                endpoints: PeerEndpoints::Iroh {
                    api_pk: api.public().to_string().parse().unwrap(),
                    p2p_pk: p2p.public().to_string().parse().unwrap(),
                },
                federation_name: Some("native-module-test".into()),
                disable_base_fees: Some(false),
                enabled_modules: Some(enabled_modules),
                federation_size: Some(1),
                network: bitcoin::Network::Regtest,
                fedimint_version: "0.12.0".into(),
            },
        )]),
        meta: BTreeMap::new(),
        disable_base_fees: false,
        enabled_modules: registry.iter().map(|(kind, _)| kind.clone()).collect(),
        network: bitcoin::Network::Regtest,
    };
    ServerConfig::trusted_dealer_gen(&BTreeMap::from([(peer, params)]), &registry, "0.12.0")
        .remove(&peer)
        .unwrap()
}

fn replace<T: TypedServerModuleConfig>(config: &mut ServerConfig, id: ModuleInstanceId, module: T) {
    let module = module.to_erased();
    config.private.modules.insert(id, module.private);
    config.consensus.modules.insert(id, module.consensus);
}

#[test]
fn real_modules_validate_and_every_network_bearing_instance_is_checked() {
    let original = fixture();
    let validate =
        |cfg: &ServerConfig| NativeGuardianConfigValidator.validate(cfg, bitcoin::Network::Regtest);
    validate(&original).unwrap();
    assert!(
        NativeGuardianConfigValidator
            .validate(&original, bitcoin::Network::Bitcoin)
            .is_err()
    );

    for (id, kind) in original.iter_module_instances() {
        let mut cfg = original.clone();
        match kind.as_str() {
            "wallet" => {
                let mut module = cfg
                    .get_module_config_typed::<fedimint_wallet_common::config::WalletConfig>(id)
                    .unwrap();
                module.consensus.network.0 = bitcoin::Network::Bitcoin;
                replace(&mut cfg, id, module);
            }
            "walletv2" => {
                let mut module = cfg
                    .get_module_config_typed::<fedimint_walletv2_common::config::WalletConfig>(id)
                    .unwrap();
                module.consensus.network = bitcoin::Network::Bitcoin;
                replace(&mut cfg, id, module);
            }
            "ln" => {
                let mut module = cfg
                    .get_module_config_typed::<fedimint_ln_common::config::LightningConfig>(id)
                    .unwrap();
                module.consensus.network.0 = bitcoin::Network::Bitcoin;
                replace(&mut cfg, id, module);
            }
            "lnv2" => {
                let mut module = cfg
                    .get_module_config_typed::<fedimint_lnv2_common::config::LightningConfig>(id)
                    .unwrap();
                module.consensus.network = bitcoin::Network::Bitcoin;
                replace(&mut cfg, id, module);
            }
            _ => continue,
        }
        assert!(
            validate(&cfg).is_err(),
            "{kind} mismatch must not hide behind a valid wallet"
        );
    }

    let mut cfg = original.clone();
    cfg.consensus
        .modules
        .retain(|_, m| !matches!(m.kind.as_str(), "wallet" | "walletv2"));
    assert!(validate(&cfg).is_err());
    let mut cfg = original.clone();
    cfg.consensus.modules.values_mut().next().unwrap().kind =
        ModuleKind::from_static_str("unsupported");
    assert!(validate(&cfg).is_err());
    let mut cfg = original.clone();
    cfg.private.modules.clear();
    assert!(validate(&cfg).is_err());
}

#[test]
fn malformed_native_mint_tables_are_rejected_without_panicking() {
    let original = fixture();
    for (id, kind) in original.iter_module_instances() {
        let mut cfg = original.clone();
        match kind.as_str() {
            "mint" => {
                let mut module = cfg
                    .get_module_config_typed::<fedimint_mint_common::config::MintConfig>(id)
                    .unwrap();
                module.consensus.peer_tbs_pks.clear();
                replace(&mut cfg, id, module);
            }
            "mintv2" => {
                let mut module = cfg
                    .get_module_config_typed::<fedimint_mintv2_common::config::MintConfig>(id)
                    .unwrap();
                module.private.tbs_sks.clear();
                replace(&mut cfg, id, module);
            }
            _ => continue,
        }
        assert!(
            NativeGuardianConfigValidator
                .validate(&cfg, bitcoin::Network::Regtest)
                .is_err()
        );
    }
}
