//! How many seats this host's memory, CPU and disk carry
//! (`crates/mg/specs/REQ-seat-capacity-default.md`).
//!
//! One task writes the runtime inputs beside the operator's limits: a limit
//! from sampled memory, one from the measured CPU capacity
//! ([`crate::cpu_capacity`]), one from the data filesystem's size, and the
//! publication's seat limit and per-seat
//! monthly data, which the operator's data budget is divided by
//! ([`crate::db::SeatLimits::data_max_seats`]). Admission uses the lowest
//! limit, and the database applies each write without breaking the
//! offer-epoch invariant ([`crate::db::Db::update_seats_config`]).

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use fedi_decentralized_domain::{
    AdmittedFeaturePolicy, DEFAULT_MAX_SEATS_PER_MG, DEFAULT_SEAT_DISK_GB,
    DEFAULT_SEAT_MEMORY_BUDGET_MIB, DEFAULT_SEAT_MONTHLY_DATA_GB,
};
use tokio::sync::watch;

use crate::cpu_capacity::CpuCapacity;
use crate::db::SeatsConfigUpdate;
use crate::fleet::Fleet;

#[cfg(test)]
#[path = "../tests/seat_capacity.rs"]
mod tests;

const MIB: u64 = 1024 * 1024;
/// The publication's default data per seat, for when there is no publication.
const DEFAULT_SEAT_MONTHLY_DATA: std::num::NonZeroU32 =
    std::num::NonZeroU32::new(DEFAULT_SEAT_MONTHLY_DATA_GB).expect("the default is nonzero");
/// The publication's default disk per seat, for when there is no publication.
const DEFAULT_SEAT_DISK: NonZeroU32 =
    NonZeroU32::new(DEFAULT_SEAT_DISK_GB).expect("the default is nonzero");
/// Disk figures are in GB of 10^9 bytes, as drives are sold.
const GB: u64 = 1_000_000_000;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// A raised limit takes effect only once this many consecutive samples agree,
/// while a lowered one takes effect at once.
const RAISE_AFTER_SAMPLES: usize = 3;

/// The runtime limit on total seats: one seat per whole budget of the memory
/// the seats can use, which is what is available plus what this daemon's
/// seat processes already hold privately.
fn memory_max_seats(available_ram_bytes: u64, seat_process_bytes: u64, budget_bytes: u64) -> u32 {
    let seats = available_ram_bytes.saturating_add(seat_process_bytes) / budget_bytes;
    u32::try_from(seats).unwrap_or(u32::MAX)
}

/// The runtime limit from CPU: one seat per whole per-seat budget of the
/// measured capacity, or none while the publication sets no budget.
fn cpu_max_seats(iterations_per_sec: u32, budget: Option<NonZeroU32>) -> Option<u32> {
    budget.map(|budget| iterations_per_sec / budget)
}

/// The runtime limit from disk: one seat per whole per-seat figure of the data
/// filesystem's size. Size stands in for the drive's write endurance, which
/// grows with it and which the MG cannot read.
fn disk_max_seats(filesystem_bytes: u64, seat_disk_gb: NonZeroU32) -> u32 {
    let seats = filesystem_bytes / (u64::from(seat_disk_gb.get()) * GB);
    u32::try_from(seats).unwrap_or(u32::MAX)
}

/// The size of the filesystem that holds `path`, used or not: its blocks
/// times their size, which is what `df` reports as its size. Free space does
/// not enter, since the limit is about how much writing the drive takes.
pub fn data_filesystem_bytes(path: &Path) -> anyhow::Result<u64> {
    let stat = rustix::fs::statvfs(path)
        .context("statvfs the data directory to derive a seat capacity")?;
    Ok(stat.f_blocks.saturating_mul(stat.f_frsize))
}

/// Lower at once; raise only after [`RAISE_AFTER_SAMPLES`] samples.
struct Smoothed(VecDeque<u32>);

impl Smoothed {
    /// Start from the stored limit, so a restart also needs
    /// [`RAISE_AFTER_SAMPLES`] samples to raise it.
    fn starting_at(stored: u32) -> Self {
        Self(VecDeque::from([stored]))
    }

    fn push(&mut self, sample: u32) -> u32 {
        if self.0.len() == RAISE_AFTER_SAMPLES {
            self.0.pop_front();
        }
        self.0.push_back(sample);
        *self.0.iter().min().expect("a sample was just pushed")
    }
}

/// Write the runtime seat limits until the runtime shuts down: at once, on
/// every newly admitted publication, and every [`SAMPLE_INTERVAL`]. On a host
/// without procfs the memory limit keeps its stored value (4 seats until a
/// first sample). The CPU limit divides `cpu`, measured once at start;
/// without a measurement (local E2E) there is no CPU limit. The disk limit
/// divides `data_filesystem_bytes`, also measured once at start, and is
/// likewise absent without it.
pub fn spawn_runtime_seat_limits(
    fleet: Arc<Fleet>,
    mut policy: watch::Receiver<Option<AdmittedFeaturePolicy>>,
    cpu: Option<CpuCapacity>,
    data_filesystem_bytes: Option<u64>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut smoothed =
            Smoothed::starting_at(fleet.availability_snapshot().await.limits.memory_max_seats);
        let mut sample_memory = true;
        loop {
            let (policy_max_seats, budget_mib, cpu_budget, seat_monthly_data_gb, seat_disk_gb) =
                policy.borrow_and_update().as_ref().map_or(
                    (
                        DEFAULT_MAX_SEATS_PER_MG,
                        DEFAULT_SEAT_MEMORY_BUDGET_MIB,
                        None,
                        DEFAULT_SEAT_MONTHLY_DATA,
                        DEFAULT_SEAT_DISK,
                    ),
                    |policy| {
                        (
                            policy.max_seats_per_mg(),
                            policy.seat_memory_budget_mib(),
                            policy.seat_cpu_iterations_per_sec(),
                            policy.seat_monthly_data_gb(),
                            policy.seat_disk_gb(),
                        )
                    },
                );
            let mut memory_limit = None;
            if sample_memory {
                match detect_available_ram_bytes() {
                    Ok(available) => {
                        memory_limit = Some(smoothed.push(memory_max_seats(
                            available,
                            seat_process_bytes(),
                            u64::from(budget_mib) * MIB,
                        )));
                    }
                    Err(error) => {
                        tracing::warn!(%error, "cannot sample memory; the memory seat limit stays");
                        sample_memory = false;
                    }
                }
            }
            let update = SeatsConfigUpdate {
                memory_max_seats: memory_limit,
                policy_max_seats: Some(policy_max_seats),
                cpu_max_seats: Some(
                    cpu.as_ref()
                        .and_then(|cpu| cpu_max_seats(cpu.iterations_per_sec, cpu_budget)),
                ),
                disk_max_seats: Some(
                    data_filesystem_bytes.map(|bytes| disk_max_seats(bytes, seat_disk_gb)),
                ),
                policy_seat_monthly_data_gb: Some(seat_monthly_data_gb),
                ..SeatsConfigUpdate::default()
            };
            if let Err(error) = fleet.update_seats_config(update).await {
                tracing::warn!(%error, "failed to record the runtime seat limits");
            }
            tokio::select! {
                changed = policy.changed() => {
                    if changed.is_err() {
                        // The publication runtime ended; keep sampling memory.
                        tokio::time::sleep(SAMPLE_INTERVAL).await;
                    }
                }
                () = tokio::time::sleep(SAMPLE_INTERVAL) => {}
            }
        }
    })
}

/// Available (not total) RAM: other services share the host, and the limit
/// must not budget memory they already hold. A container's cgroup limit
/// bounds it further, since `/proc/meminfo` reports the whole host.
pub(crate) fn detect_available_ram_bytes() -> anyhow::Result<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo")
        .context("read /proc/meminfo to derive a seat capacity")?;
    let available = parse_mem_available_bytes(&meminfo)?;
    let cgroups = std::fs::read_to_string("/proc/self/cgroup")
        .map(|cgroup| cgroup_dirs(&cgroup))
        .unwrap_or_default();
    let cgroup = cgroups
        .iter()
        .filter_map(|dir| {
            let read = |file| std::fs::read_to_string(dir.join(file)).ok();
            cgroup_headroom_bytes(
                read("memory.max").as_deref(),
                read("memory.current").as_deref(),
            )
        })
        .min();
    Ok(cgroup.map_or(available, |cgroup| cgroup.min(available)))
}

fn parse_mem_available_bytes(meminfo: &str) -> anyhow::Result<u64> {
    let line = meminfo
        .lines()
        .find(|line| line.starts_with("MemAvailable:"))
        .context("/proc/meminfo has no MemAvailable line")?;
    let kib: u64 = line
        .split_whitespace()
        .nth(1)
        .context("MemAvailable line has no value")?
        .parse()
        .context("MemAvailable value is not a number")?;
    Ok(kib * 1024)
}

/// The cgroup v2 directories that bound this process: its own and each
/// ancestor up to the root of the unified hierarchy. A limit on any of them
/// applies, for example on the slice of a systemd service.
fn cgroup_dirs(proc_self_cgroup: &str) -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new("/sys/fs/cgroup");
    let Some(own) = proc_self_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
    else {
        return Vec::new();
    };
    root.join(own.trim().trim_start_matches('/'))
        .ancestors()
        .take_while(|dir| dir.starts_with(root))
        .map(std::path::Path::to_path_buf)
        .collect()
}

/// Remaining room under a cgroup v2 memory limit, or `None` without one.
fn cgroup_headroom_bytes(max: Option<&str>, current: Option<&str>) -> Option<u64> {
    let max: u64 = max?.trim().parse().ok()?;
    let current: u64 = current?.trim().parse().ok()?;
    Some(max.saturating_sub(current))
}

/// Private memory held by this daemon's direct children, the seat
/// `fedimintd` processes. Zero where it cannot be read.
fn seat_process_bytes() -> u64 {
    let parent = std::process::id().to_string();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("status")).ok())
        .filter(|status| status_field(status, "PPid") == Some(parent.as_str()))
        .filter_map(|status| {
            let kib = status_field(&status, "RssAnon")?.strip_suffix(" kB")?;
            kib.trim().parse::<u64>().ok()?.checked_mul(1024)
        })
        .sum()
}

fn status_field<'a>(status: &'a str, name: &str) -> Option<&'a str> {
    status
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
        .map(str::trim)
}
