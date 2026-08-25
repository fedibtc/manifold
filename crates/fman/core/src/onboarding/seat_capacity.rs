//! Initial seat-capacity recommendation from available RAM.
//! (`crates/fman/specs/REQ-seat-capacity-default.md`).

use anyhow::Context as _;

#[cfg(test)]
mod tests;

const GIB: u64 = 1024 * 1024 * 1024;
/// The per-seat RAM budget: one seat per 1.5 GiB of available RAM.
const PER_SEAT_BYTES: u64 = 3 * GIB / 2;
/// Above this the expected binding constraints are concurrent-ceremony CPU
/// and disk IO, not RAM.
const MAX_RECOMMENDED_SEATS: u64 = 8;

/// The REQ-seat-capacity-default rule: one seat per whole 1.5 GiB of
/// available RAM, capped at 8. Fractional budget truncates toward fewer
/// seats, and a host under 1.5 GiB gets 0 — too small to sell seats at all.
pub(crate) fn recommended_max_seats(available_ram_bytes: u64) -> u32 {
    (available_ram_bytes / PER_SEAT_BYTES).min(MAX_RECOMMENDED_SEATS) as u32
}

/// Available (not total) RAM, the rule's input: other services share the
/// host, and the recommendation must not budget memory they already hold.
pub(crate) fn detect_available_ram_bytes() -> anyhow::Result<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo")
        .context("read /proc/meminfo to derive an initial seat-capacity recommendation")?;
    parse_mem_available_bytes(&meminfo)
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
