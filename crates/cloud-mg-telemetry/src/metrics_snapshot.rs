//! Timestamp-preserving private Prometheus exposition.

use std::{collections::BTreeMap, fmt::Write as _};

use crate::{metrics_observability::MetricsObservability, metrics_policy::exposition_family};

type ForwardedFamilies = BTreeMap<&'static str, (&'static str, Vec<(String, i64)>)>;

/// Latest durable observation for one authenticated guardian seat.
#[derive(Clone)]
pub(crate) struct MetricsSnapshot {
    /// Canonical registered MG public key.
    pub mg_id: String,
    /// Deterministic display-only name derived from `fman_id`.
    pub mg_name: String,
    /// Canonical seat id returned by the authenticated MG.
    pub guardian_seat_id: String,
    /// Canonical federation id derived from the invite asserted for that seat.
    pub federation_id: String,
    /// Successful collection time in Unix milliseconds.
    pub observed_at_ms: i64,
    /// Policy-admitted sample lines without source timestamps.
    pub samples: Vec<String>,
}

/// Current remote dependency state for one active registered MG.
pub(crate) struct MetricsTargetHealth {
    pub(crate) mg_id: String,
    pub(crate) mg_name: String,
    /// Whether the MG completed a full metrics poll within the freshness threshold.
    pub(crate) fresh: bool,
    /// Seat ids from the MG's latest successful seat listing.
    pub(crate) seats: Vec<String>,
}

/// Render durable sparse observations without changing their observation time.
pub(crate) fn render_metrics(
    mut snapshots: Vec<MetricsSnapshot>,
    mut targets: Vec<MetricsTargetHealth>,
    rejected_snapshots: usize,
    now_ms: i64,
    stale_after_ms: i64,
    method_source_ready: bool,
    observability: &MetricsObservability,
) -> Result<String, ExpositionError> {
    if stale_after_ms <= 0 {
        return Err(ExpositionError);
    }
    snapshots.sort_by(|left, right| {
        (&left.mg_id, &left.guardian_seat_id).cmp(&(&right.mg_id, &right.guardian_seat_id))
    });
    let mut target_samples = String::new();
    let mut observation_samples = String::new();
    let mut stale_samples = String::new();
    let mut seat_samples = String::new();
    // A seat is fresh only through a snapshot that passed persisted-row
    // revalidation; a rejected or missing observation reports 0.
    let observed: BTreeMap<(&str, &str), i64> = snapshots
        .iter()
        .map(|snapshot| {
            (
                (snapshot.mg_id.as_str(), snapshot.guardian_seat_id.as_str()),
                snapshot.observed_at_ms,
            )
        })
        .collect();
    targets.sort_by(|left, right| left.mg_id.cmp(&right.mg_id));
    for mut target in targets {
        target.seats.sort();
        for seat in &target.seats {
            let fresh = observed
                .get(&(target.mg_id.as_str(), seat.as_str()))
                .is_some_and(|&observed_at_ms| {
                    (0..=stale_after_ms).contains(&now_ms.saturating_sub(observed_at_ms))
                });
            writeln!(
                seat_samples,
                "cloud_fman_telemetry_seat_fresh{{fman_id=\"{}\",fman_name=\"{}\",guardian_seat_id=\"{}\"}} {} {now_ms}",
                escape(&target.mg_id),
                escape(&target.mg_name),
                escape(seat),
                i64::from(fresh),
            )
            .map_err(|_| ExpositionError)?;
        }
        writeln!(
            target_samples,
            "cloud_fman_telemetry_target_fresh{{fman_id=\"{}\",fman_name=\"{}\"}} {} {now_ms}",
            escape(&target.mg_id),
            escape(&target.mg_name),
            i64::from(target.fresh),
        )
        .map_err(|_| ExpositionError)?;
    }
    let mut forwarded: ForwardedFamilies = BTreeMap::new();
    for snapshot in snapshots {
        if snapshot.observed_at_ms < 0 || snapshot.observed_at_ms > now_ms {
            return Err(ExpositionError);
        }
        let labels = identity_labels(&snapshot);
        let stale = i64::from(now_ms.saturating_sub(snapshot.observed_at_ms) > stale_after_ms);
        writeln!(
            observation_samples,
            "cloud_fman_telemetry_snapshot_observation_timestamp_seconds{{{labels}}} {} {now_ms}",
            snapshot.observed_at_ms as f64 / 1000.0,
        )
        .map_err(|_| ExpositionError)?;
        writeln!(
            stale_samples,
            "cloud_fman_telemetry_snapshot_stale{{{labels}}} {stale} {now_ms}"
        )
        .map_err(|_| ExpositionError)?;
        for sample in snapshot.samples {
            if sample.contains('\n') || sample.starts_with('#') {
                return Err(ExpositionError);
            }
            let (family, kind) =
                exposition_family(&sample, method_source_ready).ok_or(ExpositionError)?;
            let group = forwarded.entry(family).or_insert((kind, Vec::new()));
            if group.0 != kind {
                return Err(ExpositionError);
            }
            group.1.push((sample, snapshot.observed_at_ms));
        }
    }
    let mut output = String::new();
    output.push_str("# HELP cloud_fman_telemetry_target_fresh Whether the active MG completed a full metrics poll within two configured polling intervals.\n");
    output.push_str("# TYPE cloud_fman_telemetry_target_fresh gauge\n");
    output.push_str(&target_samples);
    output.push_str("# HELP cloud_fman_telemetry_seat_fresh Whether a seat from the MG's latest seat listing has an admitted observation within two configured polling intervals.\n");
    output.push_str("# TYPE cloud_fman_telemetry_seat_fresh gauge\n");
    output.push_str(&seat_samples);
    output.push_str("# HELP cloud_fman_telemetry_snapshot_observation_timestamp_seconds Unix time of the latest admitted guardian observation.\n");
    output.push_str("# TYPE cloud_fman_telemetry_snapshot_observation_timestamp_seconds gauge\n");
    output.push_str(&observation_samples);
    output.push_str("# HELP cloud_fman_telemetry_snapshot_stale Whether the latest admitted guardian observation is older than two configured polling intervals.\n");
    output.push_str("# TYPE cloud_fman_telemetry_snapshot_stale gauge\n");
    output.push_str(&stale_samples);
    output.push_str("# HELP cloud_fman_telemetry_snapshot_rejected_rows Number of bounded persisted snapshot rows rejected before private exposition.\n");
    output.push_str("# TYPE cloud_fman_telemetry_snapshot_rejected_rows gauge\n");
    writeln!(
        output,
        "cloud_fman_telemetry_snapshot_rejected_rows {} {now_ms}",
        rejected_snapshots
    )
    .map_err(|_| ExpositionError)?;
    observability
        .render(&mut output, now_ms)
        .map_err(|_| ExpositionError)?;
    for (family, (kind, samples)) in forwarded {
        writeln!(
            output,
            "# HELP fm_{family} Vetted upstream Fedimint metric."
        )
        .map_err(|_| ExpositionError)?;
        writeln!(output, "# TYPE fm_{family} {kind}").map_err(|_| ExpositionError)?;
        for (sample, observed_at_ms) in samples {
            writeln!(output, "{sample} {observed_at_ms}").map_err(|_| ExpositionError)?;
        }
    }
    Ok(output)
}

fn identity_labels(snapshot: &MetricsSnapshot) -> String {
    format!(
        "federation_id=\"{}\",fman_id=\"{}\",fman_name=\"{}\",guardian_seat_id=\"{}\"",
        escape(&snapshot.federation_id),
        escape(&snapshot.mg_id),
        escape(&snapshot.mg_name),
        escape(&snapshot.guardian_seat_id)
    )
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

/// Persisted snapshot state could not be represented safely.
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("stored metrics snapshot was invalid")]
pub(crate) struct ExpositionError;
