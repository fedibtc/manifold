use std::time::{Duration, UNIX_EPOCH};

use super::*;

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_000_000_000 + secs)
}

fn seat(byte: u8) -> SeatId {
    SeatId::new(hex::encode([byte; 32])).unwrap()
}

fn totals(sum_secs: f64, count: f64) -> OrderingTotals {
    OrderingTotals { sum_secs, count }
}

/// fedimintd's own exposition: no labels, buckets beside the totals.
#[test]
fn parses_the_raw_histogram_totals() {
    let exposition = "# HELP fm_consensus_ordering_latency_seconds Duration of ordering a batch of consensus items\n\
        # TYPE fm_consensus_ordering_latency_seconds histogram\n\
        fm_consensus_ordering_latency_seconds_bucket{le=\"1\"} 40\n\
        fm_consensus_ordering_latency_seconds_bucket{le=\"+Inf\"} 52\n\
        fm_consensus_ordering_latency_seconds_sum 93.5\n\
        fm_consensus_ordering_latency_seconds_count 52\n\
        fm_consensus_session_count 7\n";
    assert_eq!(OrderingTotals::parse(exposition), Some(totals(93.5, 52.0)));
}

/// Several label sets add up, whatever the labels hold; a sample may carry a
/// timestamp, and families that only share a prefix do not count.
#[test]
fn adds_every_label_set() {
    let exposition = "fm_consensus_ordering_latency_seconds_sum{peer=\"a b}\",x=\"q\\\"}\"} 10 1700000000000\n\
        fm_consensus_ordering_latency_seconds_count{peer=\"a b}\",x=\"q\\\"}\"} 4 1700000000000\n\
        fm_consensus_ordering_latency_seconds_sum{peer=\"c\"} 2.5\n\
        fm_consensus_ordering_latency_seconds_count{peer=\"c\"} 1\n\
        fm_consensus_ordering_latency_seconds_sum_total 1000\n\
        fm_consensus_ordering_latency_seconds_counts 1000\n\
        other_fm_consensus_ordering_latency_seconds_sum 1000\n";
    assert_eq!(OrderingTotals::parse(exposition), Some(totals(12.5, 5.0)));
}

#[test]
fn a_missing_family_or_a_bad_value_reads_as_nothing() {
    assert_eq!(OrderingTotals::parse(""), None);
    assert_eq!(
        OrderingTotals::parse(
            "fm_consensus_session_count 7\nfm_consensus_ordering_latency_seconds_bucket{le=\"+Inf\"} 3\n"
        ),
        None
    );
    // Both totals are needed.
    assert_eq!(
        OrderingTotals::parse("fm_consensus_ordering_latency_seconds_sum 3\n"),
        None
    );
    assert_eq!(
        OrderingTotals::parse("fm_consensus_ordering_latency_seconds_count 3\n"),
        None
    );
    for value in ["NaN", "+Inf", "-1", "x"] {
        assert_eq!(
            OrderingTotals::parse(&format!(
                "fm_consensus_ordering_latency_seconds_sum {value}\nfm_consensus_ordering_latency_seconds_count 3\n"
            )),
            None,
            "{value}"
        );
    }
}

/// A seat's first reading only sets its baseline; later ones give the mean
/// of the items ordered in between.
#[test]
fn the_interval_mean_is_the_added_sum_over_the_added_count() {
    let mut baselines = SeatBaselines::default();
    assert_eq!(
        baselines.interval_median(&[(seat(1), Some(totals(100.0, 50.0)))]),
        None
    );
    assert_eq!(
        baselines.interval_median(&[(seat(1), Some(totals(130.0, 60.0)))]),
        Some(3.0)
    );
}

/// A restarted seat starts its counters again. Neither a fall in both nor in
/// the sum alone yields a mean: the reading becomes the new baseline.
#[test]
fn a_counter_reset_is_a_fresh_baseline() {
    let mut baselines = SeatBaselines::default();
    baselines.interval_median(&[(seat(1), Some(totals(100.0, 50.0)))]);
    assert_eq!(
        baselines.interval_median(&[(seat(1), Some(totals(2.0, 1.0)))]),
        None
    );
    assert_eq!(
        baselines.interval_median(&[(seat(1), Some(totals(6.0, 3.0)))]),
        Some(2.0)
    );
    // The count went on rising past its old value but the sum fell: reset.
    assert_eq!(
        baselines.interval_median(&[(seat(1), Some(totals(1.0, 10.0)))]),
        None
    );
    assert_eq!(
        baselines.interval_median(&[(seat(1), Some(totals(3.0, 11.0)))]),
        Some(2.0)
    );
}

/// A seat that ordered nothing, could not be read, or stopped running says
/// nothing; an unreadable or stopped seat starts its baseline again.
#[test]
fn a_seat_without_items_or_metrics_says_nothing() {
    let mut baselines = SeatBaselines::default();
    baselines.interval_median(&[
        (seat(1), Some(totals(10.0, 10.0))),
        (seat(2), Some(totals(10.0, 10.0))),
    ]);
    assert_eq!(
        baselines.interval_median(&[(seat(1), Some(totals(10.0, 10.0))), (seat(2), None)]),
        None
    );
    // Seat 2 was unreadable, so its next reading is a baseline again.
    assert_eq!(
        baselines.interval_median(&[
            (seat(1), Some(totals(14.0, 12.0))),
            (seat(2), Some(totals(500.0, 20.0))),
        ]),
        Some(2.0)
    );
    // Seat 1 stopped running for a run.
    baselines.interval_median(&[(seat(2), Some(totals(520.0, 30.0)))]);
    assert_eq!(
        baselines.interval_median(&[
            (seat(1), Some(totals(400.0, 13.0))),
            (seat(2), Some(totals(530.0, 40.0))),
        ]),
        Some(1.0)
    );
}

/// One seat in a slow federation does not move the run's value: the median
/// of 1 s, 1 s and 40 s is 1 s, where the mean would be 14 s. An even number
/// of seats takes the middle two's mean.
#[test]
fn the_run_takes_the_median_seat() {
    let mut baselines = SeatBaselines::default();
    let three = |sums: [f64; 3], count: f64| {
        [1, 2, 3]
            .into_iter()
            .zip(sums)
            .map(|(byte, sum)| (seat(byte), Some(totals(sum, count))))
            .collect::<Vec<_>>()
    };
    baselines.interval_median(&three([0.0, 0.0, 0.0], 0.0));
    assert_eq!(
        baselines.interval_median(&three([10.0, 400.0, 10.0], 10.0)),
        Some(1.0)
    );
    let mut baselines = SeatBaselines::default();
    let two = |sums: [f64; 2], count: f64| {
        [1, 2]
            .into_iter()
            .zip(sums)
            .map(|(byte, sum)| (seat(byte), Some(totals(sum, count))))
            .collect::<Vec<_>>()
    };
    baselines.interval_median(&two([0.0, 0.0], 0.0));
    assert_eq!(
        baselines.interval_median(&two([10.0, 30.0], 10.0)),
        Some(2.0)
    );
}

/// A history read every 10 minutes at `secs` per item, `runs` times after
/// its first reading at 0.
fn steady(secs: f64, runs: u64) -> OrderingLatencyState {
    let mut state = OrderingLatencyState::record(None, at(0), None);
    for run in 1..=runs {
        state = OrderingLatencyState::record(Some(state), at(run * 10 * MINUTE), Some(secs));
    }
    state
}

/// The bound holds on both sides of 5 s through the average, and a figure
/// equal to it passes.
#[test]
fn the_average_compares_against_five_seconds() {
    for (secs, expected) in [
        (4.9, ReadinessOutcome::Pass),
        (5.0, ReadinessOutcome::Pass),
        (5.1, ReadinessOutcome::OrderingLatencyHigh),
    ] {
        let state = steady(secs, 36);
        let millis = (secs * 1000.0).round() as u32;
        assert_eq!(outcome(Some(Ok(state))), (expected, Some(millis)), "{secs}");
    }
}

/// The first interval sets the average to its own median, and later ones
/// fold in with their share of the time: the average is time-weighted.
#[test]
fn the_average_is_bias_corrected_and_time_weighted() {
    let first = OrderingLatencyState::record(Some(steady(0.0, 0)), at(10 * MINUTE), Some(8.0));
    assert_eq!(first.average.unwrap().six_hours_secs, 8.0);
    // Twenty minutes at 2 s after ten at 8 s: about the plain time-weighted
    // mean of 4 s, a little under it as the newer interval weighs more.
    let second = OrderingLatencyState::record(Some(first), at(30 * MINUTE), Some(2.0));
    let average = second.average.unwrap();
    let tau = SIX_HOURS_TAU.as_secs_f64();
    let gain = |minutes: f64| 1.0 - (-minutes * 60.0 / tau).exp();
    let weight = gain(10.0) * (1.0 - gain(20.0)) + gain(20.0);
    let expected = 8.0 + gain(20.0) / weight * (2.0 - 8.0);
    assert!(
        (average.six_hours_secs - expected).abs() < 1e-12,
        "{average:?}"
    );
    assert!((average.weight - weight).abs() < 1e-12, "{average:?}");
    assert!((3.9..4.0).contains(&average.six_hours_secs), "{average:?}");
}

/// A day at 1 s, then one 10-minute interval at 40 s. The day's history
/// weighs w = 1 - e^(-4), and the interval g = 1 - e^(-1/36), so the 6-hour
/// average moves 39 s * g / (w (1 - g) + g), to about 2.09 s, and the MG
/// stays open.
#[test]
fn a_single_slow_interval_does_not_close_a_healthy_mg() {
    let healthy = steady(1.0, 24 * 6);
    let spike = OrderingLatencyState::record(
        Some(healthy),
        healthy.sampled_at + Duration::from_secs(10 * MINUTE),
        Some(40.0),
    );
    let millis = spike.average.unwrap().millis();
    assert!((2080..=2095).contains(&millis), "{millis}");
    assert_eq!(outcome(Some(Ok(spike))).0, ReadinessOutcome::Pass);
    // Six hours at 10 s close it.
    let mut slow = spike;
    for _ in 0..36 {
        slow = OrderingLatencyState::record(
            Some(slow),
            slow.sampled_at + Duration::from_secs(10 * MINUTE),
            Some(10.0),
        );
    }
    assert_eq!(
        outcome(Some(Ok(slow))).0,
        ReadinessOutcome::OrderingLatencyHigh
    );
}

/// No median, a clock that did not advance, an interval longer than
/// [`MAX_INTERVAL`] or no previous reading leave the average unchanged;
/// the reading time always moves on.
#[test]
fn an_interval_that_says_nothing_leaves_the_average() {
    let state = steady(3.0, 3);
    let average = state.average;
    let later = state.sampled_at + Duration::from_secs(10 * MINUTE);
    for next in [
        OrderingLatencyState::record(Some(state), later, None),
        OrderingLatencyState::record(Some(state), state.sampled_at, Some(40.0)),
        OrderingLatencyState::record(
            Some(state),
            state.sampled_at + MAX_INTERVAL + Duration::from_secs(1),
            Some(40.0),
        ),
    ] {
        assert_eq!(next.average, average);
    }
    assert_eq!(
        OrderingLatencyState::record(Some(state), later, None).sampled_at,
        later
    );
    assert_eq!(
        OrderingLatencyState::record(None, at(HOUR), Some(40.0)),
        OrderingLatencyState {
            sampled_at: at(HOUR),
            average: None,
        }
    );
}

/// Without running seats the check does not apply; with them it fails
/// until an interval is measured, and when the history cannot be stored.
#[test]
fn the_check_applies_only_to_running_seats_and_fails_while_measuring() {
    assert_eq!(outcome(None), (ReadinessOutcome::NotApplicable, None));
    assert_eq!(
        outcome(Some(Ok(steady(1.0, 0)))),
        (ReadinessOutcome::OrderingLatencyMeasuring, None)
    );
    assert_eq!(
        outcome(Some(Err(anyhow::anyhow!("database is locked")))),
        (ReadinessOutcome::OrderingLatencyMeasuring, None)
    );
    assert!(!ReadinessOutcome::OrderingLatencyMeasuring.passed());
    assert!(!ReadinessOutcome::OrderingLatencyHigh.passed());
}
