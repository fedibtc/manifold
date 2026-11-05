//! Periodic shareable memory samples, so operator-host memory use is visible
//! in telemetry.
//!
//! The daemon and every bundled `fedimintd` run this in their own process and
//! write to their own safe-event journal, so a seat's samples arrive in that
//! seat's journal without the daemon tracking child process ids.

use std::time::Duration;

/// Early enough to see post-startup memory, after RocksDB and Iroh have opened.
const FIRST_SAMPLE_DELAY: Duration = Duration::from_secs(60);
/// The peak field covers the time between samples.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(15 * 60);
const THP_MODE_PATH: &str = "/sys/kernel/mm/transparent_hugepage/enabled";

/// Sample this process until the runtime shuts down. `process` names the
/// process role in each event.
pub(crate) fn spawn(process: &'static str) {
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_SAMPLE_DELAY).await;
        loop {
            match std::fs::read_to_string("/proc/self/status")
                .ok()
                .as_deref()
                .and_then(ProcessMemory::parse)
            {
                Some(memory) => tracing::info!(
                    safe_to_share = true,
                    process,
                    rss_bytes = memory.rss_bytes,
                    anon_bytes = memory.anon_bytes,
                    peak_rss_bytes = memory.peak_rss_bytes,
                    threads = memory.threads,
                    thp_mode = thp_mode(std::fs::read_to_string(THP_MODE_PATH).ok().as_deref()),
                    thp_disabled = thp_disabled(),
                    "process memory sample"
                ),
                // Non-Linux development hosts have no procfs.
                None => return,
            }
            tokio::time::sleep(SAMPLE_INTERVAL).await;
        }
    });
}

/// The host's transparent huge page mode, which decides how much a process
/// without [`crate::huge_pages::disable`] retains. Only these fixed names reach
/// the shareable event, whatever the file holds.
fn thp_mode(enabled: Option<&str>) -> &'static str {
    let Some(enabled) = enabled else {
        return "not_built";
    };
    match enabled
        .split_whitespace()
        .find(|word| word.starts_with('['))
    {
        Some("[always]") => "always",
        Some("[madvise]") => "madvise",
        Some("[never]") => "never",
        _ => "unknown",
    }
}

/// Whether this process has huge pages off regardless of [`thp_mode`], which
/// separates a seat the MG protected from one on a host with huge pages off.
fn thp_disabled() -> bool {
    #[cfg(target_os = "linux")]
    return crate::huge_pages::disabled();
    #[cfg(not(target_os = "linux"))]
    return false;
}

#[derive(Debug, PartialEq, Eq)]
struct ProcessMemory {
    rss_bytes: u64,
    /// Private anonymous memory: the heap and thread stacks, which grow with
    /// each seat, unlike file-backed code pages that seats share.
    anon_bytes: u64,
    /// Peak resident memory since the process started.
    peak_rss_bytes: u64,
    threads: u64,
}

impl ProcessMemory {
    /// Parse the Linux `/proc/<pid>/status` fields this report needs.
    fn parse(status: &str) -> Option<Self> {
        let field = |name: &str| {
            status.lines().find_map(|line| {
                let value = line.strip_prefix(name)?.strip_prefix(':')?.trim();
                match value.strip_suffix(" kB") {
                    Some(kib) => kib.trim().parse::<u64>().ok()?.checked_mul(1024),
                    None => value.parse().ok(),
                }
            })
        };
        Some(Self {
            rss_bytes: field("VmRSS")?,
            anon_bytes: field("RssAnon")?,
            peak_rss_bytes: field("VmHWM")?,
            threads: field("Threads")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ProcessMemory, thp_mode};

    #[test]
    fn reads_the_bracketed_thp_mode() {
        assert_eq!(thp_mode(Some("[always] madvise never\n")), "always");
        assert_eq!(thp_mode(Some("always [madvise] never\n")), "madvise");
        assert_eq!(thp_mode(Some("always madvise [never]\n")), "never");
        assert_eq!(thp_mode(None), "not_built");
        // Anything else stays out of the shareable event.
        assert_eq!(thp_mode(Some("always defer [secret]\n")), "unknown");
        assert_eq!(thp_mode(Some("")), "unknown");
    }

    #[test]
    fn parses_linux_status_fields_in_bytes() {
        let status = "Name:\tfleet-manager\n\
            VmPeak:\t 2312340 kB\n\
            VmHWM:\t  176420 kB\n\
            VmRSS:\t  170212 kB\n\
            RssAnon:\t  115088 kB\n\
            RssFile:\t   55124 kB\n\
            Threads:\t126\n\
            SigQ:\t0/1027362\n";
        assert_eq!(
            ProcessMemory::parse(status),
            Some(ProcessMemory {
                rss_bytes: 170_212 * 1024,
                anon_bytes: 115_088 * 1024,
                peak_rss_bytes: 176_420 * 1024,
                threads: 126,
            })
        );
    }

    #[test]
    fn rejects_status_without_a_required_field() {
        assert_eq!(ProcessMemory::parse("VmRSS:\t1 kB\nThreads:\t1\n"), None);
    }
}
