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
        log_poll_seconds: 300,
        log_concurrency: 4,
        log_quota_bytes: MAX_LOG_QUOTA_BYTES,
        log_retention_days: MAX_LOG_RETENTION_DAYS,
        source_budget: 4,
        admission: AdmissionMode::Badged,
        max_targets: None,
        new_targets_per_hour: None,
        source_new_targets_per_hour: None,
        log_target_quota_bytes: None,
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
fn retained_journal_limit_is_higher_only_in_staging() {
    for (environment, expected) in [("development", 32), ("production", 32), ("staging", 1_000)] {
        let mut args = args();
        args.environment = environment.into();
        assert_eq!(args.validate().unwrap().max_journals_per_target, expected);
    }
}

#[test]
fn unbadged_admission_requires_every_limit_and_badged_admission_takes_none() {
    let unbadged = [
        "--admission",
        "unbadged",
        "--max-targets",
        "1024",
        "--new-targets-per-hour",
        "60",
        "--source-new-targets-per-hour",
        "4",
        "--log-target-quota-bytes",
        "8388608",
    ];
    let Admission::Unbadged(limits) = parsed_args(&unbadged).validate().unwrap().admission else {
        panic!("unbadged mode was not selected");
    };
    assert_eq!(limits.max_targets.get(), 1024);
    assert_eq!(limits.new_targets_per_hour.get(), 60);
    assert_eq!(limits.source_new_targets_per_hour.get(), 4);
    assert_eq!(limits.log_target_quota_bytes.get(), 8_388_608);

    // The existing deployment sets no mode and stays badged.
    assert!(matches!(
        parsed_args(&[]).validate().unwrap().admission,
        Admission::Badged
    ));
    // Each limit is required: a forgotten one must not mean "unlimited".
    for missing in [2, 4, 6, 8] {
        let mut partial = unbadged.to_vec();
        partial.drain(missing..missing + 2);
        assert!(parsed_args(&partial).validate().is_err(), "{partial:?}");
    }
    // A limit on a badged collector would silently do nothing.
    assert!(parsed_args(&["--max-targets", "10"]).validate().is_err());

    for (flag, value) in [
        ("--max-targets", "0"),
        ("--max-targets", "4097"),
        ("--new-targets-per-hour", "0"),
        ("--source-new-targets-per-hour", "0"),
        ("--log-target-quota-bytes", "0"),
        // Above the whole archive quota.
        ("--log-target-quota-bytes", "10737418241"),
    ] {
        let mut arguments = unbadged.to_vec();
        let index = arguments
            .iter()
            .position(|argument| *argument == flag)
            .unwrap();
        arguments[index + 1] = value;
        assert!(
            parsed_args(&arguments).validate().is_err(),
            "{flag}={value}"
        );
    }
    let mut at_bounds = unbadged.to_vec();
    at_bounds[3] = "4096";
    at_bounds[9] = "10737418240";
    assert!(parsed_args(&at_bounds).validate().is_ok());

    let command = Args::command();
    for (id, env) in [
        ("admission", "CLOUD_FMAN_TELEMETRY_ADMISSION"),
        ("max_targets", "CLOUD_FMAN_TELEMETRY_MAX_TARGETS"),
        (
            "new_targets_per_hour",
            "CLOUD_FMAN_TELEMETRY_NEW_TARGETS_PER_HOUR",
        ),
        (
            "source_new_targets_per_hour",
            "CLOUD_FMAN_TELEMETRY_SOURCE_NEW_TARGETS_PER_HOUR",
        ),
        (
            "log_target_quota_bytes",
            "CLOUD_FMAN_TELEMETRY_LOG_TARGET_QUOTA_BYTES",
        ),
    ] {
        let argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == id)
            .unwrap();
        assert_eq!(argument.get_env(), Some(OsStr::new(env)));
    }
}
