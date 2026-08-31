//! Exact default-deny admission for guardian Prometheus text.

use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const INVENTORY_REVISION: &str = "stage0-federation-identity-v6";
const MAX_LINES: usize = 50_000;
const MAX_SAMPLES: usize = 20_000;
const MAX_FAMILIES: usize = 64;
const MAX_LABELS: usize = 12;
const MAX_LINE_BYTES: usize = 16 * 1024;
const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
// JSON persistence can nearly double escaped text; keep admitted text below half
// the per-seat serialized ceiling so every admitted observation is committable.
const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;

/// Verified labels attached to every admitted guardian sample.
pub struct MetricsIdentity<'a> {
    /// Canonical registered FMan public key.
    pub fman_id: &'a str,
    /// Deterministic display-only name derived from the public key.
    pub fman_name: &'a str,
    /// Seat selected through the authenticated FMan connection.
    pub guardian_seat_id: &'a str,
    /// Canonical federation id derived from the invite asserted for that seat.
    pub federation_id: &'a str,
}

/// Version-independent default-deny admission policy for guardian metrics.
#[derive(Default)]
pub struct MetricsPolicy;

/// One bounded, policy-admitted seat snapshot without a fabricated timestamp.
pub struct AdmittedMetrics {
    /// Canonical sample lines. The durable observation timestamp is appended on exposition.
    pub samples: Vec<String>,
    /// Whether at least one reviewed-deny family was discarded.
    pub discarded_known_deny: bool,
    /// Whether at least one unknown family was discarded.
    pub discarded_unknown: bool,
    /// Whether at least one malformed or invalid admitted family was discarded.
    pub discarded_invalid_admitted: bool,
}

impl MetricsPolicy {
    /// Stable identity of every policy choice that affects durable admission.
    pub fn fingerprint(&self) -> String {
        let digest = Sha256::digest(format!("{INVENTORY_REVISION}\0method-labels-allowlisted"));
        format!("{digest:x}")
    }
    /// Admit a response while cooperatively respecting the target's CPU deadline.
    pub fn admit_until(
        &self,
        body: &[u8],
        identity: MetricsIdentity<'_>,
        deadline: Option<std::time::Instant>,
    ) -> Result<AdmittedMetrics, MetricsPolicyError> {
        self.project_with_identity(body, Some(identity), deadline)
    }

    /// Project a source response without adding collector-owned identity labels.
    pub fn project_until(
        &self,
        body: &[u8],
        deadline: Option<std::time::Instant>,
    ) -> Result<AdmittedMetrics, MetricsPolicyError> {
        self.project_with_identity(body, None, deadline)
    }

    fn project_with_identity(
        &self,
        body: &[u8],
        identity: Option<MetricsIdentity<'_>>,
        deadline: Option<std::time::Instant>,
    ) -> Result<AdmittedMetrics, MetricsPolicyError> {
        check_deadline(deadline)?;
        if body.len() > MAX_INPUT_BYTES {
            return Err(MetricsPolicyError);
        }
        let text = std::str::from_utf8(body).map_err(|_| MetricsPolicyError)?;
        check_deadline(deadline)?;
        let mut families = BTreeSet::new();
        let mut admitted_families: BTreeMap<&'static str, FamilyStage> = BTreeMap::new();
        let mut sample_count = 0usize;
        let mut discarded_known_deny = false;
        let mut discarded_unknown = false;
        for (line_index, raw) in text.lines().enumerate() {
            check_deadline(deadline)?;
            if line_index >= MAX_LINES || raw.len() > MAX_LINE_BYTES {
                return Err(MetricsPolicyError);
            }
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if sample_count >= MAX_SAMPLES {
                return Err(MetricsPolicyError);
            }
            sample_count += 1;
            let name = sample_name(line)?;
            let shape = match shape(name, self.method_source_ready()) {
                Ok(shape) => shape,
                Err(_) => {
                    if let Some(family) = known_denied_family(name) {
                        families.insert(family.to_owned());
                        discarded_known_deny = true;
                    } else {
                        families.insert(name.to_owned());
                        discarded_unknown = true;
                    }
                    if families.len() > MAX_FAMILIES {
                        return Err(MetricsPolicyError);
                    }
                    continue;
                }
            };
            families.insert(shape.family.to_owned());
            if families.len() > MAX_FAMILIES {
                return Err(MetricsPolicyError);
            }
            let stage = admitted_families.entry(shape.family).or_default();
            let parsed = match ParsedSample::parse(line) {
                Ok(parsed) if parsed.name == name && parsed.labels.len() <= MAX_LABELS => parsed,
                _ => {
                    stage.tainted = true;
                    continue;
                }
            };
            if validate_labels(shape, &parsed.labels).is_err() {
                stage.tainted = true;
                continue;
            }
            let series_key = format!("{}{:?}", parsed.name, parsed.labels);
            if !stage.series.insert(series_key) {
                stage.tainted = true;
                continue;
            }
            if shape.family == "app_start_ts" {
                stage.release_markers += 1;
            }
            if shape.kind == Kind::Histogram {
                let mut base_labels = parsed.labels.clone();
                let bucket = base_labels.remove("le");
                let seen = stage
                    .histograms
                    .entry(format!("{}{:?}", shape.family, base_labels))
                    .or_default();
                let complete_part = match shape.suffix {
                    "_bucket" => bucket.is_some_and(|bucket| seen.buckets.insert(bucket)),
                    "_sum" if !seen.sum => {
                        seen.sum = true;
                        true
                    }
                    "_count" if !seen.count => {
                        seen.count = true;
                        true
                    }
                    _ => false,
                };
                if !complete_part {
                    stage.tainted = true;
                    continue;
                }
                seen.family = shape.family;
            }
            let value: f64 = match parsed.value.parse() {
                Ok(value) => value,
                Err(_) => {
                    stage.tainted = true;
                    continue;
                }
            };
            if !value.is_finite() || value.is_sign_negative() {
                stage.tainted = true;
                continue;
            }
            let mut labels = parsed.labels;
            let mut identity_collision = false;
            if let Some(identity) = &identity {
                for (name, value) in [
                    ("fman_id", identity.fman_id),
                    ("fman_name", identity.fman_name),
                    ("guardian_seat_id", identity.guardian_seat_id),
                    ("federation_id", identity.federation_id),
                ] {
                    if labels.insert(name.to_owned(), value.to_owned()).is_some() {
                        identity_collision = true;
                    }
                }
            }
            if identity_collision {
                stage.tainted = true;
                continue;
            }
            stage
                .samples
                .push(render_sample(&parsed.name, &labels, parsed.value));
            check_deadline(deadline)?;
        }
        check_deadline(deadline)?;
        let mut samples = Vec::new();
        let mut output_bytes = 0usize;
        let mut discarded_invalid_admitted = false;
        for (family, stage) in admitted_families {
            check_deadline(deadline)?;
            let histograms_complete = stage.histograms.values().all(|seen| {
                let expected = bucket_set(seen.family);
                seen.sum
                    && seen.count
                    && seen.buckets.len() == expected.len()
                    && expected.iter().all(|bucket| seen.buckets.contains(*bucket))
            });
            if stage.tainted
                || !histograms_complete
                || (family == "app_start_ts" && stage.release_markers != 1)
            {
                discarded_invalid_admitted = true;
                continue;
            }
            for sample in stage.samples {
                output_bytes = output_bytes
                    .checked_add(sample.len() + 1)
                    .ok_or(MetricsPolicyError)?;
                if output_bytes > MAX_OUTPUT_BYTES {
                    return Err(MetricsPolicyError);
                }
                samples.push(sample);
            }
        }
        check_deadline(deadline)?;
        Ok(AdmittedMetrics {
            samples,
            discarded_known_deny,
            discarded_unknown,
            discarded_invalid_admitted,
        })
    }

    /// Re-admit a persisted canonical snapshot against this policy and its stored identity.
    ///
    /// Stored lines are not trusted merely because a prior collector wrote them. This removes
    /// the collector-owned labels only after checking their exact values, then makes the normal
    /// admission path recreate them and requires byte-for-byte canonical equality.
    pub fn revalidate_persisted(
        &self,
        persisted: &[String],
        identity: MetricsIdentity<'_>,
    ) -> Result<(), MetricsPolicyError> {
        if persisted.len() > MAX_SAMPLES {
            return Err(MetricsPolicyError);
        }
        let mut raw = String::new();
        for sample in persisted {
            let ParsedSample {
                name,
                mut labels,
                value,
            } = ParsedSample::parse(sample)?;
            for (name, expected) in [
                ("fman_id", identity.fman_id),
                ("fman_name", identity.fman_name),
                ("guardian_seat_id", identity.guardian_seat_id),
                ("federation_id", identity.federation_id),
            ] {
                if labels.remove(name).as_deref() != Some(expected) {
                    return Err(MetricsPolicyError);
                }
            }
            let line = if labels.is_empty() {
                format!("{name} {value}")
            } else {
                render_sample(&name, &labels, value)
            };
            let next_len = raw
                .len()
                .checked_add(line.len() + 1)
                .ok_or(MetricsPolicyError)?;
            if next_len > MAX_OUTPUT_BYTES {
                return Err(MetricsPolicyError);
            }
            raw.push_str(&line);
            raw.push('\n');
        }
        let admitted = self.admit_until(raw.as_bytes(), identity, None)?;
        if admitted.samples == persisted {
            Ok(())
        } else {
            Err(MetricsPolicyError)
        }
    }

    /// Reports whether method-labeled families have a reviewed version-independent contract.
    pub fn method_source_ready(&self) -> bool {
        true
    }
}

#[doc(hidden)]
pub fn checked_source_manifest_matches_policy() -> bool {
    let manifest = include_str!("../../../docs/telemetry/fedimint-metrics-v0.11.1-fedi16.tsv")
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            if fields.next()? != "metric" {
                return None;
            }
            Some((fields.next()?, fields.next()?))
        })
        .collect::<BTreeSet<_>>();
    let policy_admitted = DEFAULT_ADMITTED_SOURCE_FAMILIES
        .iter()
        .map(|family| ("admit", *family))
        .collect::<BTreeSet<_>>();
    let policy_denied = DENIED_COUNTERS
        .iter()
        .chain(DENIED_HISTOGRAMS)
        .map(|family| ("deny", *family))
        .collect::<BTreeSet<_>>();
    manifest == policy_admitted.union(&policy_denied).copied().collect()
}

const DEFAULT_ADMITTED_SOURCE_FAMILIES: &[&str] = &[
    "app_start_ts",
    "backup_counts",
    "backup_write_size_bytes",
    "consensus_item_processing_duration_seconds",
    "consensus_item_processing_module_audit_duration_seconds",
    "consensus_items_processed_total",
    "consensus_ordering_latency_seconds",
    "consensus_peer_contribution_session_idx",
    "consensus_session_count",
    "consensus_tx_processed_inputs",
    "consensus_tx_processed_outputs",
    "iroh_api_connection_duration_seconds",
    "iroh_api_connections_active",
    "iroh_api_request_duration_seconds",
    "jsonrpc_api_request_duration_seconds",
    "jsonrpc_api_request_response_code_total",
    "ln_canceled_outgoing_contract_total",
    "ln_funded_contract_sats",
    "ln_incoming_offer_total",
    "lnv2_funded_contract_sats",
    "lnv2_outgoing_contract_settled_total",
    "mint_inout_fees_sats",
    "mint_inout_sats",
    "mint_issued_ecash_fees_sats",
    "mint_issued_ecash_sats",
    "mint_redeemed_ecash_fees_sats",
    "mint_redeemed_ecash_sats",
    "mintv2_inout_fees_sats",
    "mintv2_inout_sats",
    "mintv2_issued_ecash_fees_sats",
    "mintv2_issued_ecash_sats",
    "mintv2_redeemed_ecash_fees_sats",
    "mintv2_redeemed_ecash_sats",
    "peer_connect_total",
    "peer_disconnect_total",
    "peer_messages_total",
    "server_bitcoin_rpc_request_duration_seconds",
    "server_bitcoin_rpc_requests_total",
    "stored_backups_count",
    "total_backup_size",
    "wallet_block_count",
    "wallet_inout_fees_sats",
    "wallet_inout_sats",
    "wallet_pegin_fees_sats",
    "wallet_pegin_sats",
    "wallet_pegout_fees_sats",
    "wallet_pegout_sats",
    "walletv2_block_count",
    "walletv2_inout_fees_sats",
    "walletv2_inout_sats",
    "walletv2_pegin_fees_sats",
    "walletv2_pegin_sats",
    "walletv2_pegout_fees_sats",
    "walletv2_pegout_sats",
];

const DENIED_COUNTERS: &[&str] = &[
    "bitcoind_rpc_requests_total",
    "client_api_requests_total",
    "connector_connection_attempts_total",
    "ln_rpc_requests_total",
];

const DENIED_HISTOGRAMS: &[&str] = &[
    "bitcoind_rpc_request_duration_seconds",
    "client_api_request_duration_seconds",
    "connector_connection_duration_seconds",
    "gateway_htlc_handling_duration_seconds",
    "gateway_htlc_lnv1_attempt_duration_seconds",
    "gateway_htlc_lnv2_attempt_duration_seconds",
    "ln_rpc_request_duration_seconds",
];

fn sample_name(line: &str) -> Result<&str, MetricsPolicyError> {
    let end = line
        .find(|character: char| character == '{' || character.is_ascii_whitespace())
        .unwrap_or(line.len());
    if end == 0 {
        Err(MetricsPolicyError)
    } else {
        Ok(&line[..end])
    }
}

fn known_denied_family(name: &str) -> Option<&'static str> {
    let base = name.strip_prefix("fm_")?;
    if let Some(family) = DENIED_COUNTERS.iter().find(|family| **family == base) {
        return Some(family);
    }
    DENIED_HISTOGRAMS.iter().find_map(|family| {
        ["_bucket", "_sum", "_count"]
            .iter()
            .any(|suffix| {
                base.strip_suffix(suffix)
                    .is_some_and(|candidate| candidate == *family)
            })
            .then_some(*family)
    })
}

fn check_deadline(deadline: Option<std::time::Instant>) -> Result<(), MetricsPolicyError> {
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        Err(MetricsPolicyError)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Kind {
    Gauge,
    Counter,
    Histogram,
}

pub fn exposition_family(
    sample: &str,
    method_source_ready: bool,
) -> Option<(&'static str, &'static str)> {
    let name = sample
        .split_once('{')
        .map_or_else(|| sample.split_whitespace().next(), |(name, _)| Some(name))?;
    let shape = shape(name, method_source_ready).ok()?;
    let kind = match shape.kind {
        Kind::Gauge => "gauge",
        Kind::Counter => "counter",
        Kind::Histogram => "histogram",
    };
    Some((shape.family, kind))
}

#[derive(Clone, Copy)]
struct Shape {
    family: &'static str,
    kind: Kind,
    suffix: &'static str,
    labels: &'static [&'static str],
}

fn shape(name: &str, method_ready: bool) -> Result<Shape, MetricsPolicyError> {
    let base = name.strip_prefix("fm_").ok_or(MetricsPolicyError)?;
    let policy_family = if DEFAULT_ADMITTED_SOURCE_FAMILIES.contains(&base) {
        base
    } else {
        base.strip_suffix("_bucket")
            .or_else(|| base.strip_suffix("_sum"))
            .or_else(|| base.strip_suffix("_count"))
            .unwrap_or(base)
    };
    if !method_ready && !DEFAULT_ADMITTED_SOURCE_FAMILIES.contains(&policy_family) {
        return Err(MetricsPolicyError);
    }
    const GAUGES: &[(&str, &[&str])] = &[
        ("app_start_ts", &["version", "version_hash"]),
        ("stored_backups_count", &[]),
        ("total_backup_size", &[]),
        ("backup_counts", &["timeframe"]),
        ("consensus_session_count", &[]),
        (
            "consensus_peer_contribution_session_idx",
            &["self_id", "peer_id"],
        ),
        ("iroh_api_connections_active", &[]),
        ("wallet_block_count", &[]),
        ("walletv2_block_count", &[]),
    ];
    const COUNTERS: &[(&str, &[&str])] = &[
        ("consensus_items_processed_total", &["peer_id"]),
        ("peer_connect_total", &["self_id", "peer_id", "direction"]),
        ("peer_messages_total", &["self_id", "peer_id", "direction"]),
        ("peer_disconnect_total", &["self_id", "peer_id"]),
        ("ln_incoming_offer_total", &[]),
        ("ln_canceled_outgoing_contract_total", &[]),
        ("lnv2_outgoing_contract_settled_total", &["outcome"]),
        (
            "server_bitcoin_rpc_requests_total",
            &["method", "name", "result"],
        ),
    ];
    const HISTOGRAMS: &[(&str, &[&str])] = &[
        ("backup_write_size_bytes", &[]),
        ("consensus_tx_processed_inputs", &[]),
        ("consensus_tx_processed_outputs", &[]),
        ("consensus_ordering_latency_seconds", &[]),
        ("consensus_item_processing_duration_seconds", &["peer_id"]),
        (
            "consensus_item_processing_module_audit_duration_seconds",
            &["module_id", "module_kind"],
        ),
        ("iroh_api_connection_duration_seconds", &[]),
        ("mint_inout_sats", &["direction"]),
        ("mint_inout_fees_sats", &["direction"]),
        ("mint_redeemed_ecash_sats", &[]),
        ("mint_redeemed_ecash_fees_sats", &[]),
        ("mint_issued_ecash_sats", &[]),
        ("mint_issued_ecash_fees_sats", &[]),
        ("ln_funded_contract_sats", &["direction"]),
        ("lnv2_funded_contract_sats", &["direction"]),
        ("mintv2_inout_sats", &["direction"]),
        ("mintv2_inout_fees_sats", &["direction"]),
        ("mintv2_redeemed_ecash_sats", &[]),
        ("mintv2_redeemed_ecash_fees_sats", &[]),
        ("mintv2_issued_ecash_sats", &[]),
        ("mintv2_issued_ecash_fees_sats", &[]),
        ("wallet_inout_sats", &["direction"]),
        ("wallet_inout_fees_sats", &["direction"]),
        ("wallet_pegin_sats", &[]),
        ("wallet_pegin_fees_sats", &[]),
        ("wallet_pegout_sats", &[]),
        ("wallet_pegout_fees_sats", &[]),
        ("walletv2_inout_sats", &["direction"]),
        ("walletv2_inout_fees_sats", &["direction"]),
        ("walletv2_pegin_sats", &[]),
        ("walletv2_pegin_fees_sats", &[]),
        ("walletv2_pegout_sats", &[]),
        ("walletv2_pegout_fees_sats", &[]),
        (
            "server_bitcoin_rpc_request_duration_seconds",
            &["method", "name"],
        ),
    ];
    if let Some((family, labels)) = GAUGES.iter().find(|(family, _)| *family == base) {
        return Ok(Shape {
            family,
            kind: Kind::Gauge,
            suffix: "",
            labels,
        });
    }
    if let Some((family, labels)) = COUNTERS.iter().find(|(family, _)| *family == base) {
        return Ok(Shape {
            family,
            kind: Kind::Counter,
            suffix: "",
            labels,
        });
    }
    if method_ready && base == "jsonrpc_api_request_response_code_total" {
        return Ok(Shape {
            family: "jsonrpc_api_request_response_code_total",
            kind: Kind::Counter,
            suffix: "",
            labels: &["method", "code", "type"],
        });
    }
    let (family, suffix) = ["_bucket", "_sum", "_count"]
        .into_iter()
        .find_map(|suffix| base.strip_suffix(suffix).map(|family| (family, suffix)))
        .ok_or(MetricsPolicyError)?;
    let labels = if method_ready
        && matches!(
            family,
            "iroh_api_request_duration_seconds" | "jsonrpc_api_request_duration_seconds"
        ) {
        &["method"][..]
    } else {
        HISTOGRAMS
            .iter()
            .find(|(candidate, _)| *candidate == family)
            .map(|(_, labels)| *labels)
            .ok_or(MetricsPolicyError)?
    };
    Ok(Shape {
        family: HISTOGRAMS
            .iter()
            .find(|(candidate, _)| *candidate == family)
            .map(|(f, _)| *f)
            .unwrap_or_else(|| {
                if family == "iroh_api_request_duration_seconds" {
                    "iroh_api_request_duration_seconds"
                } else {
                    "jsonrpc_api_request_duration_seconds"
                }
            }),
        kind: Kind::Histogram,
        suffix,
        labels,
    })
}

fn validate_labels(
    shape: Shape,
    labels: &BTreeMap<String, String>,
) -> Result<(), MetricsPolicyError> {
    let mut expected: BTreeSet<&str> = shape.labels.iter().copied().collect();
    if shape.kind == Kind::Histogram && shape.suffix == "_bucket" {
        expected.insert("le");
    }
    if labels.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected {
        return Err(MetricsPolicyError);
    }
    for (name, value) in labels {
        let valid = match name.as_str() {
            "version" | "version_hash" => bounded_release_label(value),
            "timeframe" => matches!(value.as_str(), "1d" | "1w" | "1m" | "3m" | "all_time"),
            "direction" => matches!(value.as_str(), "incoming" | "outgoing"),
            "outcome" => matches!(value.as_str(), "claim" | "refund" | "cancel"),
            "module_id" => value == "65535",
            // `meta` is the bundled `fedimint-meta-server` module. It is
            // registered by the pinned source and loaded by real federations,
            // so a guardian audit histogram carries it like any other kind.
            // Omitting it here refused the complete scrape of every seat that
            // runs the module.
            "module_kind" => matches!(
                value.as_str(),
                "lnv2" | "meta" | "mintv2" | "walletv2" | "multi_sig_stability_pool"
            ),
            "peer_id" | "self_id" => value.len() <= 5 && value.parse::<u16>().is_ok(),
            "le" => valid_bucket(shape.family, value),
            "method" if shape.family.starts_with("server_bitcoin_rpc_") => matches!(
                value.as_str(),
                "get_block_count"
                    | "get_block_hash"
                    | "get_block"
                    | "get_feerate"
                    | "submit_transaction"
                    | "get_sync_progress"
                    | "get_chain_id"
            ),
            "method" => canonical_api_method(value),
            "name" => value == "server",
            "result" => matches!(value.as_str(), "success" | "error"),
            "code" => matches!(
                value.as_str(),
                "0" | "400"
                    | "401"
                    | "404"
                    | "500"
                    | "-32700"
                    | "-32600"
                    | "-32601"
                    | "-32602"
                    | "-32603"
            ),
            "type" => matches!(value.as_str(), "subscription" | "batch" | "default"),
            _ => false,
        };
        if !valid {
            return Err(MetricsPolicyError);
        }
    }
    Ok(())
}

fn bounded_release_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+' | b':')
        })
}

fn canonical_api_method(value: &str) -> bool {
    // The FMan and collector enforce this static set independently of the
    // producer release. A compromised producer cannot turn a caller-supplied
    // method string into an unbounded label value or an admitted new family.
    value == "unknown" || CORE_API_METHODS.contains(&value)
}

const CORE_API_METHODS: &[&str] = &[
    "api_announcements",
    "audit",
    "auth",
    "await_output_outcome",
    "await_outputs_outcomes",
    "await_session_outcome",
    "await_signed_session_outcome",
    "await_transaction",
    "backup",
    "backup_statistics",
    "chain_id",
    "change_password",
    "client_config",
    "client_config_json",
    "consensus_ord_latency",
    "download_guardian_backup",
    "federation_id",
    "fedimintd_version",
    "guardian_metadata",
    "invite_code",
    "p2p_connection_status",
    "recover",
    "server_config_consensus_hash",
    "session_count",
    "session_status",
    "setup_status",
    "shutdown",
    "sign_api_announcement",
    "sign_guardian_metadata",
    "signed_session_status",
    "status",
    "submit_api_announcement",
    "submit_guardian_metadata",
    "submit_transaction",
    "version",
];

fn valid_bucket(family: &str, value: &str) -> bool {
    bucket_set(family).contains(&value)
}

fn bucket_set(family: &str) -> &'static [&'static str] {
    const AMOUNT: &[&str] = &[
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
    ];
    const TX: &[&str] = &[
        "1", "2", "5", "10", "20", "50", "100", "200", "500", "1000", "2000", "5000", "+Inf",
    ];
    const BACKUP: &[&str] = &[
        "1", "10", "100", "1000", "5000", "10000", "50000", "100000", "1000000", "+Inf",
    ];
    const DURATION: &[&str] = &[
        "0.005", "0.01", "0.025", "0.05", "0.1", "0.25", "0.5", "1", "2.5", "5", "10", "+Inf",
    ];
    if family == "backup_write_size_bytes" {
        BACKUP
    } else if matches!(
        family,
        "consensus_tx_processed_inputs" | "consensus_tx_processed_outputs"
    ) {
        TX
    } else if family.ends_with("_sats") || family.ends_with("_fees_sats") {
        AMOUNT
    } else {
        DURATION
    }
}

#[derive(Default)]
struct HistogramSeen {
    family: &'static str,
    sum: bool,
    count: bool,
    buckets: BTreeSet<String>,
}

#[derive(Default)]
struct FamilyStage {
    samples: Vec<String>,
    series: BTreeSet<String>,
    histograms: BTreeMap<String, HistogramSeen>,
    release_markers: usize,
    tainted: bool,
}

struct ParsedSample<'a> {
    name: String,
    labels: BTreeMap<String, String>,
    value: &'a str,
}
impl<'a> ParsedSample<'a> {
    fn parse(line: &'a str) -> Result<Self, MetricsPolicyError> {
        let split = line.rfind(char::is_whitespace).ok_or(MetricsPolicyError)?;
        let (head, value) = (&line[..split], line[split..].trim());
        if value.is_empty() || value.contains(char::is_whitespace) {
            return Err(MetricsPolicyError);
        }
        let (name, labels) = if let Some(open) = head.find('{') {
            if !head.ends_with('}') {
                return Err(MetricsPolicyError);
            }
            (
                &head[..open],
                parse_labels(&head[open + 1..head.len() - 1])?,
            )
        } else {
            (head, BTreeMap::new())
        };
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b':'))
        {
            return Err(MetricsPolicyError);
        }
        Ok(Self {
            name: name.to_owned(),
            labels,
            value,
        })
    }
}

fn parse_labels(mut input: &str) -> Result<BTreeMap<String, String>, MetricsPolicyError> {
    let mut labels = BTreeMap::new();
    while !input.is_empty() {
        let eq = input.find('=').ok_or(MetricsPolicyError)?;
        let name = &input[..eq];
        input = &input[eq + 1..];
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || !input.starts_with('"')
        {
            return Err(MetricsPolicyError);
        }
        input = &input[1..];
        let mut value = String::new();
        let mut escaped = false;
        let mut end = None;
        for (idx, ch) in input.char_indices() {
            if escaped {
                match ch {
                    'n' => value.push('\n'),
                    '\\' => value.push('\\'),
                    '"' => value.push('"'),
                    _ => return Err(MetricsPolicyError),
                };
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                end = Some(idx);
                break;
            } else {
                value.push(ch);
            }
        }
        let end = end.ok_or(MetricsPolicyError)?;
        if labels.insert(name.to_owned(), value).is_some() {
            return Err(MetricsPolicyError);
        }
        input = &input[end + 1..];
        if input.is_empty() {
            break;
        }
        input = input.strip_prefix(',').ok_or(MetricsPolicyError)?;
    }
    Ok(labels)
}

fn render_sample(name: &str, labels: &BTreeMap<String, String>, value: &str) -> String {
    let labels = labels
        .iter()
        .map(|(name, value)| format!("{name}=\"{}\"", escape(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!("{name}{{{labels}}} {value}")
}
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

/// A guardian response did not exactly match the reviewed inventory.
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("guardian metrics response was not admitted")]
pub struct MetricsPolicyError;
