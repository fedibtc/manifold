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

fn snapshot(fman: &str, seat: &str, observed_at_ms: i64) -> MetricsSnapshot {
    MetricsSnapshot {
        fman_id: fman.to_owned(),
        fman_name: "same-display-name".to_owned(),
        guardian_seat_id: seat.to_owned(),
        federation_id: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_owned(),
        observed_at_ms,
        samples: vec![format!(
            "fm_consensus_session_count{{fman_id=\"{fman}\",fman_name=\"same-display-name\",guardian_seat_id=\"{seat}\"}} 7"
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
fn colliding_names_and_seats_cannot_merge_fman_identities() {
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

    let mut disabled_method_family = snapshot("11", "aa", 1_000);
    disabled_method_family.samples = vec![
        "fm_jsonrpc_api_request_response_code_total{fman_id=\"11\",fman_name=\"same-display-name\",guardian_seat_id=\"aa\",method=\"unknown\",response_code=\"200\"} 1".into(),
    ];
    assert!(
        render_metrics(
            vec![disabled_method_family],
            vec![],
            0,
            2_000,
            60_000,
            false
        )
        .is_err()
    );
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
                fman_id: "11".into(),
                fman_name: "same".into(),
                fresh: true,
            },
            MetricsTargetHealth {
                fman_id: "22".into(),
                fman_name: "same".into(),
                fresh: false,
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
