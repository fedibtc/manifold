#[path = "../src/metrics_observability.rs"]
mod metrics_observability;
#[path = "../src/metrics_policy.rs"]
#[allow(dead_code)]
mod metrics_policy;
#[path = "../src/metrics_snapshot.rs"]
mod metrics_snapshot;

use metrics_snapshot::{ExpositionError, MetricsSnapshot, MetricsTargetHealth};

fn render_metrics(
    snapshots: Vec<MetricsSnapshot>,
    targets: Vec<MetricsTargetHealth>,
    rejected_snapshots: usize,
    now_ms: i64,
    stale_after_ms: i64,
    method_source_ready: bool,
) -> Result<String, ExpositionError> {
    metrics_snapshot::render_metrics(
        snapshots,
        targets,
        rejected_snapshots,
        now_ms,
        stale_after_ms,
        method_source_ready,
        &metrics_observability::MetricsObservability::default(),
    )
}

fn snapshot(mg: &str, seat: &str, observed_at_ms: i64) -> MetricsSnapshot {
    MetricsSnapshot {
        mg_id: mg.to_owned(),
        mg_name: "same-display-name".to_owned(),
        guardian_seat_id: seat.to_owned(),
        federation_id: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_owned(),
        observed_at_ms,
        samples: vec![format!(
            "fm_consensus_session_count{{fman_id=\"{mg}\",fman_name=\"same-display-name\",guardian_seat_id=\"{seat}\"}} 7"
        )],
    }
}

#[test]
fn preserves_sparse_observation_timestamp_across_repeated_scrapes() {
    let first = render_metrics(
        vec![snapshot("11", "aa", 1_000)],
        vec![],
        0,
        5_000,
        30_000,
        false,
    )
    .unwrap();
    let later = render_metrics(
        vec![snapshot("11", "aa", 1_000)],
        vec![],
        0,
        20_000,
        30_000,
        false,
    )
    .unwrap();
    let sample = "fm_consensus_session_count{fman_id=\"11\",fman_name=\"same-display-name\",guardian_seat_id=\"aa\"} 7 1000";
    assert!(first.contains(sample));
    assert!(later.contains(sample));
    assert!(!later.contains(" 7 20000"));
}

#[test]
fn colliding_names_and_seats_cannot_merge_mg_identities() {
    let output = render_metrics(
        vec![snapshot("11", "aa", 1_000), snapshot("22", "aa", 2_000)],
        vec![],
        0,
        3_000,
        30_000,
        false,
    )
    .unwrap();
    assert!(output.contains("fman_id=\"11\""));
    assert!(output.contains("fman_id=\"22\""));
    assert_eq!(output.matches("fm_consensus_session_count{").count(), 2);
}

#[test]
fn staleness_is_metadata_and_does_not_retime_source_samples() {
    let output = render_metrics(
        vec![snapshot("11", "aa", 1_000)],
        vec![],
        0,
        62_000,
        60_000,
        false,
    )
    .unwrap();
    assert!(output.contains(
        "cloud_fman_telemetry_snapshot_stale{federation_id=\"0000000000000000000000000000000000000000000000000000000000000000\",fman_id=\"11\""
    ));
    assert!(output.contains("guardian_seat_id=\"aa\"} 1 62000\n"));
    assert!(output.contains("} 7 1000\n"));
}

#[test]
fn observation_at_exact_freshness_boundary_is_not_stale() {
    let output = render_metrics(
        vec![snapshot("11", "aa", 1_000)],
        vec![],
        0,
        61_000,
        60_000,
        false,
    )
    .unwrap();
    assert!(output.contains(
        "cloud_fman_telemetry_snapshot_stale{federation_id=\"0000000000000000000000000000000000000000000000000000000000000000\",fman_id=\"11\",fman_name=\"same-display-name\",guardian_seat_id=\"aa\"} 0 61000\n"
    ));
}

#[test]
fn impossible_or_corrupted_snapshot_state_fails_closed() {
    assert!(
        render_metrics(
            vec![snapshot("11", "aa", 2_000)],
            vec![],
            0,
            1_000,
            60_000,
            false
        )
        .is_err()
    );
    let mut corrupt = snapshot("11", "aa", 1_000);
    corrupt.samples[0].push('\n');
    assert!(render_metrics(vec![corrupt], vec![], 0, 2_000, 60_000, false).is_err());

    let mut canonical_method_family = snapshot("11", "aa", 1_000);
    canonical_method_family.samples = vec![
        "fm_jsonrpc_api_request_response_code_total{code=\"0\",federation_id=\"0000000000000000000000000000000000000000000000000000000000000000\",fman_id=\"11\",fman_name=\"same-display-name\",guardian_seat_id=\"aa\",method=\"unknown\",type=\"default\"} 1".into(),
    ];
    let output = render_metrics(
        vec![canonical_method_family],
        vec![],
        0,
        2_000,
        60_000,
        true,
    )
    .unwrap();
    assert!(output.contains("# TYPE fm_jsonrpc_api_request_response_code_total counter\n"));
}

#[test]
fn exposition_is_accepted_by_a_prometheus_text_parser() {
    use std::io::BufRead as _;

    let output = render_metrics(
        vec![snapshot("11", "aa", 1_000)],
        vec![],
        0,
        2_000,
        60_000,
        false,
    )
    .unwrap();
    let scrape = prometheus_parse::Scrape::parse(std::io::Cursor::new(output).lines()).unwrap();
    assert!(!scrape.samples.is_empty());
}

#[test]
fn remote_failure_is_exposed_without_removing_healthy_targets() {
    let output = render_metrics(
        vec![],
        vec![
            MetricsTargetHealth {
                mg_id: "11".into(),
                mg_name: "same".into(),
                fresh: true,
                seats: vec![],
            },
            MetricsTargetHealth {
                mg_id: "22".into(),
                mg_name: "same".into(),
                fresh: false,
                seats: vec![],
            },
        ],
        0,
        2_000,
        60_000,
        false,
    )
    .unwrap();
    assert!(output.contains("fman_id=\"11\",fman_name=\"same\"} 1 2000"));
    assert!(output.contains("fman_id=\"22\",fman_name=\"same\"} 0 2000"));
}

#[test]
fn exposes_bounded_persisted_snapshot_rejections() {
    let output = render_metrics(vec![], vec![], 3, 2_000, 60_000, false).unwrap();
    assert!(output.contains("cloud_fman_telemetry_snapshot_rejected_rows 3 2000\n"));
}

#[test]
fn exposes_fixed_admission_outcomes_without_source_dimensions() {
    use metrics_observability::{AdmissionOutcome, MetricsObservability};

    let observability = MetricsObservability::default();
    observability.record(AdmissionOutcome::KnownDenyDiscarded);
    observability.record(AdmissionOutcome::UnknownDiscarded);
    observability.record(AdmissionOutcome::InvalidAdmittedDiscarded);
    observability.record(AdmissionOutcome::Rejected);
    let output =
        metrics_snapshot::render_metrics(vec![], vec![], 0, 2_000, 60_000, false, &observability)
            .unwrap();
    assert!(output.contains(
        "cloud_fman_telemetry_metrics_admission_total{event=\"known_deny_discarded\"} 1 2000"
    ));
    assert!(
        output.contains("cloud_fman_telemetry_metrics_admission_total{event=\"rejected\"} 1 2000")
    );
    assert_eq!(
        output
            .matches("cloud_fman_telemetry_metrics_admission_total{")
            .count(),
        6
    );
    assert!(!output.contains("family="));
    assert!(!output.contains("fman_id="));
}

#[test]
fn exposes_fixed_collection_failure_stages() {
    use metrics_observability::{CollectionFailure, MetricsObservability};

    let observability = MetricsObservability::default();
    observability.record_collection_failure(CollectionFailure::Scrape);
    observability.record_collection_failure(CollectionFailure::Scrape);
    observability.record_collection_failure(CollectionFailure::Deadline);
    let output =
        metrics_snapshot::render_metrics(vec![], vec![], 0, 2_000, 60_000, false, &observability)
            .unwrap();
    for (stage, count) in [
        ("connect", 0),
        ("list", 0),
        ("scrape", 2),
        ("deadline", 1),
        ("saturated", 0),
    ] {
        let line = format!(
            "cloud_fman_telemetry_metrics_collection_failures_total{{stage=\"{stage}\"}} {count} 2000\n"
        );
        assert!(output.contains(&line), "{line}");
    }
    assert_eq!(
        output
            .matches("cloud_fman_telemetry_metrics_collection_failures_total{")
            .count(),
        5
    );
}

fn observed(observed_at_ms: i64, samples: &[(&str, u64)]) -> MetricsSnapshot {
    MetricsSnapshot {
        samples: samples
            .iter()
            .map(|(peer, value)| {
                format!("fm_consensus_items_processed_total{{guardian_seat_id=\"aa\",peer_id=\"{peer}\"}} {value}")
            })
            .collect(),
        ..snapshot("11", "aa", observed_at_ms)
    }
}

#[test]
fn seat_history_keeps_each_series_contiguous_and_in_observation_order() {
    use std::io::BufRead as _;

    // Values that sort the other way round as text than in time.
    let output = metrics_snapshot::render_metric_history(
        vec![
            observed(1_000, &[("1", 9), ("0", 90)]),
            observed(2_000, &[("1", 10), ("0", 100)]),
        ],
        0,
        false,
    )
    .unwrap();
    let samples: Vec<&str> = output
        .lines()
        .filter(|line| line.starts_with("fm_consensus_items_processed_total"))
        .collect();
    assert_eq!(
        samples,
        [
            "fm_consensus_items_processed_total{guardian_seat_id=\"aa\",peer_id=\"0\"} 90 1000",
            "fm_consensus_items_processed_total{guardian_seat_id=\"aa\",peer_id=\"0\"} 100 2000",
            "fm_consensus_items_processed_total{guardian_seat_id=\"aa\",peer_id=\"1\"} 9 1000",
            "fm_consensus_items_processed_total{guardian_seat_id=\"aa\",peer_id=\"1\"} 10 2000",
        ]
    );
    assert_eq!(
        output
            .matches("# TYPE fm_consensus_items_processed_total counter\n")
            .count(),
        1
    );
    assert!(!output.contains("cloud_fman_telemetry_"));
    assert!(!output.contains("omitted"));
    let scrape = prometheus_parse::Scrape::parse(std::io::Cursor::new(output).lines()).unwrap();
    assert_eq!(scrape.samples.len(), 4);
}

#[test]
fn seat_history_names_where_to_continue_after_omitted_observations() {
    let output = metrics_snapshot::render_metric_history(
        vec![observed(5_000, &[("0", 1)]), observed(6_000, &[("0", 2)])],
        3,
        false,
    )
    .unwrap();
    assert!(
        output.starts_with(
            "# 3 observations were invalid or did not fit; older ones: before_ms=5000\n"
        )
    );
}

#[test]
fn seat_history_rejects_observations_out_of_time_order() {
    for snapshots in [
        vec![observed(2_000, &[("0", 1)]), observed(1_000, &[("0", 2)])],
        vec![observed(1_000, &[("0", 1)]), observed(1_000, &[("0", 2)])],
    ] {
        assert!(metrics_snapshot::render_metric_history(snapshots, 0, false).is_err());
    }
}
