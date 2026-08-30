//! Validated collector configuration tests.

use std::{ffi::OsStr, net::SocketAddr, path::PathBuf};

use clap::{CommandFactory as _, Parser as _};

use super::*;

fn args() -> Args {
    Args {
        public_bind: "127.0.0.1:10000".parse::<SocketAddr>().unwrap(),
        private_bind: "127.0.0.1:10001".parse::<SocketAddr>().unwrap(),
        private_bind_isolated: false,
        public_base_url: "https://collector.example".into(),
        data_dir: PathBuf::from("/tmp/data"),
        key_file: PathBuf::from("/tmp/key"),
        key_id: "test".into(),
        environment: "development".into(),
        lease_seconds: 3600,
        metrics_poll_seconds: 1800,
        metrics_concurrency: 4,
        metrics_source_version_requirement: "*".into(),
        metrics_source_version_hash: "release-hash".into(),
        canonical_method_labels: false,
        log_poll_seconds: 300,
        log_concurrency: 4,
        log_quota_bytes: MAX_LOG_QUOTA_BYTES,
        log_retention_days: MAX_LOG_RETENTION_DAYS,
        source_budget: 4,
        trusted_proxies: Vec::new(),
        #[cfg(feature = "defe-test-support")]
        e2e_iroh_endpoint_addr: None,
        #[cfg(feature = "defe-test-support")]
        e2e_poll_millis: None,
        #[cfg(feature = "defe-test-support")]
        e2e_issuer: None,
        #[cfg(feature = "defe-test-support")]
        e2e_nostr_relay: None,
    }
}

fn parsed_args(extra: &[&str]) -> Args {
    let mut arguments = vec![
        "cloud-fman-telemetry",
        "--public-base-url",
        "https://collector.example",
        "--data-dir",
        "/tmp/data",
        "--key-file",
        "/tmp/key",
        "--key-id",
        "test",
        "--environment",
        "development",
        "--metrics-source-version-requirement",
        "*",
        "--metrics-source-version-hash",
        "release-hash",
    ];
    arguments.extend_from_slice(extra);
    Args::try_parse_from(arguments).unwrap()
}

#[test]
fn non_loopback_private_listener_requires_isolation_acknowledgement() {
    assert!(!parsed_args(&[]).private_bind_isolated);

    for address in ["0.0.0.0:10001", "[::]:10001"] {
        let broad = parsed_args(&["--private-bind", address]);
        assert_eq!(
            broad.validate().err().as_deref(),
            Some("a non-loopback private listener requires --private-bind-isolated")
        );
    }

    let acknowledged = parsed_args(&["--private-bind", "0.0.0.0:10001", "--private-bind-isolated"]);
    assert!(acknowledged.private_bind_isolated);
    assert!(acknowledged.validate().is_ok());

    let command = Args::command();
    let isolation = command
        .get_arguments()
        .find(|argument| argument.get_id() == "private_bind_isolated")
        .unwrap();
    assert_eq!(
        isolation.get_env(),
        Some(OsStr::new("CLOUD_FMAN_TELEMETRY_PRIVATE_BIND_ISOLATED"))
    );
}

#[test]
fn archive_defaults_are_hard_upper_bounds() {
    assert!(args().validate().is_ok());
    let mut excessive_quota = args();
    excessive_quota.log_quota_bytes = MAX_LOG_QUOTA_BYTES + 1;
    assert!(excessive_quota.validate().is_err());
    let mut excessive_retention = args();
    excessive_retention.log_retention_days = MAX_LOG_RETENTION_DAYS + 1;
    assert!(excessive_retention.validate().is_err());
}

#[test]
fn sparse_metrics_cadence_is_only_fifteen_or_thirty_minutes() {
    let mut thirty_minutes = args();
    thirty_minutes.metrics_poll_seconds = 1800;
    assert!(thirty_minutes.validate().is_ok());
    let mut fifteen_minutes = args();
    fifteen_minutes.metrics_poll_seconds = 900;
    assert!(fifteen_minutes.validate().is_ok());
    let mut rapid_retry = args();
    rapid_retry.metrics_poll_seconds = 60;
    assert!(rapid_retry.validate().is_err());
}

#[test]
fn metrics_source_requirement_must_be_bounded_semver() {
    let mut invalid = args();
    invalid.metrics_source_version_requirement = "not a requirement".into();
    assert_eq!(
        invalid.validate().err().as_deref(),
        Some("metrics source version requirement is invalid")
    );

    let mut oversized = args();
    oversized.metrics_source_version_requirement = "1".repeat(129);
    assert_eq!(
        oversized.validate().err().as_deref(),
        Some("metrics source version requirement must contain 1..=128 bytes")
    );
}

#[test]
fn production_rejects_metrics_source_placeholders() {
    for placeholder in ["version", "hash"] {
        let mut args = args();
        args.environment = "production".into();
        match placeholder {
            "version" => args.metrics_source_version_requirement = "REPLACE_ME".into(),
            "hash" => args.metrics_source_version_hash = "REPLACE_ME".into(),
            _ => unreachable!(),
        }
        assert!(
            args.validate().is_err(),
            "{placeholder} placeholder was accepted"
        );
    }
}
