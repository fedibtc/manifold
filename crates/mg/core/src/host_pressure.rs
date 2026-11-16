//! CPU and IO pressure on the cgroup this MG shares with its seats, as the
//! readiness pressure check compares it
//! ([`crate::seat_readiness`], `crates/mg/specs/SPEC-advertisement.md`).
//!
//! The kernel's pressure stall information (PSI) counts, per cgroup, the
//! microseconds in which at least one task stalled waiting for a CPU or for
//! storage IO. The counters are cumulative, so the share of an interval spent
//! stalled is the stall time added over the wall time elapsed. Each run folds
//! that share into exponential moving averages, like the uptime scores
//! ([`crate::seat_readiness::UptimeScores`]). Unlike the system load average,
//! the share of time stalled does not depend on how fast or how many the CPUs
//! are.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context as _;
use fedi_decentralized_domain::{MaxPressurePercent, MaxPressureWindows};

use crate::seat_readiness::ReadinessOutcome;

/// The time constant of the short averages, which close an MG for the hours
/// a spike lasts.
pub const SIX_HOURS_TAU: Duration = Duration::from_secs(6 * 60 * 60);

/// The time constant of the long averages, 3.5 days, which close an MG whose
/// pressure stays high for days and barely move for a single spike.
pub const HALF_WEEK_TAU: Duration = Duration::from_secs(84 * 60 * 60);

/// The longest interval folded into the averages. Runs come every 10
/// minutes, or every minute while a check fails, so a longer interval spans
/// downtime: a daemon restarted inside a live cgroup would otherwise count
/// the time it was down as time without pressure.
pub const MAX_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Cumulative stall time from a pressure stall information file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pressure {
    /// Time in which at least one task stalled on the resource.
    pub some_us: u64,
    /// Time in which all non-idle tasks stalled. CPU pressure has no
    /// meaningful `full` line.
    pub full_us: Option<u64>,
}

impl Pressure {
    /// Parse a PSI file such as `cpu.pressure` or `io.pressure`: the `total=`
    /// counters, not the kernel's decaying averages.
    pub fn parse(pressure: &str) -> Option<Self> {
        let total = |kind: &str| {
            pressure
                .lines()
                .find_map(|line| line.strip_prefix(kind)?.strip_prefix(' '))?
                .split_ascii_whitespace()
                .find_map(|pair| pair.strip_prefix("total="))?
                .parse()
                .ok()
        };
        Some(Self {
            some_us: total("some")?,
            full_us: total("full"),
        })
    }
}

/// The cgroup v2 directory of this process, from `/proc/<pid>/cgroup`. A
/// container with its own cgroup namespace sees its own cgroup as `/`.
pub fn cgroup_v2_dir(cgroup: &str) -> Option<PathBuf> {
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    Some(Path::new("/sys/fs/cgroup").join(path.trim().trim_start_matches('/')))
}

/// One reading of the `some` counters of this process's cgroup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PressureTotals {
    pub cpu_us: u64,
    pub io_us: u64,
}

/// Read the `some` stall counters of the cgroup this process runs in, which
/// holds the MG daemon and its seats.
pub fn read_cgroup_pressure() -> anyhow::Result<PressureTotals> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").context("read /proc/self/cgroup")?;
    let dir = cgroup_v2_dir(&cgroup).context("this process is in no cgroup v2")?;
    let read = |name: &str| -> anyhow::Result<u64> {
        let text = std::fs::read_to_string(dir.join(name))
            .with_context(|| format!("read the cgroup's {name}"))?;
        Ok(Pressure::parse(&text)
            .with_context(|| format!("the cgroup's {name} has no some total"))?
            .some_us)
    };
    Ok(PressureTotals {
        cpu_us: read("cpu.pressure")?,
        io_us: read("io.pressure")?,
    })
}

/// One reading with the wall time it was taken at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PressureSample {
    pub at: SystemTime,
    pub totals: PressureTotals,
}

/// One resource's stall share averages, each between 0 and 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResourceAverages {
    /// Exponential moving average with time constant [`SIX_HOURS_TAU`].
    pub six_hours: f64,
    /// Exponential moving average with time constant [`HALF_WEEK_TAU`].
    pub three_and_a_half_days: f64,
}

impl ResourceAverages {
    /// The averages of a first measured interval: its share, so nothing is
    /// assumed about a host before it was measured.
    fn seed(share: f64) -> Self {
        Self {
            six_hours: share,
            three_and_a_half_days: share,
        }
    }

    /// Fold one interval of `elapsed` in: an average moves
    /// `1 - exp(-elapsed / tau)` of the way to the interval's share.
    fn fold(self, share: f64, elapsed: Duration) -> Self {
        let toward = |average: f64, tau: Duration| {
            let weight = 1.0 - (-elapsed.as_secs_f64() / tau.as_secs_f64()).exp();
            average + weight * (share - average)
        };
        Self {
            six_hours: toward(self.six_hours, SIX_HOURS_TAU),
            three_and_a_half_days: toward(self.three_and_a_half_days, HALF_WEEK_TAU),
        }
    }
}

/// The stall share averages of each resource.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PressureAverages {
    pub cpu: ResourceAverages,
    pub io: ResourceAverages,
}

/// The share of one interval each resource spent stalled, between 0 and 1.
#[derive(Clone, Copy, Debug, PartialEq)]
struct IntervalShares {
    cpu: f64,
    io: f64,
    elapsed: Duration,
}

impl IntervalShares {
    /// The interval from `earlier` to `later`, or `None` when it says nothing
    /// about pressure: a counter went backwards, as it does when a
    /// restart recreates the cgroup; the clock did not advance; or the
    /// interval is longer than [`MAX_INTERVAL`] and so spans downtime.
    fn between(earlier: PressureSample, later: PressureSample) -> Option<Self> {
        let elapsed = later.at.duration_since(earlier.at).ok()?;
        if elapsed.is_zero() || elapsed > MAX_INTERVAL {
            return None;
        }
        let share = |before: u64, after: u64| {
            let stalled = after.checked_sub(before)?;
            // A cgroup cannot stall longer than wall time; clamp the
            // counters' rounding noise.
            Some((stalled as f64 / elapsed.as_micros() as f64).min(1.0))
        };
        Some(Self {
            cpu: share(earlier.totals.cpu_us, later.totals.cpu_us)?,
            io: share(earlier.totals.io_us, later.totals.io_us)?,
            elapsed,
        })
    }
}

impl PressureAverages {
    fn seed(shares: IntervalShares) -> Self {
        Self {
            cpu: ResourceAverages::seed(shares.cpu),
            io: ResourceAverages::seed(shares.io),
        }
    }

    /// Fold one interval in, weighted by its length, so one interval counts
    /// as much as the shorter ones it could have been.
    fn fold(self, shares: IntervalShares) -> Self {
        Self {
            cpu: self.cpu.fold(shares.cpu, shares.elapsed),
            io: self.io.fold(shares.io, shares.elapsed),
        }
    }
}

/// The durable pressure history: the last reading and the averages over the
/// intervals measured since the first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PressureState {
    pub last: PressureSample,
    /// `None` until an interval has been measured.
    pub averages: Option<PressureAverages>,
}

impl PressureState {
    /// Account for this run's `reading`. History starts at the first reading,
    /// and a reading always becomes the last one, so an interval that says
    /// nothing is skipped and the next is measured from the new counters.
    pub fn record(previous: Option<Self>, reading: PressureSample) -> Self {
        let Some(previous) = previous else {
            return Self {
                last: reading,
                averages: None,
            };
        };
        let averages = match (
            previous.averages,
            IntervalShares::between(previous.last, reading),
        ) {
            (averages, None) => averages,
            (None, Some(shares)) => Some(PressureAverages::seed(shares)),
            (Some(averages), Some(shares)) => Some(averages.fold(shares)),
        };
        Self {
            last: reading,
            averages,
        }
    }
}

/// The averages per resource, or the policy's maximums for them, as a report
/// states them and the check compares them: in per-mille of wall time.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
pub struct HostPressure {
    pub cpu: PressurePermille,
    pub io: PressurePermille,
}

/// One resource's figures, in per-mille of wall time (0..=1000): the share of
/// time some task stalled on it, so 213 is 21.3%.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
pub struct PressurePermille {
    /// The average with a 6-hour time constant.
    #[serde(rename = "6h")]
    pub six_hours: u16,
    /// The average with a 3.5-day time constant.
    #[serde(rename = "3.5d")]
    pub three_and_a_half_days: u16,
}

impl From<PressureAverages> for HostPressure {
    fn from(averages: PressureAverages) -> Self {
        // An average of shares in 0..=1 stays in 0..=1.
        let permille = |share: f64| (share.clamp(0.0, 1.0) * 1000.0).round() as u16;
        let resource = |averages: ResourceAverages| PressurePermille {
            six_hours: permille(averages.six_hours),
            three_and_a_half_days: permille(averages.three_and_a_half_days),
        };
        Self {
            cpu: resource(averages.cpu),
            io: resource(averages.io),
        }
    }
}

impl From<MaxPressurePercent> for HostPressure {
    fn from(max: MaxPressurePercent) -> Self {
        let resource = |max: MaxPressureWindows| PressurePermille {
            six_hours: u16::from(max.six_hours) * 10,
            three_and_a_half_days: u16::from(max.three_and_a_half_days) * 10,
        };
        Self {
            cpu: resource(max.cpu),
            io: resource(max.io),
        }
    }
}

impl HostPressure {
    /// The readiness outcome of these averages against `max`, naming the
    /// shorter window that fails, for either resource. The averages compare
    /// at the 0.1% a report shows.
    pub fn outcome(self, max: Self) -> ReadinessOutcome {
        let over = |window: fn(PressurePermille) -> u16| {
            window(self.cpu) > window(max.cpu) || window(self.io) > window(max.io)
        };
        if over(|windows| windows.six_hours) {
            ReadinessOutcome::SixHourPressureHigh
        } else if over(|windows| windows.three_and_a_half_days) {
            ReadinessOutcome::HalfWeekPressureHigh
        } else {
            ReadinessOutcome::Pass
        }
    }
}

#[cfg(test)]
#[path = "../tests/host_pressure.rs"]
mod tests;
