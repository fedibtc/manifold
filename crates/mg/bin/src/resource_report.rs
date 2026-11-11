//! Periodic shareable resource samples, so CPU, memory and disk limits on
//! operator hosts are visible in telemetry.
//!
//! The daemon and every bundled `fedimintd` sample their own process and write
//! to their own safe-event journal, so a seat's samples arrive in that seat's
//! journal without the daemon tracking child process ids. The daemon also
//! samples the cgroup that it shares with its seats.
//!
//! Counters are cumulative. A consumer derives rates from the deltas between
//! two samples. A counter whose source the kernel does not provide is omitted
//! from the event.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Early enough to see post-startup memory, after RocksDB and Iroh have opened.
const FIRST_SAMPLE_DELAY: Duration = Duration::from_secs(60);
/// The peak field covers the time between samples.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// The process that takes the samples.
#[derive(Clone, Copy)]
pub(crate) enum Role {
    /// The MG daemon. It also samples the cgroup it shares with its seats.
    Daemon,
    /// One bundled `fedimintd` seat process.
    Fedimintd,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::Daemon => "mg",
            Self::Fedimintd => "fedimintd",
        }
    }
}

/// Sample this process, and for the daemon also its cgroup, until the runtime
/// shuts down.
pub(crate) fn spawn(role: Role) {
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_SAMPLE_DELAY).await;
        loop {
            let Some(memory) = read("/proc/self/status", ProcessMemory::parse) else {
                // Non-Linux development hosts have no procfs.
                return;
            };
            let cpu = read("/proc/self/schedstat", ProcessCpu::parse);
            let io = read("/proc/self/io", ProcessIo::parse);
            tracing::info!(
                safe_to_share = true,
                process = role.as_str(),
                rss_bytes = memory.rss_bytes,
                anon_bytes = memory.anon_bytes,
                peak_rss_bytes = memory.peak_rss_bytes,
                threads = memory.threads,
                cpu_us = cpu.as_ref().map(|cpu| cpu.run_ns / 1000),
                cpu_wait_us = cpu.as_ref().map(|cpu| cpu.wait_ns / 1000),
                io_read_bytes = io.as_ref().map(|io| io.read_bytes),
                io_write_bytes = io.as_ref().map(|io| io.write_bytes),
                "process resource sample"
            );
            if let Role::Daemon = role {
                report_cgroup();
            }
            tokio::time::sleep(SAMPLE_INTERVAL).await;
        }
    });
}

/// Report the cgroup that the daemon shares with its seats. Its path stays
/// local.
fn report_cgroup() {
    let cgroup = read("/proc/self/cgroup", cgroup_v2_dir);
    let cgroup_file = |name: &str| cgroup.as_ref().map(|dir| dir.join(name));
    let cpu = cgroup_file("cpu.stat").and_then(|path| read(&path, CgroupCpu::parse));
    let cpu_pressure = cgroup_file("cpu.pressure").and_then(|path| read(&path, Pressure::parse));
    let io_pressure = cgroup_file("io.pressure").and_then(|path| read(&path, Pressure::parse));
    tracing::info!(
        safe_to_share = true,
        available_cpus = std::thread::available_parallelism()
            .ok()
            .map(|cpus| cpus.get() as u64),
        cgroup_cpu_us = cpu.as_ref().map(|cpu| cpu.usage_us),
        cgroup_throttled_periods = cpu.as_ref().and_then(|cpu| cpu.throttled_periods),
        cgroup_throttled_us = cpu.as_ref().and_then(|cpu| cpu.throttled_us),
        cgroup_cpu_pressure_us = cpu_pressure.as_ref().map(|pressure| pressure.some_us),
        cgroup_io_pressure_us = io_pressure.as_ref().map(|pressure| pressure.some_us),
        cgroup_io_full_pressure_us = io_pressure.as_ref().and_then(|pressure| pressure.full_us),
        "cgroup resource sample"
    );
}

/// Parse a procfs or cgroupfs file, or `None` when the kernel does not
/// provide it.
fn read<T>(path: impl AsRef<Path>, parse: impl FnOnce(&str) -> Option<T>) -> Option<T> {
    parse(&std::fs::read_to_string(path).ok()?)
}

/// The value of the first `name<separator>value` line.
fn field<'a>(text: &'a str, name: &str, separator: char) -> Option<&'a str> {
    text.lines()
        .find_map(|line| Some(line.strip_prefix(name)?.strip_prefix(separator)?.trim()))
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
            let value = field(status, name, ':')?;
            match value.strip_suffix(" kB") {
                Some(kib) => kib.trim().parse::<u64>().ok()?.checked_mul(1024),
                None => value.parse().ok(),
            }
        };
        Some(Self {
            rss_bytes: field("VmRSS")?,
            anon_bytes: field("RssAnon")?,
            peak_rss_bytes: field("VmHWM")?,
            threads: field("Threads")?,
        })
    }
}

/// Scheduler time of all threads of this process.
#[derive(Debug, PartialEq, Eq)]
struct ProcessCpu {
    run_ns: u64,
    /// Time runnable but waiting for a CPU. It grows when the host or the
    /// cgroup quota has too little CPU for the process.
    wait_ns: u64,
}

impl ProcessCpu {
    /// Parse `/proc/<pid>/schedstat`: run time, wait time and timeslices.
    fn parse(schedstat: &str) -> Option<Self> {
        let mut fields = schedstat.split_ascii_whitespace();
        Some(Self {
            run_ns: fields.next()?.parse().ok()?,
            wait_ns: fields.next()?.parse().ok()?,
        })
    }
}

/// Bytes this process caused to be read from or written to storage.
#[derive(Debug, PartialEq, Eq)]
struct ProcessIo {
    read_bytes: u64,
    write_bytes: u64,
}

impl ProcessIo {
    /// Parse `/proc/<pid>/io`.
    fn parse(io: &str) -> Option<Self> {
        Some(Self {
            read_bytes: field(io, "read_bytes", ':')?.parse().ok()?,
            write_bytes: field(io, "write_bytes", ':')?.parse().ok()?,
        })
    }
}

/// The cgroup v2 directory of this process, from `/proc/<pid>/cgroup`. A
/// container with its own cgroup namespace sees its own cgroup as `/`.
fn cgroup_v2_dir(cgroup: &str) -> Option<PathBuf> {
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    Some(Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/')))
}

/// CPU accounting of the cgroup that holds the daemon and its seats.
#[derive(Debug, PartialEq, Eq)]
struct CgroupCpu {
    usage_us: u64,
    /// Present only when the cgroup has the `cpu` controller, which enforces
    /// a CPU quota.
    throttled_periods: Option<u64>,
    throttled_us: Option<u64>,
}

impl CgroupCpu {
    /// Parse cgroup v2 `cpu.stat`.
    fn parse(stat: &str) -> Option<Self> {
        let field = |name: &str| field(stat, name, ' ')?.parse().ok();
        Some(Self {
            usage_us: field("usage_usec")?,
            throttled_periods: field("nr_throttled"),
            throttled_us: field("throttled_usec"),
        })
    }
}

/// Cumulative stall time from a pressure stall information file.
#[derive(Debug, PartialEq, Eq)]
struct Pressure {
    /// Time in which at least one task stalled on the resource.
    some_us: u64,
    /// Time in which all non-idle tasks stalled. CPU pressure has no
    /// meaningful `full` line.
    full_us: Option<u64>,
}

impl Pressure {
    /// Parse a PSI file such as `cpu.pressure` or `io.pressure`.
    fn parse(pressure: &str) -> Option<Self> {
        let total = |kind: &str| {
            field(pressure, kind, ' ')?
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{CgroupCpu, Pressure, ProcessCpu, ProcessIo, ProcessMemory, cgroup_v2_dir};

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

    #[test]
    fn parses_run_and_wait_time_in_order() {
        assert_eq!(
            ProcessCpu::parse("8123456789 2500000 4711\n"),
            Some(ProcessCpu {
                run_ns: 8_123_456_789,
                wait_ns: 2_500_000,
            })
        );
        assert_eq!(ProcessCpu::parse("8123456789\n"), None);
    }

    #[test]
    fn parses_storage_bytes_not_syscall_chars() {
        let io = "rchar: 900\nwchar: 800\nsyscr: 7\nsyscw: 6\n\
            read_bytes: 4096\nwrite_bytes: 12288\ncancelled_write_bytes: 4096\n";
        assert_eq!(
            ProcessIo::parse(io),
            Some(ProcessIo {
                read_bytes: 4096,
                write_bytes: 12288,
            })
        );
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

    #[test]
    fn parses_cgroup_cpu_stat_with_and_without_quota_fields() {
        let with_quota = "usage_usec 500\nuser_usec 300\nsystem_usec 200\n\
            nr_periods 40\nnr_throttled 9\nthrottled_usec 7000\n";
        assert_eq!(
            CgroupCpu::parse(with_quota),
            Some(CgroupCpu {
                usage_us: 500,
                throttled_periods: Some(9),
                throttled_us: Some(7000),
            })
        );
        assert_eq!(
            CgroupCpu::parse("usage_usec 500\nuser_usec 300\n"),
            Some(CgroupCpu {
                usage_us: 500,
                throttled_periods: None,
                throttled_us: None,
            })
        );
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
    }
}
