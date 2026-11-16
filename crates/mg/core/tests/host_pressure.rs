use std::time::{Duration, UNIX_EPOCH};

use super::*;

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const STEP: u64 = 10 * MINUTE;

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_000_000_000 + secs)
}

/// The maximums Fedi would first publish, which are also the defaults:
/// CPU 15% over 6 hours and 10% over 3.5 days, IO 25% and 15%.
fn max() -> HostPressure {
    HostPressure::from(MaxPressurePercent::default())
}

#[test]
fn parses_pressure_totals_not_averages() {
    let io = "some avg10=1.50 avg60=0.20 avg300=0.05 total=123456\n\
        full avg10=0.75 avg60=0.10 avg300=0.02 total=65432\n";
    assert_eq!(
        Pressure::parse(io),
        Some(Pressure {
            some_us: 123_456,
            full_us: Some(65_432),
        })
    );
    assert_eq!(
        Pressure::parse("some avg10=0.00 avg60=0.00 avg300=0.00 total=99\n"),
        Some(Pressure {
            some_us: 99,
            full_us: None,
        })
    );
    assert_eq!(Pressure::parse("full avg10=0.00 total=99\n"), None);
}

#[test]
fn finds_the_unified_cgroup_among_v1_lines() {
    let cgroup = "12:cpu,cpuacct:/legacy\n0::/system.slice/fedimint-mg.service\n";
    assert_eq!(
        cgroup_v2_dir(cgroup),
        Some(PathBuf::from(
            "/sys/fs/cgroup/system.slice/fedimint-mg.service"
        ))
    );
    assert_eq!(
        cgroup_v2_dir("0::/\n"),
        Some(PathBuf::from("/sys/fs/cgroup/"))
    );
    assert_eq!(cgroup_v2_dir("4:memory:/docker/abc\n"), None);
}

fn reading(secs: u64, cpu_us: u64, io_us: u64) -> PressureSample {
    PressureSample {
        at: at(secs),
        totals: PressureTotals { cpu_us, io_us },
    }
}

/// Counter growth for `secs` of wall time at `share` of it stalled.
fn stalled_us(secs: u64, share: f64) -> u64 {
    (secs as f64 * 1_000_000.0 * share).round() as u64
}

fn averages(cpu: (f64, f64), io: (f64, f64)) -> PressureAverages {
    PressureAverages {
        cpu: ResourceAverages {
            six_hours: cpu.0,
            three_and_a_half_days: cpu.1,
        },
        io: ResourceAverages {
            six_hours: io.0,
            three_and_a_half_days: io.1,
        },
        // Fully weighted: a long history, where the correction is a no-op.
        weights: AverageWeights {
            six_hours: 1.0,
            three_and_a_half_days: 1.0,
        },
    }
}

fn permille(cpu: (u16, u16), io: (u16, u16)) -> HostPressure {
    HostPressure {
        cpu: PressurePermille {
            six_hours: cpu.0,
            three_and_a_half_days: cpu.1,
        },
        io: PressurePermille {
            six_hours: io.0,
            three_and_a_half_days: io.1,
        },
    }
}

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-9
}

/// A host stalled at constant shares, read every [`STEP`].
struct Host {
    state: PressureState,
    secs: u64,
    cpu_us: u64,
    io_us: u64,
}

impl Host {
    fn at(averages: PressureAverages) -> Self {
        Self {
            state: PressureState {
                last: reading(0, 0, 0),
                averages: Some(averages),
            },
            secs: 0,
            cpu_us: 0,
            io_us: 0,
        }
    }

    /// One run after `elapsed` seconds at these shares.
    fn run_after(&mut self, elapsed: u64, cpu: f64, io: f64) -> PressureAverages {
        self.secs += elapsed;
        self.cpu_us += stalled_us(elapsed, cpu);
        self.io_us += stalled_us(elapsed, io);
        self.state = PressureState::record(
            Some(self.state),
            reading(self.secs, self.cpu_us, self.io_us),
        );
        self.state.averages.unwrap()
    }

    fn outcome(&self) -> ReadinessOutcome {
        HostPressure::from(self.state.averages.unwrap()).outcome(max())
    }
}

/// The first reading only starts the history; the first measured interval
/// sets both averages to its own shares rather than moving them from 0, so a
/// fresh install has measured pressure before anything can pass it.
#[test]
fn the_first_interval_seeds_both_averages() {
    let first = PressureState::record(None, reading(0, 7_000, 9_000));
    assert_eq!(first.averages, None);
    let second = PressureState::record(
        Some(first),
        reading(
            STEP,
            7_000 + stalled_us(STEP, 0.30),
            9_000 + stalled_us(STEP, 0.02),
        ),
    );
    let seeded = second.averages.unwrap();
    assert!(close(seeded.cpu.six_hours, 0.30) && close(seeded.cpu.three_and_a_half_days, 0.30));
    assert!(close(seeded.io.six_hours, 0.02) && close(seeded.io.three_and_a_half_days, 0.02));
    // 30% CPU is over the 15% maximum from the first interval on.
    assert_eq!(
        HostPressure::from(seeded).outcome(max()),
        ReadinessOutcome::SixHourPressureHigh
    );
}

/// Each interval moves an average `1 - exp(-elapsed / tau)` of the way to its
/// share, so one 30-minute interval weighs as much as the three 10-minute
/// ones it could have been.
#[test]
fn an_interval_weighs_by_its_length() {
    let start = averages((0.0, 0.0), (0.4, 0.4));
    let once = Host::at(start).run_after(30 * MINUTE, 0.5, 0.1);
    let weight = |tau_hours: f64| 1.0 - (-0.5f64 / tau_hours).exp();
    assert!(close(once.cpu.six_hours, 0.5 * weight(6.0)));
    assert!(close(once.cpu.three_and_a_half_days, 0.5 * weight(84.0)));
    assert!(close(once.io.six_hours, 0.4 - 0.3 * weight(6.0)));
    let mut host = Host::at(start);
    for _ in 0..3 {
        host.run_after(STEP, 0.5, 0.1);
    }
    let steps = host.state.averages.unwrap();
    assert!((steps.cpu.six_hours - once.cpu.six_hours).abs() < 1e-6);
    assert!((steps.cpu.three_and_a_half_days - once.cpu.three_and_a_half_days).abs() < 1e-6);
    assert!((steps.io.six_hours - once.io.six_hours).abs() < 1e-6);
}

/// An interval longer than 30 minutes spans downtime, such as a daemon
/// restarted inside a live cgroup, and leaves the averages as they were
/// rather than counting the downtime as no pressure; the next interval is
/// measured from the new counters.
#[test]
fn a_gap_longer_than_thirty_minutes_is_skipped() {
    let before = averages((0.12, 0.12), (0.03, 0.03));
    let mut host = Host::at(before);
    assert_eq!(host.run_after(12 * HOUR, 0.0, 0.0), before);
    assert_eq!(host.run_after(30 * MINUTE + 1, 0.0, 0.0), before);
    let weight = 1.0 - (-(STEP as f64) / (6.0 * 3600.0)).exp();
    let next = host.run_after(STEP, 1.0, 0.0);
    assert!(close(next.cpu.six_hours, 0.12 + weight * 0.88));
    assert!(close(next.io.six_hours, 0.03 - weight * 0.03));
    // Exactly 30 minutes still counts.
    let mut host = Host::at(before);
    assert_ne!(host.run_after(30 * MINUTE, 0.0, 0.0), before);
}

/// A counter that went backwards, as when a restart recreates the cgroup,
/// or a clock that did not advance, leaves the averages as they were; the
/// next interval is measured from the new counters.
#[test]
fn a_counter_reset_or_a_stuck_clock_is_skipped() {
    let before = averages((0.12, 0.12), (0.03, 0.03));
    let start = PressureState {
        last: reading(0, 50_000_000_000, 8_000_000_000),
        averages: Some(before),
    };
    // After a restart the CPU counter restarted near zero; the IO counter,
    // here, did not. Either going back skips the interval for both.
    let reset_reading = reading(STEP, 1_000, 8_000_000_000 + stalled_us(STEP, 0.5));
    let reset = PressureState::record(Some(start), reset_reading);
    assert_eq!(reset.averages, Some(before));
    assert_eq!(reset.last, reset_reading);
    // The clock set back, and then a repeated time.
    for secs in [STEP - 1, STEP] {
        let skipped = PressureState::record(Some(reset), reading(secs, 2_000, 9_000_000_000_000));
        assert_eq!(skipped.averages, Some(before), "at {secs}");
    }
    // The next interval counts from the counters after the reset.
    let next = PressureState::record(
        Some(reset),
        reading(
            2 * STEP,
            1_000 + stalled_us(STEP, 1.0),
            8_000_000_000 + stalled_us(STEP, 0.5),
        ),
    )
    .averages
    .unwrap();
    let weight = 1.0 - (-(STEP as f64) / (6.0 * 3600.0)).exp();
    assert!(close(next.cpu.six_hours, 0.12 + weight * 0.88));
    assert!(close(next.io.six_hours, 0.03 - weight * 0.03));
}

/// Every window passes at its maximum and fails 0.1% above it, alone, with
/// every other window idle; the outcome names the shorter failing window.
#[test]
fn each_window_fails_just_above_its_maximum() {
    let max = max();
    assert_eq!(
        permille((150, 100), (250, 150)).outcome(max),
        ReadinessOutcome::Pass
    );
    for (pressure, outcome) in [
        (
            permille((151, 0), (0, 0)),
            ReadinessOutcome::SixHourPressureHigh,
        ),
        (
            permille((0, 101), (0, 0)),
            ReadinessOutcome::HalfWeekPressureHigh,
        ),
        (
            permille((0, 0), (251, 0)),
            ReadinessOutcome::SixHourPressureHigh,
        ),
        (
            permille((0, 0), (0, 151)),
            ReadinessOutcome::HalfWeekPressureHigh,
        ),
        // Both windows over: the shorter is named.
        (
            permille((0, 101), (251, 0)),
            ReadinessOutcome::SixHourPressureHigh,
        ),
    ] {
        assert_eq!(pressure.outcome(max), outcome, "{pressure:?}");
    }
    // Rounding to 0.1% decides: 15.049% compares as 15.0%.
    let just_under = HostPressure::from(averages((0.15049, 0.0), (0.0, 0.0)));
    assert_eq!(just_under.outcome(max), ReadinessOutcome::Pass);
}

/// A host steady at 12% CPU passes the 15% 6-hour maximum but fails the 10%
/// 3.5-day one, however long it stays there.
#[test]
fn a_host_steady_at_twelve_percent_fails_the_long_window() {
    let mut host = Host::at(averages((0.12, 0.12), (0.01, 0.01)));
    for _ in 0..(7 * 24 * 6) {
        host.run_after(STEP, 0.12, 0.01);
        assert_eq!(host.outcome(), ReadinessOutcome::HalfWeekPressureHigh);
    }
}

/// A single spike of 6 hours at 40% on a 2% baseline fails the 6-hour
/// window while its average is over 15%, but barely moves the 3.5-day one:
/// 2% + 38% * (1 - e^(-6/84)) = 4.6%. So the host passes again as soon as
/// the 6-hour average is back at 15%, which from 2% + 38% * (1 - e^-1) =
/// 26.0% at the spike's end takes 6h * ln(24.0 / 13.05) = 3.66 hours.
#[test]
fn a_spike_fails_only_while_the_six_hour_average_is_over() {
    let mut host = Host::at(averages((0.02, 0.02), (0.0, 0.0)));
    let mut first_failure = None;
    for n in 1..=36 {
        let averages = host.run_after(STEP, 0.40, 0.0);
        let over = averages.cpu.six_hours >= 0.1505;
        assert_eq!(
            host.outcome(),
            if over {
                ReadinessOutcome::SixHourPressureHigh
            } else {
                ReadinessOutcome::Pass
            },
            "run {n}"
        );
        if over && first_failure.is_none() {
            first_failure = Some(n);
        }
    }
    // 2% + 38% * (1 - e^(-t/6h)) reaches 15.05% after 6h * ln(38/24.95) =
    // 2.52 hours: the 16th run.
    assert_eq!(first_failure, Some(16));
    let at_end = host.state.averages.unwrap().cpu;
    assert!(close(
        at_end.six_hours,
        0.02 + 0.38 * (1.0 - (-1.0f64).exp())
    ));
    assert!(close(
        at_end.three_and_a_half_days,
        0.02 + 0.38 * (1.0 - (-6.0f64 / 84.0).exp())
    ));
    assert!(at_end.three_and_a_half_days < 0.10);

    let expected = 6.0 * 3600.0 * ((at_end.six_hours - 0.02) / (0.1505 - 0.02)).ln();
    let mut after = 0;
    while host.outcome() != ReadinessOutcome::Pass {
        host.run_after(STEP, 0.02, 0.0);
        after += STEP;
    }
    assert!(
        (after as f64) >= expected && (after as f64) < expected + STEP as f64,
        "{after} vs {expected}"
    );
    assert!(after > 3 * HOUR && after < 4 * HOUR);
}

/// Bias correction: right after an upgrade the first interval is the busy
/// minute while the seats start. Without correction it would seed the
/// 3.5-day average at 40%, which takes days to decay under 10%; corrected,
/// an hour at 2% brings both averages to about the time-weighted mean,
/// (1 min * 40% + 60 min * 2%) / 61 min = 2.6%.
#[test]
fn a_busy_first_minute_does_not_close_the_host_for_days() {
    let mut state = PressureState::record(None, reading(0, 0, 0));
    let (mut secs, mut cpu_us) = (MINUTE, stalled_us(MINUTE, 0.40));
    state = PressureState::record(Some(state), reading(secs, cpu_us, 0));
    let first = state.averages.unwrap();
    assert!(close(first.cpu.three_and_a_half_days, 0.40));
    assert_eq!(
        HostPressure::from(first).outcome(max()),
        ReadinessOutcome::SixHourPressureHigh
    );
    for _ in 0..6 {
        secs += STEP;
        cpu_us += stalled_us(STEP, 0.02);
        state = PressureState::record(Some(state), reading(secs, cpu_us, 0));
    }
    let after = state.averages.unwrap().cpu;
    let mean = (40.0 + 60.0 * 2.0) / 61.0 / 100.0;
    // The weights favour recent minutes slightly, so a little under the mean.
    assert!(
        after.six_hours < mean && after.six_hours > 0.025,
        "{after:?}"
    );
    assert!(
        after.three_and_a_half_days < mean && after.three_and_a_half_days > 0.025,
        "{after:?}"
    );
    assert_eq!(
        HostPressure::from(state.averages.unwrap()).outcome(max()),
        ReadinessOutcome::Pass
    );
}

/// The correction does not excuse a host that is busy from the start: at 20%
/// CPU stall for its first hour, every run fails the 15% 6-hour maximum.
#[test]
fn a_host_busy_from_the_start_fails_the_six_hour_window() {
    let mut state = PressureState::record(None, reading(0, 0, 0));
    let (mut secs, mut cpu_us) = (0, 0);
    for interval in [MINUTE].into_iter().chain([STEP; 6]) {
        secs += interval;
        cpu_us += stalled_us(interval, 0.20);
        state = PressureState::record(Some(state), reading(secs, cpu_us, 0));
        let averages = state.averages.unwrap();
        assert!(close(averages.cpu.six_hours, 0.20), "{averages:?}");
        assert_eq!(
            HostPressure::from(averages).outcome(max()),
            ReadinessOutcome::SixHourPressureHigh,
            "after {secs} s"
        );
    }
}
