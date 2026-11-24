//! Consensus ordering latency of this MG's seats, as the readiness ordering
//! latency check compares it ([`crate::seat_readiness`],
//! `crates/mg/specs/SPEC-advertisement.md`).
//!
//! fedimintd observes, for each batch of its own consensus items, how long
//! AlephBFT took to order it, in the `fm_consensus_ordering_latency_seconds`
//! histogram on its loopback metrics port. A guardian on a slow disk orders
//! its items late, whatever the cause of the slowness, so its federations go
//! ahead without it. Each run takes, per running seat, the mean latency of
//! the items ordered since that seat's previous reading in this process, and
//! the median over seats: one seat in a slow federation does not close the
//! MG, while a slow host is slow in all of them. It folds that median into a
//! bias-corrected, time-weighted exponential moving average exactly as the
//! pressure averages do ([`crate::host_pressure`]).

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use fedi_decentralized_service_manifold_guardian::SeatId;

use crate::host_pressure::{MAX_INTERVAL, SIX_HOURS_TAU};
use crate::seat_readiness::ReadinessOutcome;

/// The histogram fedimintd observes each ordered batch of its own items in.
const FAMILY: &str = "fm_consensus_ordering_latency_seconds";

/// The highest 6-hour average ordering latency at which an MG sells seats.
/// Healthy MGs order their items in 1-2 s; MGs on contended disks take 5 s
/// and more.
pub const MAX_ORDERING_LATENCY_MS: u32 = 5_000;

/// The cumulative sum and count of a seat's ordering latency histogram,
/// added over its label sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrderingTotals {
    pub sum_secs: f64,
    pub count: f64,
}

impl OrderingTotals {
    /// Read the histogram's `_sum` and `_count` samples from a Prometheus
    /// text exposition, adding every label set. `None` when either is
    /// missing, or a value is not a finite, non-negative number.
    pub fn parse(exposition: &str) -> Option<Self> {
        let (sum_name, count_name) = (format!("{FAMILY}_sum"), format!("{FAMILY}_count"));
        let (mut sum, mut count) = (None::<f64>, None::<f64>);
        for line in exposition.lines() {
            let line = line.trim_start();
            if line.starts_with('#') {
                continue;
            }
            let name_end = line
                .find(|character: char| character == '{' || character.is_ascii_whitespace())
                .unwrap_or(line.len());
            let total = match &line[..name_end] {
                name if name == sum_name => &mut sum,
                name if name == count_name => &mut count,
                _ => continue,
            };
            let value: f64 = sample_value(&line[name_end..])?.parse().ok()?;
            if !value.is_finite() || value < 0.0 {
                return None;
            }
            *total = Some(total.unwrap_or(0.0) + value);
        }
        Some(Self {
            sum_secs: sum?,
            count: count?,
        })
    }
}

/// The value of a sample from what follows its name: an optional label set,
/// whose quoted values may hold spaces and braces, then the value and an
/// optional timestamp.
fn sample_value(rest: &str) -> Option<&str> {
    let rest = match rest.strip_prefix('{') {
        None => rest,
        Some(labels) => {
            let (mut quoted, mut escaped) = (false, false);
            let end = labels.char_indices().find_map(|(index, character)| {
                match character {
                    _ if escaped => escaped = false,
                    '\\' if quoted => escaped = true,
                    '"' => quoted = !quoted,
                    '}' if !quoted => return Some(index),
                    _ => {}
                }
                None
            })?;
            &labels[end + 1..]
        }
    };
    rest.split_ascii_whitespace().next()
}

/// The mean ordering latency of the items a seat ordered from `earlier` to
/// `later`, in seconds, or `None` when the interval says nothing: no item
/// was ordered, or a counter went backwards, as it does when the seat
/// restarts.
fn interval_mean(earlier: OrderingTotals, later: OrderingTotals) -> Option<f64> {
    let count = later.count - earlier.count;
    let sum = later.sum_secs - earlier.sum_secs;
    (count > 0.0 && sum >= 0.0).then(|| sum / count)
}

/// The median of `values`, the mean of the middle two for an even count.
fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    match values.len() {
        0 => None,
        len if len % 2 == 1 => Some(values[middle]),
        _ => Some((values[middle - 1] + values[middle]) / 2.0),
    }
}

/// Each running seat's last reading in this process. A seat's first reading
/// after it starts running, or after this daemon starts, only sets its
/// baseline.
#[derive(Debug, Default)]
pub struct SeatBaselines(HashMap<SeatId, OrderingTotals>);

impl SeatBaselines {
    /// Account for this run's `readings`, one per running seat, `None` for a
    /// seat whose metrics could not be read, and return the median over seats
    /// of the mean latency since each one's previous reading, or `None` when
    /// no seat's interval says anything. Each reading replaces its seat's
    /// baseline, and a seat that is not running or could not be read loses
    /// its baseline.
    pub fn interval_median(
        &mut self,
        readings: &[(SeatId, Option<OrderingTotals>)],
    ) -> Option<f64> {
        let previous = std::mem::take(&mut self.0);
        let mut means = Vec::new();
        for (seat, reading) in readings {
            let Some(reading) = *reading else {
                continue;
            };
            if let Some(mean) = previous
                .get(seat)
                .and_then(|earlier| interval_mean(*earlier, reading))
            {
                means.push(mean);
            }
            self.0.insert(seat.clone(), reading);
        }
        median(means)
    }
}

/// The bias-corrected 6-hour average of the run medians, in seconds, and the
/// weight `1 - exp(-measured / tau)` the measured intervals carry, between 0
/// and 1 ([`crate::host_pressure::AverageWeights`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrderingLatencyAverage {
    pub six_hours_secs: f64,
    pub weight: f64,
}

impl OrderingLatencyAverage {
    /// Fold in one interval's median `latency_secs` over `elapsed`, as
    /// [`crate::host_pressure`] folds an interval's stall share.
    fn fold(previous: Option<Self>, latency_secs: f64, elapsed: Duration) -> Self {
        let previous = previous.unwrap_or(Self {
            six_hours_secs: 0.0,
            weight: 0.0,
        });
        let gain = 1.0 - (-elapsed.as_secs_f64() / SIX_HOURS_TAU.as_secs_f64()).exp();
        let weight = previous.weight * (1.0 - gain) + gain;
        Self {
            six_hours_secs: previous.six_hours_secs
                + gain / weight * (latency_secs - previous.six_hours_secs),
            weight,
        }
    }

    /// The average as reports state it and the check compares it, in whole
    /// milliseconds.
    pub fn millis(self) -> u32 {
        (self.six_hours_secs.max(0.0) * 1000.0)
            .round()
            .min(f64::from(u32::MAX)) as u32
    }
}

/// The durable ordering latency history: when this MG last read its seats,
/// and the average over the intervals measured since the first reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrderingLatencyState {
    pub sampled_at: SystemTime,
    /// `None` until an interval has been measured.
    pub average: Option<OrderingLatencyAverage>,
}

impl OrderingLatencyState {
    /// Account for a run that read its running seats at `at`, with the median
    /// of their interval means, `None` when no seat's interval said anything.
    /// The interval since the previous run says nothing, and leaves the
    /// average unchanged, without a median; when the clock did not advance;
    /// or when it is longer than [`MAX_INTERVAL`], since the seats' baselines
    /// are then older than the runs that should have read them.
    pub fn record(previous: Option<Self>, at: SystemTime, median_secs: Option<f64>) -> Self {
        let average = previous.and_then(|previous| previous.average);
        let elapsed = previous
            .and_then(|previous| at.duration_since(previous.sampled_at).ok())
            .filter(|elapsed| !elapsed.is_zero() && *elapsed <= MAX_INTERVAL);
        Self {
            sampled_at: at,
            average: match (median_secs, elapsed) {
                (Some(latency), Some(elapsed)) => {
                    Some(OrderingLatencyAverage::fold(average, latency, elapsed))
                }
                _ => average,
            },
        }
    }
}

/// The ordering latency check for one run, from whether any seat is running
/// and, when one is, recording the run (the history after it). Without
/// running seats the check does not apply. A history that cannot be stored,
/// or no interval measured yet, fails the check as still measuring. Returns
/// the outcome and the average to report, in milliseconds.
pub fn outcome(
    recorded: Option<anyhow::Result<OrderingLatencyState>>,
) -> (ReadinessOutcome, Option<u32>) {
    let Some(recorded) = recorded else {
        return (ReadinessOutcome::NotApplicable, None);
    };
    match recorded
        .ok()
        .and_then(|state| state.average)
        .map(OrderingLatencyAverage::millis)
    {
        None => (ReadinessOutcome::OrderingLatencyMeasuring, None),
        Some(millis) if millis > MAX_ORDERING_LATENCY_MS => {
            (ReadinessOutcome::OrderingLatencyHigh, Some(millis))
        }
        Some(millis) => (ReadinessOutcome::Pass, Some(millis)),
    }
}

#[cfg(test)]
#[path = "../tests/ordering_latency.rs"]
mod tests;
