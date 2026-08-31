use fedi_decentralized_guardian_metrics_policy::{
    MetricsIdentity, MetricsPolicy, checked_source_manifest_matches_policy,
};

fn policy() -> MetricsPolicy {
    MetricsPolicy
}

fn identity() -> MetricsIdentity<'static> {
    MetricsIdentity {
        fman_id: "11",
        fman_name: "calm-tern",
        guardian_seat_id: "22",
        federation_id: "0000000000000000000000000000000000000000000000000000000000000000",
    }
}

fn release_marker() -> &'static str {
    "fm_app_start_ts{version=\"0.11.1\",version_hash=\"abc123\"} 1\n"
}

fn duration_histogram(family: &str, labels: &str) -> String {
    let mut body = release_marker().to_owned();
    for bucket in [
        "0.005", "0.01", "0.025", "0.05", "0.1", "0.25", "0.5", "1", "2.5", "5", "10", "+Inf",
    ] {
        body.push_str(&format!(
            "fm_{family}_bucket{{{labels}le=\"{bucket}\"}} 1\n"
        ));
    }
    body.push_str(&format!(
        "fm_{family}_sum{{{}}} 1\n",
        labels.trim_end_matches(',')
    ));
    body.push_str(&format!(
        "fm_{family}_count{{{}}} 1\n",
        labels.trim_end_matches(',')
    ));
    body
}

fn amount_histogram(family: &str, labels: &str) -> String {
    let mut body = String::new();
    for bucket in [
        "0",
        "0.1",
        "1",
        "10",
        "100",
        "1000",
        "10000",
        "100000",
        "1000000",
        "10000000",
        "100000000",
        "+Inf",
    ] {
        body.push_str(&format!(
            "fm_{family}_bucket{{{labels}le=\"{bucket}\"}} 1\n"
        ));
    }
    body.push_str(&format!(
        "fm_{family}_sum{{{}}} 1\n",
        labels.trim_end_matches(',')
    ));
    body.push_str(&format!(
        "fm_{family}_count{{{}}} 1\n",
        labels.trim_end_matches(',')
    ));
    body
}

#[test]
fn exact_inventory_adds_only_verified_identity() {
    let body = br#"
# TYPE fm_consensus_session_count gauge
fm_app_start_ts{version="0.11.1",version_hash="abc123"} 1
fm_consensus_session_count 7
fm_peer_messages_total{self_id="0",peer_id="1",direction="incoming"} 4
"#;
    let admitted = policy().admit_until(body, identity(), None).unwrap();
    assert_eq!(admitted.samples.len(), 3);
    assert!(
        admitted
            .samples
            .iter()
            .all(|line| line.contains("fman_id=\"11\""))
    );
    assert!(
        admitted
            .samples
            .iter()
            .all(|line| line.contains("fman_name=\"calm-tern\""))
    );
    assert!(
        admitted
            .samples
            .iter()
            .all(|line| line.contains("guardian_seat_id=\"22\""))
    );
    assert!(admitted.samples.iter().all(|line| line.contains(
        "federation_id=\"0000000000000000000000000000000000000000000000000000000000000000\""
    )));
}

#[test]
fn sanitized_fman_projection_remains_acceptable_to_the_collector() {
    let mut raw = duration_histogram("iroh_api_connection_duration_seconds", "");
    raw.push_str("fm_backup_counts{timeframe=\"1d\"} 7\n");
    raw.push_str("fm_future_private_value{secret=\"discard-me\"} 9\n");

    let projected = policy()
        .project_until(raw.as_bytes(), None)
        .expect("FMan projection");
    assert!(
        projected
            .samples
            .iter()
            .all(|sample| !sample.contains("fman_id"))
    );
    let sanitized = projected.samples.join("\n");

    let readmitted = policy()
        .admit_until(sanitized.as_bytes(), identity(), None)
        .expect("collector re-admission");
    assert_eq!(readmitted.samples.len(), projected.samples.len());
    assert!(
        readmitted
            .samples
            .iter()
            .all(|sample| sample.contains("fman_id=\"11\""))
    );
    assert!(
        readmitted
            .samples
            .iter()
            .any(|sample| sample.starts_with("fm_backup_counts{"))
    );
    assert!(
        readmitted
            .samples
            .iter()
            .any(|sample| sample.starts_with("fm_iroh_api_connection_duration_seconds_bucket{"))
    );
}

#[test]
fn unknown_and_invalid_families_do_not_suppress_an_unrelated_valid_family() {
    for rejected in [
        "fm_future_private_value 1",
        "fm_consensus_session_count{account=\"secret\"} 1",
        "fm_mint_inout_sats_bucket{direction=\"incoming\",le=\"1001\"} 1",
        "fm_consensus_session_count{fman_id=\"attacker\"} 1",
        "fm_consensus_session_count{federation_id=\"0000000000000000000000000000000000000000000000000000000000000000\"} 1",
        "fm_consensus_session_count{federation_id=\"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\"} 1",
        "fm_consensus_session_count{broken 1",
    ] {
        let body = format!(
            "{}fm_backup_counts{{timeframe=\"1d\"}} 7\n{rejected}",
            release_marker()
        );
        let admitted = policy()
            .admit_until(body.as_bytes(), identity(), None)
            .unwrap();
        assert_eq!(admitted.samples.len(), 2, "{rejected}");
        assert!(
            admitted
                .samples
                .iter()
                .any(|sample| sample.starts_with("fm_backup_counts{"))
        );
    }

    let suffix_lookalike = format!(
        "{}fm_backup_counts{{timeframe=\"1d\"}} 7\n\
         fm_backup_write_size_bytes_bucket_extra{{le=\"1\"}} 1",
        release_marker()
    );
    let admitted = policy()
        .admit_until(suffix_lookalike.as_bytes(), identity(), None)
        .unwrap();
    assert_eq!(admitted.samples.len(), 2);
    assert!(admitted.discarded_unknown);
    assert!(
        admitted
            .samples
            .iter()
            .all(|sample| !sample.contains("bucket_extra"))
    );
}

#[test]
fn pinned_client_and_connector_families_are_discarded_in_full() {
    for sample in [
        "fm_client_api_requests_total{method=\"wallet_balance\",peer_id=\"0\",result=\"success\"} 1",
        "fm_connector_connection_attempts_total{scheme=\"wss\",result=\"success\"} 1",
    ] {
        let body = format!("{}{sample}", release_marker());
        let admitted = policy()
            .admit_until(body.as_bytes(), identity(), None)
            .unwrap();
        assert_eq!(admitted.samples.len(), 1);
        assert!(admitted.discarded_known_deny);
    }
    for (family, labels) in [
        (
            "client_api_request_duration_seconds",
            "method=\"wallet_balance\",peer_id=\"0\",",
        ),
        ("connector_connection_duration_seconds", "scheme=\"wss\","),
    ] {
        let admitted = policy()
            .admit_until(
                duration_histogram(family, labels).as_bytes(),
                identity(),
                None,
            )
            .unwrap();
        assert_eq!(admitted.samples.len(), 1);
        assert!(admitted.discarded_known_deny, "{family}");
    }
}

#[test]
fn checked_source_manifest_matches_default_deny_policy() {
    assert!(checked_source_manifest_matches_policy());
}

#[test]
fn v2_module_metrics_have_exact_reviewed_shapes() {
    let mut body = release_marker().to_owned();
    body.push_str("fm_walletv2_block_count 1\n");
    body.push_str("fm_lnv2_outgoing_contract_settled_total{outcome=\"claim\"} 1\n");
    for (family, labels) in [
        ("lnv2_funded_contract_sats", "direction=\"incoming\","),
        ("mintv2_inout_sats", "direction=\"outgoing\","),
        ("mintv2_inout_fees_sats", "direction=\"incoming\","),
        ("mintv2_redeemed_ecash_sats", ""),
        ("mintv2_redeemed_ecash_fees_sats", ""),
        ("mintv2_issued_ecash_sats", ""),
        ("mintv2_issued_ecash_fees_sats", ""),
        ("walletv2_inout_sats", "direction=\"incoming\","),
        ("walletv2_inout_fees_sats", "direction=\"outgoing\","),
        ("walletv2_pegin_sats", ""),
        ("walletv2_pegin_fees_sats", ""),
        ("walletv2_pegout_sats", ""),
        ("walletv2_pegout_fees_sats", ""),
    ] {
        body.push_str(&amount_histogram(family, labels));
    }
    let admitted = policy()
        .admit_until(body.as_bytes(), identity(), None)
        .expect("every reviewed v2 module metric has its registered shape");
    assert!(!admitted.discarded_invalid_admitted);
    for family in [
        "walletv2_block_count",
        "lnv2_outgoing_contract_settled_total",
        "lnv2_funded_contract_sats",
        "mintv2_inout_sats",
        "mintv2_inout_fees_sats",
        "mintv2_redeemed_ecash_sats",
        "mintv2_redeemed_ecash_fees_sats",
        "mintv2_issued_ecash_sats",
        "mintv2_issued_ecash_fees_sats",
        "walletv2_inout_sats",
        "walletv2_inout_fees_sats",
        "walletv2_pegin_sats",
        "walletv2_pegin_fees_sats",
        "walletv2_pegout_sats",
        "walletv2_pegout_fees_sats",
    ] {
        assert!(
            admitted
                .samples
                .iter()
                .any(|sample| sample.starts_with(&format!("fm_{family}"))),
            "{family} should be admitted"
        );
    }
}

#[test]
fn release_labels_are_bounded_and_peer_dimensions_are_finite_u16_values() {
    let valid = "fm_app_start_ts{version=\"0.11.1\",version_hash=\"abc123\"} 1\n\
                 fm_backup_counts{timeframe=\"3m\"} 1\n\
                 fm_consensus_items_processed_total{peer_id=\"65535\"} 1";
    assert!(
        policy()
            .admit_until(valid.as_bytes(), identity(), None)
            .is_ok()
    );
    assert!(
        policy()
            .admit_until(
                b"fm_app_start_ts{version=\"other\",version_hash=\"different\"} 1",
                identity(),
                None
            )
            .is_ok()
    );
    for invalid in [
        "fm_backup_counts{timeframe=\"forever\"} 1",
        "fm_consensus_items_processed_total{peer_id=\"65536\"} 1",
    ] {
        let body = format!("{}{invalid}", release_marker());
        let admitted = policy()
            .admit_until(body.as_bytes(), identity(), None)
            .unwrap();
        assert_eq!(admitted.samples.len(), 1);
        assert!(admitted.discarded_invalid_admitted);
    }
}

#[test]
fn method_families_use_a_version_independent_canonical_allowlist() {
    let jsonrpc = duration_histogram("jsonrpc_api_request_duration_seconds", "method=\"status\",");
    let iroh = duration_histogram("iroh_api_request_duration_seconds", "method=\"status\",");
    for (body, family) in [
        (&jsonrpc, "jsonrpc_api_request_duration_seconds"),
        (&iroh, "iroh_api_request_duration_seconds"),
    ] {
        let admitted = policy()
            .admit_until(body.as_bytes(), identity(), None)
            .unwrap();
        assert!(
            admitted
                .samples
                .iter()
                .any(|sample| sample.starts_with(&format!("fm_{family}_bucket{{"))),
            "{family}"
        );
        assert!(!admitted.discarded_invalid_admitted, "{family}");
    }
    let counter = format!(
        "{}fm_jsonrpc_api_request_response_code_total{{method=\"status\",code=\"0\",type=\"default\"}} 1",
        release_marker()
    );
    let admitted = policy()
        .admit_until(counter.as_bytes(), identity(), None)
        .unwrap();
    assert!(
        admitted
            .samples
            .iter()
            .any(|sample| { sample.starts_with("fm_jsonrpc_api_request_response_code_total{") })
    );
    assert!(!admitted.discarded_invalid_admitted);
    let raw = duration_histogram(
        "jsonrpc_api_request_duration_seconds",
        "method=\"caller-secret\",",
    );
    let admitted = policy()
        .admit_until(raw.as_bytes(), identity(), None)
        .unwrap();
    assert_eq!(admitted.samples.len(), 1);
    assert!(admitted.discarded_invalid_admitted);
}

#[test]
fn canonical_method_metrics_survive_fman_projection_from_any_release() {
    for release in [
        "fm_app_start_ts{version=\"legacy\",version_hash=\"old-build\"} 1\n",
        "fm_app_start_ts{version=\"future\",version_hash=\"new-build\"} 1\n",
    ] {
        let raw = duration_histogram("jsonrpc_api_request_duration_seconds", "method=\"status\",")
            .replacen(release_marker(), release, 1);
        let projected = policy()
            .project_until(raw.as_bytes(), None)
            .expect("FMan projection");
        let admitted = policy()
            .admit_until(projected.samples.join("\n").as_bytes(), identity(), None)
            .expect("collector admission");

        assert_eq!(admitted.samples.len(), projected.samples.len());
        assert!(admitted
            .samples
            .iter()
            .any(|sample| sample.starts_with("fm_jsonrpc_api_request_duration_seconds_bucket{")));
    }
}

#[test]
fn release_markers_are_optional_and_invalid_markers_are_local() {
    let without_marker = policy()
        .admit_until(b"fm_backup_counts{timeframe=\"1d\"} 7", identity(), None)
        .unwrap();
    assert_eq!(without_marker.samples.len(), 1);
    let duplicate = format!(
        "{}fm_app_start_ts{{version=\"newer\",version_hash=\"different\"}} 1\n\
         fm_backup_counts{{timeframe=\"1d\"}} 7",
        release_marker(),
    );
    let admitted = policy()
        .admit_until(duplicate.as_bytes(), identity(), None)
        .unwrap();
    assert_eq!(admitted.samples.len(), 1);
    assert!(admitted.discarded_invalid_admitted);
    let invalid = b"fm_app_start_ts{version=\"invalid/value\",version_hash=\"hash\"} 1\n\
                    fm_backup_counts{timeframe=\"1d\"} 7";
    let admitted = policy().admit_until(invalid, identity(), None).unwrap();
    assert_eq!(admitted.samples.len(), 1);
    assert!(admitted.discarded_invalid_admitted);
    let incomplete = format!(
        "{}fm_backup_counts{{timeframe=\"1d\"}} 7\n\
         fm_backup_write_size_bytes_bucket{{le=\"1\"}} 1",
        release_marker()
    );
    let admitted = policy()
        .admit_until(incomplete.as_bytes(), identity(), None)
        .unwrap();
    assert_eq!(admitted.samples.len(), 2);
    assert!(admitted.discarded_invalid_admitted);

    let unisolatable = format!("{}{{bad family boundary}} 1", release_marker());
    assert!(
        policy()
            .admit_until(unisolatable.as_bytes(), identity(), None)
            .is_err()
    );

    let duplicate = format!(
        "{}fm_backup_counts{{timeframe=\"1d\"}} 7\n\
         fm_consensus_session_count 1\n\
         fm_consensus_session_count 2",
        release_marker()
    );
    let admitted = policy()
        .admit_until(duplicate.as_bytes(), identity(), None)
        .unwrap();
    assert_eq!(admitted.samples.len(), 2);
    assert!(
        admitted
            .samples
            .iter()
            .all(|sample| !sample.starts_with("fm_consensus_session_count"))
    );
}

#[test]
fn bitcoin_rpc_inventory_is_exact() {
    let mut body = duration_histogram(
        "server_bitcoin_rpc_request_duration_seconds",
        "method=\"get_block\",name=\"server\",",
    );
    body.push_str("fm_server_bitcoin_rpc_requests_total{method=\"get_block\",name=\"server\",result=\"success\"} 1\n");
    assert!(
        policy()
            .admit_until(body.as_bytes(), identity(), None)
            .is_ok()
    );
    let hostile = format!(
        "{}fm_server_bitcoin_rpc_requests_total{{method=\"raw\",name=\"server\",result=\"success\"}} 1",
        release_marker()
    );
    let admitted = policy()
        .admit_until(hostile.as_bytes(), identity(), None)
        .unwrap();
    assert_eq!(admitted.samples.len(), 1);
    assert!(admitted.discarded_invalid_admitted);
}

#[test]
fn malformed_and_hostile_cardinality_are_bounded() {
    for body in [
        b"fm_consensus_session_count -1".as_slice(),
        b"fm_consensus_session_count NaN".as_slice(),
    ] {
        let admitted = policy().admit_until(body, identity(), None).unwrap();
        assert!(admitted.samples.is_empty());
        assert!(admitted.discarded_invalid_admitted);
    }
    let admitted = policy()
        .admit_until(b"fm_consensus_session_count 1 2", identity(), None)
        .unwrap();
    assert!(admitted.samples.is_empty());
    assert!(admitted.discarded_invalid_admitted);
    let too_many = format!(
        "{}{}",
        release_marker(),
        "fm_consensus_session_count 1\n".repeat(20_000)
    );
    assert!(
        policy()
            .admit_until(too_many.as_bytes(), identity(), None)
            .is_err()
    );
    let oversized_line = format!(
        "{}fm_consensus_session_count{{x=\"{}\"}} 1",
        release_marker(),
        "x".repeat(20_000)
    );
    assert!(
        policy()
            .admit_until(oversized_line.as_bytes(), identity(), None)
            .is_err()
    );
}

#[test]
fn signed_negative_values_discard_the_family_without_rewriting_valid_lexemes() {
    for value in [
        "-1", "-0", "-0.0", "-1e-999", "-1e308", "NaN", "inf", "+Inf",
    ] {
        let body = format!("{}fm_consensus_session_count {value}", release_marker());
        let admitted = policy()
            .admit_until(body.as_bytes(), identity(), None)
            .unwrap();
        assert_eq!(admitted.samples.len(), 1);
        assert!(admitted.discarded_invalid_admitted);
    }

    for value in [
        "0",
        "0.0",
        "1e-999",
        "5e-324",
        "1e308",
        "1.7976931348623157e308",
    ] {
        let body = format!("{}fm_consensus_session_count {value}", release_marker());
        let admitted = policy()
            .admit_until(body.as_bytes(), identity(), None)
            .unwrap();
        assert!(
            admitted
                .samples
                .iter()
                .any(|sample| sample.ends_with(&format!(" {value}"))),
            "{value} was rewritten"
        );
    }

    for value in ["-Inf", "1e309"] {
        let body = format!("{}fm_consensus_session_count {value}", release_marker());
        let admitted = policy()
            .admit_until(body.as_bytes(), identity(), None)
            .unwrap();
        assert_eq!(admitted.samples.len(), 1);
        assert!(admitted.discarded_invalid_admitted);
    }
}

#[test]
fn maximal_hostile_body_observes_an_elapsed_parse_deadline() {
    let body = vec![b'#'; 4 * 1024 * 1024];
    assert!(
        policy()
            .admit_until(
                &body,
                MetricsIdentity {
                    fman_id: "11",
                    fman_name: "calm-tern",
                    guardian_seat_id: "aa",
                    federation_id: "0000000000000000000000000000000000000000000000000000000000000000",
                },
                Some(std::time::Instant::now()),
            )
            .is_err()
    );
}

#[test]
fn persisted_samples_must_match_current_policy_identity_and_canonical_form() {
    let admitted = policy()
        .admit_until(
            b"fm_app_start_ts{version=\"0.11.1\",version_hash=\"abc123\"} 1\n\
              fm_consensus_session_count 7",
            identity(),
            None,
        )
        .unwrap();
    assert!(
        policy()
            .revalidate_persisted(&admitted.samples, identity())
            .is_ok()
    );

    for samples in [
        admitted
            .samples
            .iter()
            .map(|sample| {
                sample.replace(
                    "federation_id=\"0000000000000000000000000000000000000000000000000000000000000000\"",
                    "federation_id=\"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\"",
                )
            })
            .collect(),
        admitted
            .samples
            .iter()
            .map(|sample| {
                sample.replace("guardian_seat_id=\"22\"", "guardian_seat_id=\"attacker\"")
            })
            .collect(),
        admitted
            .samples
            .iter()
            .map(|sample| sample.replace("} ", ",capability=\"held-secret\"} "))
            .collect(),
        vec![
            admitted.samples[0].clone(),
            admitted.samples[1].clone(),
            admitted.samples[1].clone(),
        ],
        vec![admitted.samples[1].clone(); 20_001],
    ] {
        assert!(
            policy()
                .revalidate_persisted(&samples, identity())
                .is_err()
        );
    }
}

#[test]
fn revalidation_accepts_canonical_identity_labels_over_the_raw_line_limit() {
    let value = format!(
        "{}1",
        "0".repeat(16 * 1024 - "fm_consensus_session_count ".len() - 1)
    );
    let body = format!("{}fm_consensus_session_count {value}", release_marker());
    let admitted = policy()
        .admit_until(body.as_bytes(), identity(), None)
        .unwrap();
    assert!(
        admitted.samples[1].len() > 16 * 1024,
        "the collector-owned identity labels make the persisted line longer"
    );
    assert!(
        policy()
            .revalidate_persisted(&admitted.samples, identity())
            .is_ok()
    );
}

// ---------------------------------------------------------------------------
// Replay of a real producer response.
//
// Everything above builds bodies by hand, which checks each gate in isolation
// but can only assert what the test author already believed the producer emits.
// `checked_source_manifest_matches_policy` closes half of that gap by pinning
// the policy to the registrations found in the pinned source. It is still a
// source-level check: a family can be registered and never emitted, a label can
// take a value nobody enumerated, and a histogram can be filled with buckets
// nobody listed. None of that is visible until a real guardian is scraped.
//
// These tests replay one complete captured seat response through the shipped
// policy. See the fixture header for its provenance and re-capture rule.
// ---------------------------------------------------------------------------

/// Complete Prometheus response from one fedi15 seat used to validate legacy shape support.
const CAPTURED_SCRAPE: &str =
    include_str!("../../../docs/telemetry/fedimint-metrics-v0.11.1-fedi15-seat-scrape.txt");

/// The deployed policy accepts valid shapes from any source release.
fn captured_policy() -> MetricsPolicy {
    MetricsPolicy
}

/// Families carrying the given disposition in the reviewed source inventory.
fn inventoried(disposition: &str) -> std::collections::BTreeSet<&'static str> {
    include_str!("../../../docs/telemetry/fedimint-metrics-v0.11.1-fedi16.tsv")
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            if fields.next()? != "metric" {
                return None;
            }
            let found = fields.next()?;
            let family = fields.next()?;
            (found == disposition).then_some(family)
        })
        .collect()
}

/// Reduce one sample name to its inventory family, mirroring the policy's own
/// order: a name that is already a family keeps its generated suffix.
fn captured_family(name: &str, known: &std::collections::BTreeSet<&'static str>) -> String {
    let base = name.strip_prefix("fm_").unwrap_or(name);
    if known.contains(base) {
        return base.to_owned();
    }
    base.strip_suffix("_bucket")
        .or_else(|| base.strip_suffix("_sum"))
        .or_else(|| base.strip_suffix("_count"))
        .unwrap_or(base)
        .to_owned()
}

/// Group the captured response into one line list per inventory family.
fn captured_by_family() -> std::collections::BTreeMap<String, Vec<&'static str>> {
    let mut known = inventoried("admit");
    known.extend(inventoried("deny"));
    let mut grouped: std::collections::BTreeMap<String, Vec<&'static str>> =
        std::collections::BTreeMap::new();
    for raw in CAPTURED_SCRAPE.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let name = line.split_once('{').map_or_else(
            || line.split_whitespace().next().unwrap_or(line),
            |(n, _)| n,
        );
        grouped
            .entry(captured_family(name, &known))
            .or_default()
            .push(line);
    }
    grouped
}

#[test]
fn a_real_producer_emits_only_families_the_inventory_classified() {
    let admit = inventoried("admit");
    let deny = inventoried("deny");
    let unclassified: Vec<String> = captured_by_family()
        .into_keys()
        .filter(|family| !admit.contains(family.as_str()) && !deny.contains(family.as_str()))
        .collect();
    assert!(
        unclassified.is_empty(),
        "the legacy fixture emits families the current reviewed inventory never classified: {unclassified:?}"
    );
}

#[test]
fn every_non_api_admitted_family_a_real_producer_emits_is_retained() {
    let admit = inventoried("admit");
    for (family, lines) in captured_by_family() {
        if !admit.contains(family.as_str())
            || matches!(
                family.as_str(),
                "iroh_api_request_duration_seconds"
                    | "jsonrpc_api_request_duration_seconds"
                    | "jsonrpc_api_request_response_code_total"
            )
        {
            continue;
        }
        let body = lines.join("\n");
        let admitted = captured_policy()
            .admit_until(body.as_bytes(), identity(), None)
            .unwrap();
        assert!(
            admitted
                .samples
                .iter()
                .any(|sample| sample.starts_with(&format!("fm_{family}"))),
            "family fm_{family} is inventoried `admit`, but the legacy fixture has no retained \
             samples under the version-independent policy"
        );
    }
}

#[test]
fn complete_real_scrape_projects_exact_output_and_unknown_is_locally_discarded() {
    let admitted = captured_policy()
        .admit_until(CAPTURED_SCRAPE.as_bytes(), identity(), None)
        .expect(
            "the complete legacy scrape must discard reviewed-deny families while admitting \
             every reviewed-safe sample",
        );
    assert!(admitted.discarded_known_deny);
    assert!(!admitted.discarded_unknown);
    assert!(admitted.discarded_invalid_admitted);
    let denied = inventoried("deny");
    let emitted_denied = captured_by_family()
        .into_keys()
        .filter(|family| denied.contains(family.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        emitted_denied,
        [
            "client_api_request_duration_seconds",
            "client_api_requests_total",
            "connector_connection_attempts_total",
            "connector_connection_duration_seconds",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    assert!(admitted.samples.iter().all(|sample| {
        let name = sample.split_once('{').map_or_else(
            || sample.split_whitespace().next().unwrap(),
            |(name, _)| name,
        );
        !denied.contains(captured_family(name, &denied).as_str())
    }));
    assert!(admitted.samples.iter().all(|sample| {
        !sample.starts_with("fm_iroh_api_request_duration_seconds")
            && !sample.starts_with("fm_jsonrpc_api_request_duration_seconds")
            && !sample.starts_with("fm_jsonrpc_api_request_response_code_total")
    }));
    let expected_body = captured_by_family()
        .into_iter()
        .filter(|(family, _)| !denied.contains(family.as_str()))
        .flat_map(|(_, lines)| lines)
        .collect::<Vec<_>>()
        .join("\n");
    let expected = captured_policy()
        .admit_until(expected_body.as_bytes(), identity(), None)
        .unwrap();
    assert_eq!(admitted.samples, expected.samples);

    let hostile = format!("{CAPTURED_SCRAPE}\nfm_future_private_value 1\n");
    let projected = captured_policy()
        .admit_until(hostile.as_bytes(), identity(), None)
        .unwrap();
    assert_eq!(projected.samples, admitted.samples);
    assert!(projected.discarded_unknown);
}
