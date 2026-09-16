use bitcoin::Network;
use clap::Parser as _;
use fedimint_core::Amount;
use fedimint_server_core::ConfigGenModuleArgs;
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
        "/tmp/fman",
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
        "/tmp/fman",
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
        "/tmp/fman",
        "--manifold-environment",
        "staging",
        "--bitcoind-url",
        "http://127.0.0.1:38332",
        "--bitcoind-username",
        "operator",
        "--bitcoind-password",
        "secret",
        "--esplora-url",
        "https://signet.example.test/api",
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
        Some("https://signet.example.test/api")
    );
}

#[test]
fn bitcoind_without_an_available_esplora_remains_supported() {
    let args = Args::try_parse_from([
        "fleet-manager",
        "serve",
        "--data-dir",
        "/tmp/fman",
        "--manifold-environment",
        "development",
        "--bitcoind-url",
        "http://127.0.0.1:18443",
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
        esplora_fallback, ..
    } = process.bitcoin_backend
    else {
        panic!("configured bitcoind must select the bitcoind backend");
    };
    assert_eq!(esplora_fallback, None);
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
