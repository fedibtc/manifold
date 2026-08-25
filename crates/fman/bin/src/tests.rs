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
