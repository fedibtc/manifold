use std::collections::{BTreeMap, BTreeSet};

use bitcoin::Network;
use clap::Parser as _;
use fedimint_core::Amount;
use fedimint_core::PeerId;
use fedimint_core::core::{ModuleInstanceId, ModuleKind};
use fedimint_core::module::ModuleConsensusVersion;
use fedimint_core::setup_code::{PeerEndpoints, PeerSetupCode};
use fedimint_server::config::{ConfigGenParams, ServerConfig};
use fedimint_server_core::{ConfigGenModuleArgs, ServerModuleInitRegistryExt as _};
use stability_pool_server::common::config::{
    OracleConfig, StabilityPoolClientConfig, StabilityPoolConfig,
};

use super::*;

#[test]
fn bitcoind_password_starting_with_hyphen_is_an_option_value() {
    let args = Args::try_parse_from([
        "fleet-manager",
        "serve",
        "--data-dir",
        "/tmp/mg",
        "--manifold-environment",
        "development",
        "--bitcoind-url",
        "http://127.0.0.1:18443",
        "--bitcoind-username",
        "operator",
        "--bitcoind-password=-leading-hyphen-password",
    ])
    .expect("equals form keeps a leading hyphen inside the password value");
    let Args::Serve(args) = args;
    assert_eq!(
        args.bitcoind_password.as_deref(),
        Some("-leading-hyphen-password")
    );
}

#[test]
fn bitcoind_does_not_implicitly_use_the_environment_esplora() {
    let args = Args::try_parse_from([
        "fleet-manager",
        "serve",
        "--data-dir",
        "/tmp/mg",
        "--manifold-environment",
        "staging",
        "--bitcoind-url",
        "http://127.0.0.1:38332",
        "--bitcoind-username",
        "operator",
        "--bitcoind-password",
        "secret",
    ])
    .unwrap();
    let Args::Serve(args) = args;
    let profile = args.manifold_environment.profile().unwrap();

    let process = seat_process_config(&args, &profile).unwrap();
    let BitcoinBackend::Bitcoind {
        primary,
        esplora_fallback,
    } = process.bitcoin_backend
    else {
        panic!("configured bitcoind must remain the primary backend");
    };
    assert_eq!(primary.url, "http://127.0.0.1:38332");
    assert_eq!(esplora_fallback, None);
}

#[test]
fn explicit_esplora_url_configures_bitcoind_fallback() {
    let args = Args::try_parse_from([
        "fleet-manager",
        "serve",
        "--data-dir",
        "/tmp/mg",
        "--manifold-environment",
        "production",
        "--bitcoind-url",
        "http://127.0.0.1:8332",
        "--bitcoind-username",
        "operator",
        "--bitcoind-password",
        "secret",
        "--esplora-url",
        "https://bitcoin.example.test/api",
    ])
    .unwrap();
    let Args::Serve(args) = args;
    let profile = args.manifold_environment.profile().unwrap();

    let process = seat_process_config(&args, &profile).unwrap();
    let BitcoinBackend::Bitcoind {
        esplora_fallback, ..
    } = process.bitcoin_backend
    else {
        panic!("configured bitcoind must remain the primary backend");
    };
    assert_eq!(
        esplora_fallback.as_ref().map(|url| url.as_str()),
        Some("https://bitcoin.example.test/api")
    );
}

#[test]
fn esplora_fallback_requires_bitcoind() {
    let error = Args::try_parse_from([
        "fleet-manager",
        "serve",
        "--data-dir",
        "/tmp/mg",
        "--manifold-environment",
        "production",
        "--esplora-url",
        "https://bitcoin.example.test/api",
    ])
    .err()
    .expect("fallback without Core must be rejected");
    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}

#[test]
fn manifold_profile_generates_expected_spv2_consensus_config() {
    let modules = manifold_modules();
    let kind = stability_pool_server::common::KIND;
    let init = modules
        .get(&kind)
        .expect("the registry passed to bundled fedimintd contains SPv2");
    let peers = (0_u16..10)
        .map(fedimint_core::PeerId::from)
        .collect::<Vec<_>>();
    let generated = init.trusted_dealer_gen(
        &peers,
        &ConfigGenModuleArgs {
            network: Network::Regtest,
            disable_base_fees: false,
        },
    );

    assert_eq!(generated.len(), peers.len());
    for peer in peers {
        let erased = generated.get(&peer).expect("config for every DKG peer");
        let server = erased
            .to_typed::<StabilityPoolConfig>()
            .expect("generated SPv2 config decodes");
        assert_eq!(server.consensus.consensus_threshold, 7);
        assert!(matches!(
            server.consensus.oracle_config,
            OracleConfig::Aggregate
        ));
        assert_eq!(server.consensus.cycle_duration, Duration::from_secs(600));
        assert_eq!(server.consensus.collateral_ratio.provider, 1);
        assert_eq!(server.consensus.collateral_ratio.seeker, 1);
        assert_eq!(
            server.consensus.min_allowed_seek,
            Amount::from_msats(10_000)
        );
        assert_eq!(
            server.consensus.min_allowed_provide,
            Amount::from_msats(100_000)
        );
        assert_eq!(server.consensus.max_allowed_provide_fee_rate_ppb, 22_062);
        assert_eq!(server.consensus.min_allowed_cancellation_bps, 100);

        let client = init
            .get_client_config(42, &erased.consensus)
            .expect("client consensus config generated from server config");
        let client = client
            .cast::<StabilityPoolClientConfig>()
            .expect("generated client config decodes");
        assert_eq!(client.cycle_duration, Duration::from_secs(600));
        assert_eq!(client.min_allowed_seek, Amount::from_msats(10_000));
        assert_eq!(client.max_allowed_provide_fee_rate_ppb, 22_062);
        assert_eq!(client.min_allowed_cancellation_bps, 100);
    }
}

/// The registry the bundled fedimintd had before it bundled fedi-social, the
/// one every existing federation's guardians were formed and run with.
fn registry_without_fedi_social() -> ServerModuleInitRegistry {
    let mut modules = fedimintd::default_modules();
    modules.attach(manifold_stability_pool_init());
    modules
}

/// The module set MG's formation leader puts in its setup code.
fn manifold_formation_modules() -> BTreeSet<ModuleKind> {
    [
        "lnv2",
        "meta",
        "mintv2",
        "multi_sig_stability_pool",
        "walletv2",
    ]
    .into_iter()
    .map(ModuleKind::clone_from_str)
    .collect()
}

/// One guardian's config from a single-guardian trusted-dealer formation.
fn formed_config(
    registry: &ServerModuleInitRegistry,
    enabled_modules: BTreeSet<ModuleKind>,
) -> ServerConfig {
    let peer = PeerId::from(0);
    let api = iroh::SecretKey::from_bytes(&[51; 32]);
    let p2p = iroh::SecretKey::from_bytes(&[52; 32]);
    let params = ConfigGenParams {
        identity: peer,
        tls_key: None,
        iroh_api_sk: Some(serde_json::from_value(serde_json::to_value(&api).unwrap()).unwrap()),
        iroh_p2p_sk: Some(serde_json::from_value(serde_json::to_value(&p2p).unwrap()).unwrap()),
        peers: BTreeMap::from([(
            peer,
            PeerSetupCode {
                name: "module-set-test".into(),
                endpoints: PeerEndpoints::Iroh {
                    api_pk: api.public().to_string().parse().unwrap(),
                    p2p_pk: p2p.public().to_string().parse().unwrap(),
                },
                federation_name: Some("module-set-test".into()),
                disable_base_fees: Some(false),
                enabled_modules: Some(enabled_modules.clone()),
                federation_size: Some(1),
                network: Network::Regtest,
                fedimint_version: "0.12.0".into(),
            },
        )]),
        meta: BTreeMap::new(),
        disable_base_fees: false,
        enabled_modules,
        network: Network::Regtest,
    };
    ServerConfig::trusted_dealer_gen(&BTreeMap::from([(peer, params)]), registry, "0.12.0")
        .remove(&peer)
        .unwrap()
}

fn module_layout(
    config: &ServerConfig,
) -> Vec<(ModuleInstanceId, ModuleKind, ModuleConsensusVersion)> {
    config
        .consensus
        .modules
        .iter()
        .map(|(id, module)| (*id, module.kind.clone(), module.version))
        .collect()
}

#[test]
fn fedi_social_is_available_but_not_a_default_module() {
    let fedi_social = fedi_social_server::common::KIND;
    let modules = manifold_modules();
    let before = registry_without_fedi_social();

    assert_eq!(
        modules.get(&fedi_social).map(|init| init.module_kind()),
        Some(fedi_social.clone()),
    );
    let mut expected_kinds = before.kinds();
    expected_kinds.insert(fedi_social.clone());
    assert_eq!(modules.kinds(), expected_kinds);

    // fedimintd falls back to this set when no setup code names modules, and
    // its setup UI preselects it.
    assert!(!modules.default_modules().contains(&fedi_social));
    assert_eq!(modules.default_modules(), before.default_modules());
}

#[test]
fn bundling_fedi_social_leaves_existing_federations_unchanged() {
    // A federation formed before fedi-social was bundled, as MG forms them.
    let existing = formed_config(
        &registry_without_fedi_social(),
        manifold_formation_modules(),
    );
    let modules = manifold_modules();

    // fedimintd validates the stored config, builds its decoders and serves
    // the client config only from the modules the config names.
    existing
        .validate_config(&existing.local.identity, &modules)
        .expect("an existing federation's config still validates");
    modules
        .decoders_strict(existing.consensus.iter_module_instances())
        .expect("every existing module still has a decoder");
    assert_eq!(
        existing.consensus.to_client_config(&modules).unwrap(),
        existing
            .consensus
            .to_client_config(&registry_without_fedi_social())
            .unwrap(),
    );

    // A new MG formation commits the same module instances and versions as
    // before, so it agrees with guardians that do not bundle fedi-social.
    let formed_now = formed_config(&modules, manifold_formation_modules());
    assert_eq!(module_layout(&formed_now), module_layout(&existing));
    assert!(
        formed_now
            .consensus
            .modules
            .values()
            .all(|module| module.kind != fedi_social_server::common::KIND)
    );
}

#[test]
fn a_formation_that_names_fedi_social_gets_it() {
    let fedi_social = fedi_social_server::common::KIND;
    let mut enabled = manifold_formation_modules();
    enabled.insert(fedi_social.clone());
    let modules = manifold_modules();
    let config = formed_config(&modules, enabled);

    let (id, _) = config
        .consensus
        .iter_module_instances()
        .find(|(_, kind)| **kind == fedi_social)
        .expect("fedi-social is committed when the leader names it");
    config
        .validate_config(&config.local.identity, &modules)
        .expect("the fedi-social config validates");
    let client = config.consensus.to_client_config(&modules).unwrap();
    assert_eq!(client.modules[&id].kind, fedi_social);
}
