//! CPU and IO pressure on the cgroup this MG shares with its seats, as the
//! readiness pressure check compares it
//! ([`crate::seat_readiness`], `crates/mg/specs/SPEC-advertisement.md`).
//!
//! The kernel's pressure stall information (PSI) counts, per cgroup, the
//! microseconds in which at least one task stalled waiting for a CPU or for
//! storage IO. The counters are cumulative, so a window's average pressure is
//! the stall time added between stored samples divided by the wall time
//! between them. Unlike the system load average, the share of time stalled
//! does not depend on how fast or how many the CPUs are.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context as _;

/// The window whose average stops new seats.
pub const BLOCK_WINDOW: Duration = Duration::from_secs(6 * 60 * 60);

/// The window whose average lets a stopped MG accept new seats again: longer
/// than [`BLOCK_WINDOW`], so a host near a threshold does not open and close
/// by the hour.
pub const RESUME_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// The shortest time between two stored samples. Runs come every 10 minutes,
/// or every minute while a check fails; one sample per slot is enough for
/// averages over hours.
pub const MIN_SAMPLE_SPACING: Duration = Duration::from_secs(9 * 60);

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

/// Average pressure per resource over [`BLOCK_WINDOW`] and [`RESUME_WINDOW`],
/// in per-mille of wall time. A window is `None` until its samples cover at
/// least half of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
pub struct HostPressure {
    pub cpu: PressureWindows,
    pub io: PressureWindows,
}

/// One resource's averages, in per-mille of wall time.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
pub struct PressureWindows {
    #[serde(rename = "6h")]
    pub six_hours: Option<u16>,
    #[serde(rename = "24h")]
    pub day: Option<u16>,
}

impl HostPressure {
    /// The averages ending at `now` over `samples`, in any order.
    pub fn from_samples(samples: &[PressureSample], now: SystemTime) -> Self {
        let mut samples = samples.to_vec();
        samples.sort_by_key(|sample| sample.at);
        let windows = |total: fn(&PressureTotals) -> u64| PressureWindows {
            six_hours: window_permille(&samples, now, BLOCK_WINDOW, total),
            day: window_permille(&samples, now, RESUME_WINDOW, total),
        };
        Self {
            cpu: windows(|totals| totals.cpu_us),
            io: windows(|totals| totals.io_us),
        }
    }
}

/// Stall time added over wall time elapsed, summed over consecutive sample
/// pairs that both fall in the `window` ending at `now`. A pair whose counter
/// went backwards, as it does when a restart recreates the cgroup, or whose
/// clock did not advance, says nothing about its interval and is left out of
/// both sums.
fn window_permille(
    sorted: &[PressureSample],
    now: SystemTime,
    window: Duration,
    total: fn(&PressureTotals) -> u64,
) -> Option<u16> {
    let start = now.checked_sub(window)?;
    let (mut stalled_us, mut wall_us) = (0u128, 0u128);
    for pair in sorted.windows(2) {
        let [earlier, later] = pair else { continue };
        if earlier.at < start || later.at > now {
            continue;
        }
        let Ok(elapsed) = later.at.duration_since(earlier.at) else {
            continue;
        };
        let (before, after) = (total(&earlier.totals), total(&later.totals));
        if elapsed.is_zero() || after < before {
            continue;
        }
        stalled_us += u128::from(after - before);
        wall_us += elapsed.as_micros();
    }
    if wall_us == 0 || wall_us < window.as_micros() / 2 {
        return None;
    }
    // Rounded to the 0.1% the check compares at; a cgroup cannot stall
    // longer than wall time, so clamp rounding noise from the counters.
    let permille = (stalled_us * 1000 + wall_us / 2) / wall_us;
    Some(u16::try_from(permille.min(1000)).expect("clamped to 1000"))
}

/// Whether the pressure check holds new seats after this run, given whether
/// it held them after the last one. Either resource's 6-hour average above its
/// block threshold holds them; a held MG resumes only once both 24-hour
/// averages are at or below their resume thresholds. A window not yet covered
/// neither blocks nor resumes.
pub fn holds(
    held: bool,
    pressure: HostPressure,
    limit: fedi_decentralized_domain::HostPressureLimit,
) -> bool {
    let per_mille = |percent: u8| u16::from(percent) * 10;
    let blocks = |windows: PressureWindows, block_above: u8| {
        windows
            .six_hours
            .is_some_and(|average| average > per_mille(block_above))
    };
    let resumes = |windows: PressureWindows, resume_below: u8| {
        windows
            .day
            .is_some_and(|average| average <= per_mille(resume_below))
    };
    let blocked =
        blocks(pressure.cpu, limit.cpu.block_above) || blocks(pressure.io, limit.io.block_above);
    let resumed = resumes(pressure.cpu, limit.cpu.resume_below)
        && resumes(pressure.io, limit.io.resume_below);
    blocked || (held && !resumed)
}

/// The samples to keep after `sample`: none older than [`RESUME_WINDOW`]
/// before it, and it only when the newest kept one is at least
/// [`MIN_SAMPLE_SPACING`] older. Returns whether to store `sample` and the
/// time before which stored samples can go.
pub fn retention(newest_stored: Option<SystemTime>, sample: SystemTime) -> (bool, SystemTime) {
    let store = newest_stored.is_none_or(|newest| {
        sample
            .duration_since(newest)
            .is_ok_and(|since| since >= MIN_SAMPLE_SPACING)
    });
    (store, sample.checked_sub(RESUME_WINDOW).unwrap_or(sample))
}

#[cfg(test)]
#[path = "../tests/host_pressure.rs"]
mod tests;
