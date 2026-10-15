//! How many seats this host's memory carries
//! (`crates/mg/specs/REQ-seat-capacity-default.md`).
//!
//! One task writes two limits beside the operator's: one from sampled memory
//! and one from the setup-payment publication. Admission uses the lowest, and the
//! database applies each write without breaking the offer-epoch invariant
//! ([`crate::db::Db::update_seats_config`]).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use fedi_decentralized_domain::{
    AdmittedSetupPaymentFederations, DEFAULT_MAX_SEATS_PER_MG, DEFAULT_SEAT_MEMORY_BUDGET_MIB,
};
use tokio::sync::watch;

use crate::db::SeatsConfigUpdate;
use crate::fleet::Fleet;

#[cfg(test)]
#[path = "../tests/seat_capacity.rs"]
mod tests;

const MIB: u64 = 1024 * 1024;
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

/// Lower at once; raise only after [`RAISE_AFTER_SAMPLES`] samples.
#[derive(Default)]
struct Smoothed(VecDeque<u32>);

impl Smoothed {
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
/// first sample).
pub fn spawn_runtime_seat_limits(
    fleet: Arc<Fleet>,
    mut policy: watch::Receiver<Option<AdmittedSetupPaymentFederations>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut smoothed = Smoothed::default();
        let mut sample_memory = true;
        loop {
            let (policy_max_seats, budget_mib) = policy.borrow_and_update().as_ref().map_or(
                (DEFAULT_MAX_SEATS_PER_MG, DEFAULT_SEAT_MEMORY_BUDGET_MIB),
                |policy| (policy.max_seats_per_mg(), policy.seat_memory_budget_mib()),
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
    let cgroup = cgroup_headroom_bytes(
        std::fs::read_to_string("/sys/fs/cgroup/memory.max")
            .ok()
            .as_deref(),
        std::fs::read_to_string("/sys/fs/cgroup/memory.current")
            .ok()
            .as_deref(),
    );
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
