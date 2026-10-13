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
    pub(crate) fresh: bool,
    /// Newest seat capacity and its journal reception time in Unix milliseconds.
    pub(crate) seat_capacity: Option<(i64, SeatCapacity)>,
}

/// Fields of the MG's `seat capacity` safe-journal event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
pub(crate) struct SeatCapacity {
    pub(crate) max_seats: u32,
    pub(crate) available_seats: u32,
    pub(crate) accepting_seats: bool,
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
    let mut max_seat_samples = String::new();
    let mut available_seat_samples = String::new();
    let mut accepting_seat_samples = String::new();
    let mut seat_observation_samples = String::new();
    targets.sort_by(|left, right| left.mg_id.cmp(&right.mg_id));
    for target in targets {
        let labels = format!(
            "fman_id=\"{}\",fman_name=\"{}\"",
            escape(&target.mg_id),
            escape(&target.mg_name),
        );
        writeln!(
            target_samples,
            "cloud_fman_telemetry_target_fresh{{{labels}}} {} {now_ms}",
            i64::from(target.fresh),
        )
        .map_err(|_| ExpositionError)?;
        let Some((observed_at_ms, capacity)) = target.seat_capacity else {
            continue;
        };
        if observed_at_ms < 0 || observed_at_ms > now_ms {
            return Err(ExpositionError);
        }
        writeln!(
            max_seat_samples,
            "cloud_fman_telemetry_seat_capacity_max_seats{{{labels}}} {} {observed_at_ms}",
            capacity.max_seats,
        )
        .map_err(|_| ExpositionError)?;
        writeln!(
            available_seat_samples,
            "cloud_fman_telemetry_seat_capacity_available_seats{{{labels}}} {} {observed_at_ms}",
            capacity.available_seats,
        )
        .map_err(|_| ExpositionError)?;
        writeln!(
            accepting_seat_samples,
            "cloud_fman_telemetry_seat_capacity_accepting_seats{{{labels}}} {} {observed_at_ms}",
            i64::from(capacity.accepting_seats),
        )
        .map_err(|_| ExpositionError)?;
        writeln!(
            seat_observation_samples,
            "cloud_fman_telemetry_seat_capacity_observation_timestamp_seconds{{{labels}}} {} {now_ms}",
            observed_at_ms as f64 / 1000.0,
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
    output.push_str("# HELP cloud_fman_telemetry_seat_capacity_max_seats Seats the MG offers at most, from its newest seat capacity event.\n");
    output.push_str("# TYPE cloud_fman_telemetry_seat_capacity_max_seats gauge\n");
    output.push_str(&max_seat_samples);
    output.push_str("# HELP cloud_fman_telemetry_seat_capacity_available_seats Seats the MG can still fill, from its newest seat capacity event.\n");
    output.push_str("# TYPE cloud_fman_telemetry_seat_capacity_available_seats gauge\n");
    output.push_str(&available_seat_samples);
    output.push_str("# HELP cloud_fman_telemetry_seat_capacity_accepting_seats Whether the MG accepts new seats, from its newest seat capacity event.\n");
    output.push_str("# TYPE cloud_fman_telemetry_seat_capacity_accepting_seats gauge\n");
    output.push_str(&accepting_seat_samples);
    output.push_str("# HELP cloud_fman_telemetry_seat_capacity_observation_timestamp_seconds Unix time the collector received the MG's newest seat capacity event.\n");
    output.push_str(
        "# TYPE cloud_fman_telemetry_seat_capacity_observation_timestamp_seconds gauge\n",
    );
    output.push_str(&seat_observation_samples);
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
