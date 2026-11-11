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
                // A sandbox can hide procfs.
                return;
            };
            let cpu_us = clock_ticks_per_second().and_then(|ticks| {
                read("/proc/self/stat", |stat| {
                    ProcessCpu::parse(stat)?.micros(ticks)
                })
            });
            let io = read("/proc/self/io", ProcessIo::parse);
            tracing::info!(
                safe_to_share = true,
                process = role.as_str(),
                rss_bytes = memory.rss_bytes,
                anon_bytes = memory.anon_bytes,
                peak_rss_bytes = memory.peak_rss_bytes,
                threads = memory.threads,
                cpu_us,
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

/// CPU time of all threads of this process, including threads that have
/// exited, in clock ticks.
#[derive(Debug, PartialEq, Eq)]
struct ProcessCpu {
    user_ticks: u64,
    system_ticks: u64,
}

impl ProcessCpu {
    /// Parse `utime` and `stime` from `/proc/<pid>/stat`. They are the 14th
    /// and 15th fields. The 2nd field is the command name in parentheses,
    /// which can itself hold spaces and parentheses, so count from its end.
    fn parse(stat: &str) -> Option<Self> {
        let (_, after_name) = stat.rsplit_once(')')?;
        let mut fields = after_name.split_ascii_whitespace().skip(11);
        Some(Self {
            user_ticks: fields.next()?.parse().ok()?,
            system_ticks: fields.next()?.parse().ok()?,
        })
    }

    fn micros(&self, ticks_per_second: u64) -> Option<u64> {
        let ticks = u128::from(self.user_ticks) + u128::from(self.system_ticks);
        u64::try_from(ticks * 1_000_000 / u128::from(ticks_per_second)).ok()
    }
}

/// The unit of the CPU times in `/proc/<pid>/stat`.
fn clock_ticks_per_second() -> Option<u64> {
    // SAFETY: `sysconf` only reads a system configuration value.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(ticks).ok().filter(|ticks| *ticks > 0)
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
    fn parses_cpu_ticks_after_a_command_name_with_spaces_and_parentheses() {
        // pid (comm) state ppid pgrp session tty tpgid flags minflt cminflt
        // majflt cmajflt utime stime cutime cstime ...
        let stat = "4242 (fedimintd (seat) 1) S 1 4242 4242 0 -1 4194560 \
            900 0 3 0 1234 567 89 10 20 0 13 0\n";
        let cpu = ProcessCpu::parse(stat).expect("valid stat line");
        assert_eq!(
            cpu,
            ProcessCpu {
                user_ticks: 1234,
                system_ticks: 567,
            }
        );
        assert_eq!(cpu.micros(100), Some(18_010_000));
        assert_eq!(ProcessCpu::parse("4242 (short) S 1 2 3\n"), None);
    }

    #[test]
    fn counts_cpu_time_of_every_thread() {
        let ticks = super::clock_ticks_per_second().expect("Linux has a clock tick rate");
        let cpu_us = || {
            let stat = std::fs::read_to_string("/proc/self/stat").expect("procfs");
            ProcessCpu::parse(&stat)
                .and_then(|cpu| cpu.micros(ticks))
                .expect("valid stat")
        };
        let before = cpu_us();
        // Burn CPU only on threads that exit before the second sample. A
        // reading of the main thread alone would not grow.
        let workers: Vec<_> = (0..2)
            .map(|_| {
                std::thread::spawn(|| {
                    let start = std::time::Instant::now();
                    while start.elapsed() < std::time::Duration::from_millis(300) {
                        std::hint::black_box(0u64);
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("worker thread");
        }
        assert!(
            cpu_us() - before >= 100_000,
            "two exited threads burned up to 600 ms of CPU"
        );
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
