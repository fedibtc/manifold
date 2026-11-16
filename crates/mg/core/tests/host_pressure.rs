use std::time::{Duration, UNIX_EPOCH};

use fedi_decentralized_domain::{HostPressureLimit, PressureThresholds};

use super::*;

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_000_000_000 + secs)
}

/// Samples every 10 minutes from `from` to `to` seconds, with each counter
/// growing by its share of wall time (in per-mille) from `start` totals.
fn trace(
    from: u64,
    to: u64,
    cpu_permille: u64,
    io_permille: u64,
    start: PressureTotals,
) -> Vec<PressureSample> {
    (0..=(to - from) / (10 * MINUTE))
        .map(|n| {
            let elapsed_us = n * 10 * MINUTE * 1_000_000;
            PressureSample {
                at: at(from + n * 10 * MINUTE),
                totals: PressureTotals {
                    cpu_us: start.cpu_us + elapsed_us * cpu_permille / 1000,
                    io_us: start.io_us + elapsed_us * io_permille / 1000,
                },
            }
        })
        .collect()
}

const ZERO: PressureTotals = PressureTotals {
    cpu_us: 0,
    io_us: 0,
};

fn limit() -> HostPressureLimit {
    HostPressureLimit {
        cpu: PressureThresholds {
            block_above: 15,
            resume_below: 10,
        },
        io: PressureThresholds {
            block_above: 10,
            resume_below: 5,
        },
    }
}

fn pressure(cpu: (Option<u16>, Option<u16>), io: (Option<u16>, Option<u16>)) -> HostPressure {
    HostPressure {
        cpu: PressureWindows {
            six_hours: cpu.0,
            day: cpu.1,
        },
        io: PressureWindows {
            six_hours: io.0,
            day: io.1,
        },
    }
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

/// A window's average is exactly the stall time added over the wall time
/// elapsed between its samples, not a decaying average: a load that ended
/// leaves the 6-hour window entirely once 6 hours have passed.
#[test]
fn a_window_is_stall_time_over_wall_time() {
    // 20% CPU and 4% IO for a day.
    let day = trace(0, 24 * HOUR, 200, 40, ZERO);
    let now = at(24 * HOUR);
    assert_eq!(
        HostPressure::from_samples(&day, now),
        pressure((Some(200), Some(200)), (Some(40), Some(40)))
    );
    // Then 2% CPU for 6 hours: the 6-hour window sees only that; the day
    // window is 18 hours at 20% and 6 at 2%: 15.5%.
    let last = day.last().unwrap().totals;
    let mut samples = day;
    samples.extend(
        trace(24 * HOUR, 30 * HOUR, 20, 40, last)
            .into_iter()
            .skip(1),
    );
    let measured = HostPressure::from_samples(&samples, at(30 * HOUR));
    assert_eq!(
        measured.cpu,
        PressureWindows {
            six_hours: Some(20),
            day: Some(155)
        }
    );
    // Sample order does not matter.
    samples.reverse();
    assert_eq!(
        HostPressure::from_samples(&samples, at(30 * HOUR)),
        measured
    );
}

#[test]
fn a_window_needs_samples_over_half_of_it() {
    let samples = trace(0, 3 * HOUR - 10 * MINUTE, 500, 500, ZERO);
    let early = HostPressure::from_samples(&samples, at(3 * HOUR));
    assert_eq!(early, pressure((None, None), (None, None)));
    let samples = trace(0, 3 * HOUR, 500, 500, ZERO);
    let covered = HostPressure::from_samples(&samples, at(3 * HOUR));
    assert_eq!(covered, pressure((Some(500), None), (Some(500), None)));
    assert_eq!(
        HostPressure::from_samples(&[], at(0)),
        pressure((None, None), (None, None))
    );
}

/// A restart recreates the cgroup, so its counters start again near zero. The
/// pair across the reset says nothing about its interval and is skipped; the
/// pairs on both sides still count.
#[test]
fn a_counter_reset_after_restart_skips_only_the_pair_across_it() {
    let before = trace(
        0,
        4 * HOUR,
        300,
        100,
        PressureTotals {
            cpu_us: 9_000_000_000,
            io_us: 7_000_000_000,
        },
    );
    // Back after 10 minutes with counters from zero; one reading at the
    // reset itself.
    let after = trace(
        4 * HOUR + 10 * MINUTE,
        6 * HOUR,
        300,
        100,
        PressureTotals {
            cpu_us: 5,
            io_us: 5,
        },
    );
    let samples: Vec<_> = before.iter().chain(after.iter()).copied().collect();
    let measured = HostPressure::from_samples(&samples, at(6 * HOUR));
    // 5 h 50 min of pairs at 30% and 10%, nothing negative or inflated.
    assert_eq!(measured, pressure((Some(300), None), (Some(100), None)));
    // A reset of one counter only skips that counter's pair.
    let mut io_only = samples.clone();
    let reset = before.len();
    io_only[reset].totals.cpu_us =
        before[reset - 1].totals.cpu_us + 10 * MINUTE * 1_000_000 * 300 / 1000;
    for (n, sample) in io_only.iter_mut().enumerate().skip(reset + 1) {
        sample.totals.cpu_us = io_only_cpu(&before, n - reset);
    }
    assert_eq!(
        HostPressure::from_samples(&io_only, at(6 * HOUR))
            .io
            .six_hours,
        Some(100)
    );

    fn io_only_cpu(before: &[PressureSample], steps: usize) -> u64 {
        before.last().unwrap().totals.cpu_us
            + (steps as u64 + 1) * 10 * MINUTE * 1_000_000 * 300 / 1000
    }
}

#[test]
fn a_clock_set_back_or_a_repeated_time_adds_nothing() {
    let mut samples = trace(0, 4 * HOUR, 300, 100, ZERO);
    let last = *samples.last().unwrap();
    // The same time again with more stall: no wall time, so no average.
    samples.push(PressureSample {
        at: last.at,
        totals: PressureTotals {
            cpu_us: last.totals.cpu_us + 1_000_000_000,
            io_us: last.totals.io_us,
        },
    });
    assert_eq!(
        HostPressure::from_samples(&samples, at(4 * HOUR))
            .cpu
            .six_hours,
        Some(300)
    );
}

/// Both sides of each threshold, for each resource alone.
#[test]
fn either_six_hour_average_above_its_threshold_blocks() {
    let calm = (Some(0), Some(0));
    for (cpu, io, blocked) in [
        // At the CPU threshold (15%) does not block; just above does.
        ((Some(150), Some(0)), calm, false),
        ((Some(151), Some(0)), calm, true),
        // IO alone, with CPU calm.
        (calm, (Some(100), Some(0)), false),
        (calm, (Some(101), Some(0)), true),
        // One resource over is enough while the other is fine.
        ((Some(400), Some(0)), (Some(10), Some(0)), true),
        ((Some(10), Some(0)), (Some(400), Some(0)), true),
        // An uncovered window does not block.
        ((None, None), (None, None), false),
    ] {
        assert_eq!(
            holds(false, pressure(cpu, io), limit()),
            blocked,
            "{cpu:?} {io:?}"
        );
    }
}

#[test]
fn a_held_mg_resumes_only_when_both_day_averages_are_down() {
    for (cpu, io, still_held) in [
        // At both resume thresholds (10%, 5%) resumes; just above either
        // keeps holding, although both 6-hour averages are under their block
        // thresholds.
        ((Some(0), Some(100)), (Some(0), Some(50)), false),
        ((Some(0), Some(101)), (Some(0), Some(50)), true),
        ((Some(0), Some(100)), (Some(0), Some(51)), true),
        // An uncovered day window does not resume.
        ((Some(0), None), (Some(0), Some(0)), true),
        // A resource over its block threshold holds even with good days.
        ((Some(151), Some(0)), (Some(0), Some(0)), true),
    ] {
        assert_eq!(
            holds(true, pressure(cpu, io), limit()),
            still_held,
            "{cpu:?} {io:?}"
        );
    }
}

/// Driven through a trace, the check holds a host whose CPU pressure went
/// from 2% to 30% for 8 hours, keeps holding it after the 6-hour average is
/// back under 15%, and reopens once the day average is at 10%.
#[test]
fn a_pressured_host_reopens_only_after_the_day_average_falls() {
    let mut samples = trace(0, 24 * HOUR, 20, 0, ZERO);
    let mut totals = samples.last().unwrap().totals;
    let mut held = false;
    let (mut first_held, mut under_block, mut reopened) = (None, None, None);
    for n in 1..=(48 * HOUR / (10 * MINUTE)) {
        let secs = 24 * HOUR + n * 10 * MINUTE;
        let hours = (secs - 24 * HOUR) as f64 / HOUR as f64;
        let cpu_permille = if hours <= 8.0 { 300 } else { 20 };
        totals.cpu_us += 10 * MINUTE * 1_000_000 * cpu_permille / 1000;
        samples.push(PressureSample {
            at: at(secs),
            totals,
        });
        let measured = HostPressure::from_samples(&samples, at(secs));
        held = holds(held, measured, limit());
        if held {
            first_held.get_or_insert(hours);
        }
        if hours > 8.0 && measured.cpu.six_hours.is_some_and(|average| average <= 150) {
            under_block.get_or_insert(hours);
        }
        if first_held.is_some() && !held {
            reopened.get_or_insert(hours);
        }
    }
    let (first_held, under_block, reopened) =
        (first_held.unwrap(), under_block.unwrap(), reopened.unwrap());
    // The 6-hour average passes 15% once 13/28 of the window is at 30%:
    // about 2.8 hours in.
    assert!((2.5..=3.0).contains(&first_held), "{first_held}");
    // Back to 15% once at most 13/28 of the window is still the 8 high
    // hours: about 11.2 hours in.
    assert!((11.0..=11.5).contains(&under_block), "{under_block}");
    // The day average is 2% + 28%·(8 h / 24 h) ≈ 11.3% while all 8 hours are
    // in it, and reaches 10% once about 6.9 of them remain: about 25 hours in.
    assert!((24.5..=25.5).contains(&reopened), "{reopened}");
    assert!(under_block < reopened);
    assert!(!held, "the host stays open once it reopened");
}

#[test]
fn stores_one_sample_per_spacing_and_keeps_a_day() {
    let (store, before) = retention(None, at(HOUR));
    assert!(store);
    assert_eq!(before, at(HOUR) - RESUME_WINDOW);
    assert!(!retention(Some(at(0)), at(9 * MINUTE - 1)).0);
    assert!(retention(Some(at(0)), at(9 * MINUTE)).0);
    // A clock set back stores nothing.
    assert!(!retention(Some(at(HOUR)), at(0)).0);
}
