use fedi_decentralized_guardian_metrics_policy::{
    MetricsIdentity, MetricsPolicy, checked_source_manifest_matches_policy,
};

fn policy(methods: bool) -> MetricsPolicy<'static> {
    MetricsPolicy {
        version: "0.11.1",
        version_hash: "abc123",
        canonical_method_labels: methods,
    }
}

fn identity() -> MetricsIdentity<'static> {
    MetricsIdentity {
        fman_id: "11",
        fman_name: "calm-tern",
        guardian_seat_id: "22",
        asserted_federation_id: "0000000000000000000000000000000000000000000000000000000000000000",
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

#[test]
fn exact_inventory_adds_only_bounded_identity_labels() {
    let body = br#"
# TYPE fm_consensus_session_count gauge
fm_app_start_ts{version="0.11.1",version_hash="abc123"} 1
fm_consensus_session_count 7
fm_peer_messages_total{self_id="0",peer_id="1",direction="incoming"} 4
"#;
    let admitted = policy(false).admit_until(body, identity(), None).unwrap();
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
        "asserted_federation_id=\"0000000000000000000000000000000000000000000000000000000000000000\""
    )));
    assert!(
        admitted
            .samples
            .iter()
            .all(|line| { !line.contains("{federation_id=") && !line.contains(",federation_id=") })
    );
}

#[test]
fn sanitized_fman_projection_remains_acceptable_to_the_collector() {
    let mut raw = duration_histogram("iroh_api_connection_duration_seconds", "");
    raw.push_str("fm_backup_counts{timeframe=\"1d\"} 7\n");
    raw.push_str("fm_future_private_value{secret=\"discard-me\"} 9\n");

    let projected = policy(false)
        .project_until(raw.as_bytes(), None)
        .expect("FMan projection");
    assert!(
        projected
            .samples
            .iter()
            .all(|sample| !sample.contains("fman_id"))
    );
    let sanitized = projected.samples.join("\n");

    let readmitted = policy(false)
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
        "fm_consensus_session_count{asserted_federation_id=\"0000000000000000000000000000000000000000000000000000000000000000\"} 1",
        "fm_consensus_session_count{asserted_federation_id=\"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\"} 1",
        "fm_consensus_session_count{broken 1",
    ] {
        let body = format!(
            "{}fm_backup_counts{{timeframe=\"1d\"}} 7\n{rejected}",
            release_marker()
        );
        let admitted = policy(false)
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
    let admitted = policy(false)
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
        let admitted = policy(false)
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
        let admitted = policy(false)
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
fn release_labels_are_exact_and_peer_dimensions_are_finite_u16_values() {
    let valid = "fm_app_start_ts{version=\"0.11.1\",version_hash=\"abc123\"} 1\n\
                 fm_backup_counts{timeframe=\"3m\"} 1\n\
                 fm_consensus_items_processed_total{peer_id=\"65535\"} 1";
    assert!(
        policy(false)
            .admit_until(valid.as_bytes(), identity(), None)
            .is_ok()
    );
    assert!(
        policy(false)
            .admit_until(
                b"fm_app_start_ts{version=\"other\",version_hash=\"abc123\"} 1",
                identity(),
                None
            )
            .is_err()
    );
    for invalid in [
        "fm_backup_counts{timeframe=\"forever\"} 1",
        "fm_consensus_items_processed_total{peer_id=\"65536\"} 1",
    ] {
        let body = format!("{}{invalid}", release_marker());
        let admitted = policy(false)
            .admit_until(body.as_bytes(), identity(), None)
            .unwrap();
        assert_eq!(admitted.samples.len(), 1);
        assert!(admitted.discarded_invalid_admitted);
    }
}

#[test]
fn method_families_remain_disabled_without_a_reviewed_combined_source_hash() {
    let jsonrpc = duration_histogram("jsonrpc_api_request_duration_seconds", "method=\"status\",");
    let iroh = duration_histogram("iroh_api_request_duration_seconds", "method=\"unknown\",");
    for (body, family) in [
        (&jsonrpc, "jsonrpc_api_request_duration_seconds"),
        (&iroh, "iroh_api_request_duration_seconds"),
    ] {
        for methods in [false, true] {
            let admitted = policy(methods)
                .admit_until(body.as_bytes(), identity(), None)
                .unwrap();
            assert_eq!(admitted.samples.len(), 1);
            assert!(admitted.discarded_known_deny, "{family}");
        }
    }
    let raw = duration_histogram(
        "jsonrpc_api_request_duration_seconds",
        "method=\"caller-secret\",",
    );
    let admitted = policy(true)
        .admit_until(raw.as_bytes(), identity(), None)
        .unwrap();
    assert_eq!(admitted.samples.len(), 1);
    assert!(admitted.discarded_known_deny);
}

#[test]
fn missing_or_duplicate_release_fails_globally_but_incomplete_histogram_is_local() {
    assert!(policy(false).admit_until(b"", identity(), None).is_err());
    let duplicate = format!("{}{}{}", release_marker(), release_marker(), "");
    assert!(
        policy(false)
            .admit_until(duplicate.as_bytes(), identity(), None)
            .is_err()
    );
    let incomplete = format!(
        "{}fm_backup_counts{{timeframe=\"1d\"}} 7\n\
         fm_backup_write_size_bytes_bucket{{le=\"1\"}} 1",
        release_marker()
    );
    let admitted = policy(false)
        .admit_until(incomplete.as_bytes(), identity(), None)
        .unwrap();
    assert_eq!(admitted.samples.len(), 2);
    assert!(admitted.discarded_invalid_admitted);

    let unisolatable = format!("{}{{bad family boundary}} 1", release_marker());
    assert!(
        policy(false)
            .admit_until(unisolatable.as_bytes(), identity(), None)
            .is_err()
    );

    let duplicate = format!(
        "{}fm_backup_counts{{timeframe=\"1d\"}} 7\n\
         fm_consensus_session_count 1\n\
         fm_consensus_session_count 2",
        release_marker()
    );
    let admitted = policy(false)
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
        policy(false)
            .admit_until(body.as_bytes(), identity(), None)
            .is_ok()
    );
    let hostile = format!(
        "{}fm_server_bitcoin_rpc_requests_total{{method=\"raw\",name=\"server\",result=\"success\"}} 1",
        release_marker()
    );
    let admitted = policy(false)
        .admit_until(hostile.as_bytes(), identity(), None)
        .unwrap();
    assert_eq!(admitted.samples.len(), 1);
    assert!(admitted.discarded_invalid_admitted);
}

#[test]
fn malformed_and_hostile_cardinality_are_bounded() {
    assert!(
        policy(false)
            .admit_until(b"fm_consensus_session_count -1", identity(), None)
            .is_err()
    );
    assert!(
        policy(false)
            .admit_until(b"fm_consensus_session_count NaN", identity(), None)
            .is_err()
    );
    assert!(
        policy(false)
            .admit_until(b"fm_consensus_session_count 1 2", identity(), None)
            .is_err()
    );
    let too_many = format!(
        "{}{}",
        release_marker(),
        "fm_consensus_session_count 1\n".repeat(20_000)
    );
    assert!(
        policy(false)
            .admit_until(too_many.as_bytes(), identity(), None)
            .is_err()
    );
    let oversized_line = format!(
        "{}fm_consensus_session_count{{x=\"{}\"}} 1",
        release_marker(),
        "x".repeat(20_000)
    );
    assert!(
        policy(false)
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
        let admitted = policy(false)
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
        let admitted = policy(false)
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
        let admitted = policy(false)
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
        policy(false)
            .admit_until(
                &body,
                MetricsIdentity {
                    fman_id: "11",
                    fman_name: "calm-tern",
                    guardian_seat_id: "aa",
                    asserted_federation_id: "0000000000000000000000000000000000000000000000000000000000000000",
                },
                Some(std::time::Instant::now()),
            )
            .is_err()
    );
}

#[test]
fn persisted_samples_must_match_current_policy_identity_and_canonical_form() {
    let admitted = policy(false)
        .admit_until(
            b"fm_app_start_ts{version=\"0.11.1\",version_hash=\"abc123\"} 1\n\
              fm_consensus_session_count 7",
            identity(),
            None,
        )
        .unwrap();
    assert!(
        policy(false)
            .revalidate_persisted(&admitted.samples, identity())
            .is_ok()
    );

    for samples in [
        admitted
            .samples
            .iter()
            .map(|sample| {
                sample.replace(
                    "asserted_federation_id=\"0000000000000000000000000000000000000000000000000000000000000000\"",
                    "asserted_federation_id=\"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\"",
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
            policy(false)
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
    let admitted = policy(false)
        .admit_until(body.as_bytes(), identity(), None)
        .unwrap();
    assert!(
        admitted.samples[1].len() > 16 * 1024,
        "the collector-owned identity labels make the persisted line longer"
    );
    assert!(
        policy(false)
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

/// Complete Prometheus response from one running seat at the pinned release.
const CAPTURED_SCRAPE: &str =
    include_str!("../../../docs/telemetry/fedimint-metrics-v0.11.1-fedi15-seat-scrape.txt");

/// The release the captured response reports, which the policy matches exactly.
const CAPTURED_VERSION: &str = "0.11.1";
const CAPTURED_VERSION_HASH: &str = "4c70c0e54f2f6a25df518c5082ac5a81d7a46d70";

/// The deployed policy: method-labeled families stay disabled.
fn captured_policy() -> MetricsPolicy<'static> {
    MetricsPolicy {
        version: CAPTURED_VERSION,
        version_hash: CAPTURED_VERSION_HASH,
        canonical_method_labels: false,
    }
}

/// The captured response's own release marker, which every sub-body needs.
fn captured_release_marker() -> &'static str {
    CAPTURED_SCRAPE
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("fm_app_start_ts"))
        .expect("the captured scrape must carry its release marker")
}

/// Families carrying the given disposition in the reviewed source inventory.
fn inventoried(disposition: &str) -> std::collections::BTreeSet<&'static str> {
    include_str!("../../../docs/telemetry/fedimint-metrics-v0.11.1-fedi15.tsv")
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
        "the running producer emits families the reviewed inventory never classified, \
         so the inventory is stale for its own pin: {unclassified:?}"
    );
}

#[test]
fn every_admitted_family_a_real_producer_emits_is_accepted() {
    let admit = inventoried("admit");
    for (family, lines) in captured_by_family() {
        if !admit.contains(family.as_str()) {
            continue;
        }
        // Every response must carry exactly one release marker.
        let body = if family == "app_start_ts" {
            lines.join("\n")
        } else {
            format!("{}\n{}", captured_release_marker(), lines.join("\n"))
        };
        assert!(
            captured_policy()
                .admit_until(body.as_bytes(), identity(), None)
                .is_ok(),
            "family fm_{family} is inventoried `admit`, and the running producer emits it, \
             but the shipped policy refuses the real samples. The inventory disagrees with \
             the producer it was written for: a label value, bucket, or type is not what \
             was enumerated."
        );
    }
}

#[test]
fn complete_real_scrape_projects_exact_output_and_unknown_is_locally_discarded() {
    let admitted = captured_policy()
        .admit_until(CAPTURED_SCRAPE.as_bytes(), identity(), None)
        .expect(
            "the complete real scrape must discard reviewed-deny families while admitting \
             every reviewed-safe sample",
        );
    assert_eq!(admitted.samples.len(), 341);
    assert!(admitted.discarded_known_deny);
    assert!(!admitted.discarded_unknown);
    assert!(!admitted.discarded_invalid_admitted);
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
            "iroh_api_request_duration_seconds",
            "jsonrpc_api_request_duration_seconds",
            "jsonrpc_api_request_response_code_total",
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
