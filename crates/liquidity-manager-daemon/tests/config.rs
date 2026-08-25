use super::*;

fn parse(args: &[&str]) -> Result<CliCommand, clap::Error> {
    parse_cli_from(args.iter().copied())
}

#[test]
fn cli_derives_sqlite_path_from_data_dir() {
    let CliCommand::RunDaemon(args) = parse(&[
        "liquidity-manager-daemon",
        "run",
        "daemon",
        "--manifold-environment",
        "development",
        "--data-dir",
        "/tmp/flip-test",
    ])
    .expect("parse run daemon");

    assert_eq!(args.data_dir, PathBuf::from("/tmp/flip-test"));
    assert_eq!(
        args.sqlite_path,
        PathBuf::from("/tmp/flip-test/flip.sqlite")
    );
}

#[test]
fn cli_sqlite_path_flag_overrides_derived_default() {
    let CliCommand::RunDaemon(args) = parse(&[
        "liquidity-manager-daemon",
        "run",
        "daemon",
        "--manifold-environment=development",
        "--data-dir=/tmp/flip-test",
        "--sqlite-path=/tmp/elsewhere/db.sqlite",
    ])
    .expect("parse run daemon");

    assert_eq!(args.sqlite_path, PathBuf::from("/tmp/elsewhere/db.sqlite"));
}

#[test]
fn cli_parses_daemon_mode_flags() {
    let CliCommand::RunDaemon(args) = parse(&[
        "liquidity-manager-daemon",
        "run",
        "daemon",
        "--manifold-environment",
        "development",
        "--restore-mode",
    ])
    .expect("parse run daemon");

    assert_eq!(args.mode, DaemonMode::Restore);
}

#[test]
fn cli_trust_fixtures_defaults_off_and_parses_directory() {
    let CliCommand::RunDaemon(args) = parse(&[
        "liquidity-manager-daemon",
        "run",
        "daemon",
        "--manifold-environment",
        "development",
    ])
    .expect("parse run daemon");
    assert_eq!(args.trust_fixtures_dir, None);

    let CliCommand::RunDaemon(args) = parse(&[
        "liquidity-manager-daemon",
        "run",
        "daemon",
        "--manifold-environment",
        "development",
        "--trust-fixtures",
        "/tmp/flip-fixtures",
    ])
    .expect("parse run daemon");
    assert_eq!(
        args.trust_fixtures_dir,
        Some(PathBuf::from("/tmp/flip-fixtures"))
    );
}

#[test]
fn cli_rejects_unknown_daemon_flag() {
    parse(&["liquidity-manager-daemon", "run", "daemon", "--bogus"])
        .expect_err("unknown flag must fail");
}

#[test]
fn cli_rejects_missing_subcommand() {
    parse(&["liquidity-manager-daemon"]).expect_err("missing command must fail");
    parse(&["liquidity-manager-daemon", "run"]).expect_err("missing subcommand must fail");
    parse(&["liquidity-manager-daemon", "serve"]).expect_err("unknown command must fail");
}

#[test]
fn daemon_args_debug_redacts_boot_secrets() {
    let args = DaemonArgs {
        manifold_environment: ManifoldEnvironment::Development,
        data_dir: PathBuf::from("/tmp/flip"),
        sqlite_path: PathBuf::from("/tmp/flip/flip.sqlite"),
        admin_bind_address: "127.0.0.1:8173".parse().expect("valid admin bind"),
        public_bind_address: "127.0.0.1:8174".parse().expect("valid public bind"),
        bootstrap_admin_token: Some("admin-secret-token".to_owned()),
        secret_store_key: Some("0123456789abcdef".repeat(4)),
        allow_bootstrap_token_fallback: false,
        mode: DaemonMode::Normal,
        provider_nostr_secret_key: Some("provider-secret".to_owned()),
        trust_fixtures_dir: None,
        max_open_target_clients: crate::target_fedimint::DEFAULT_MAX_OPEN_TARGET_CLIENTS,
        allow_private_federation_endpoints: false,
    };

    let debug = format!("{args:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("admin-secret-token"));
    assert!(!debug.contains("0123456789abcdef"));
    assert!(!debug.contains("provider-secret"));
}
