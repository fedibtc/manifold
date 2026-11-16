//! Periodic shareable resource samples, so CPU, memory and disk limits on
//! operator hosts are visible in telemetry.
//!
//! The daemon and every bundled `fedimintd` sample their own process and write
//! to their own safe-event journal, so a seat's samples arrive in that seat's
//! journal without the daemon tracking child process ids. The daemon also
//! samples the cgroup and the network namespace that it shares with its
//! seats, the whole host's pressure (to tell the MG's own contention from
//! other services'), and names the filesystem of its data directory.
//!
//! Counters are cumulative. A consumer derives rates from the deltas between
//! two samples. A counter whose source the kernel does not provide is omitted
//! from the event.

use std::path::Path;
use std::time::Duration;

use mg_core::host_pressure::{Pressure, cgroup_v2_dir};

/// Early enough to see post-startup memory, after RocksDB and Iroh have opened.
const FIRST_SAMPLE_DELAY: Duration = Duration::from_secs(60);
/// The peak field covers the time between samples.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// The process that takes the samples.
#[derive(Clone, Copy)]
pub(crate) enum Role {
    /// The MG daemon. It also samples the cgroup and network namespace it
    /// shares with its seats, the host's pressure, and names the filesystem
    /// that holds the data directory.
    Daemon { data_filesystem: DataFilesystem },
    /// One bundled `fedimintd` seat process.
    Fedimintd,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::Daemon { .. } => "mg",
            Self::Fedimintd => "fedimintd",
        }
    }
}

/// Sample this process, and for the daemon also its cgroup and network
/// namespace, until the runtime shuts down.
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
            if let Role::Daemon { data_filesystem } = role {
                report_cgroup(data_filesystem);
            }
            tokio::time::sleep(SAMPLE_INTERVAL).await;
        }
    });
}

/// Report the cgroup and the network namespace that the daemon shares with
/// its seats, and the host's pressure. The cgroup path, device numbers and
/// interface names stay local.
fn report_cgroup(data_filesystem: DataFilesystem) {
    let cgroup = read("/proc/self/cgroup", cgroup_v2_dir);
    let cgroup_file = |name: &str| cgroup.as_ref().map(|dir| dir.join(name));
    let cpu = cgroup_file("cpu.stat").and_then(|path| read(&path, CgroupCpu::parse));
    let cpu_pressure = cgroup_file("cpu.pressure").and_then(|path| read(&path, Pressure::parse));
    let io_pressure = cgroup_file("io.pressure").and_then(|path| read(&path, Pressure::parse));
    let memory_pressure =
        cgroup_file("memory.pressure").and_then(|path| read(&path, Pressure::parse));
    let block_io = cgroup_file("io.stat").and_then(|path| read(&path, CgroupBlockIo::parse));
    let host_pressure = |resource: &str| {
        read(format!("/proc/pressure/{resource}"), Pressure::parse).map(|pressure| pressure.some_us)
    };
    let network = read("/proc/self/net/dev", NetnsTraffic::parse);
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
        cgroup_memory_pressure_us = memory_pressure.as_ref().map(|pressure| pressure.some_us),
        cgroup_memory_full_pressure_us = memory_pressure
            .as_ref()
            .and_then(|pressure| pressure.full_us),
        cgroup_block_read_bytes = block_io.as_ref().map(|io| io.read_bytes),
        cgroup_block_write_bytes = block_io.as_ref().map(|io| io.write_bytes),
        cgroup_block_write_ios = block_io.as_ref().map(|io| io.write_ios),
        host_cpu_pressure_us = host_pressure("cpu"),
        host_io_pressure_us = host_pressure("io"),
        host_memory_pressure_us = host_pressure("memory"),
        data_filesystem = %data_filesystem,
        netns_rx_bytes = network.as_ref().map(|network| network.rx_bytes),
        netns_tx_bytes = network.as_ref().map(|network| network.tx_bytes),
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

/// Block-layer IO of the cgroup that holds the daemon and its seats, summed
/// over devices: what reached the disks, including writeback the kernel
/// charges to the cgroup, unlike the per-process `write_bytes`, which counts
/// pages as they are dirtied and so depends on the filesystem.
#[derive(Debug, PartialEq, Eq)]
struct CgroupBlockIo {
    read_bytes: u64,
    write_bytes: u64,
    write_ios: u64,
}

impl CgroupBlockIo {
    /// Parse cgroup v2 `io.stat`: one `MAJ:MIN key=value ...` line per
    /// device. A cgroup without IO yet has an empty file.
    fn parse(stat: &str) -> Option<Self> {
        let mut io = Self {
            read_bytes: 0,
            write_bytes: 0,
            write_ios: 0,
        };
        for line in stat.lines() {
            let mut fields = line.split_ascii_whitespace();
            fields.next()?;
            for pair in fields {
                let (key, value) = pair.split_once('=')?;
                let total = match key {
                    "rbytes" => &mut io.read_bytes,
                    "wbytes" => &mut io.write_bytes,
                    "wios" => &mut io.write_ios,
                    _ => continue,
                };
                *total = total.checked_add(value.parse().ok()?)?;
            }
        }
        Some(io)
    }
}

/// The kind of filesystem that holds the MG data directory, where the seats'
/// databases sync every consensus unit. Filesystems count and write those
/// small synced writes differently, so storage figures compare only within
/// one kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DataFilesystem {
    Known(&'static str),
    /// A filesystem not in [`FILESYSTEM_MAGICS`], by its `statfs` magic.
    Other(u32),
    /// `statfs` failed.
    Unknown,
}

/// `statfs` `f_type` magics, from Linux `include/uapi/linux/magic.h` and
/// OpenZFS.
const FILESYSTEM_MAGICS: &[(u32, &str)] = &[
    (0xEF53, "ext4"),
    (0x9123_683E, "btrfs"),
    (0x5846_5342, "xfs"),
    (0x2FC1_2FC1, "zfs"),
    (0xF2F5_2010, "f2fs"),
    (0xCA45_1A4E, "bcachefs"),
    (0x794C_7630, "overlayfs"),
    (0x0102_1994, "tmpfs"),
    (0x6573_5546, "fuse"),
    (0x6969, "nfs"),
];

impl DataFilesystem {
    /// The filesystem that holds `path`.
    pub(crate) fn of(path: &Path) -> Self {
        use std::os::unix::ffi::OsStrExt as _;
        let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return Self::Unknown;
        };
        // SAFETY: `statfs` writes only into `stat`, which outlives the call,
        // and reads the NUL-terminated `path`.
        let mut stat = unsafe { std::mem::zeroed::<libc::statfs>() };
        if unsafe { libc::statfs(path.as_ptr(), &raw mut stat) } != 0 {
            return Self::Unknown;
        }
        // `f_type` is a signed word on some targets; the magics are 32-bit.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Self::from_magic(stat.f_type as u32)
    }

    fn from_magic(magic: u32) -> Self {
        FILESYSTEM_MAGICS
            .iter()
            .find(|(known, _)| *known == magic)
            .map_or(Self::Other(magic), |(_, name)| Self::Known(name))
    }
}

impl std::fmt::Display for DataFilesystem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known(name) => formatter.write_str(name),
            Self::Other(magic) => write!(formatter, "0x{magic:x}"),
            Self::Unknown => formatter.write_str("unknown"),
        }
    }
}

/// Bytes received and sent on every interface of this process's network
/// namespace except loopback.
///
/// The daemon and its seats share one namespace, and the kernel does not split
/// these counters by process. Where the MG runs in its own container
/// namespace, as on Umbrel and StartOS, they count exactly the MG's and its
/// seats' traffic, including bitcoind RPC to another container. With host
/// networking, as in the Docker Compose example, they count the whole host,
/// including any bitcoind on it. The sum falls when an interface goes away; a
/// consumer treats that like a restart.
#[derive(Debug, PartialEq, Eq)]
struct NetnsTraffic {
    rx_bytes: u64,
    tx_bytes: u64,
}

impl NetnsTraffic {
    /// Parse `/proc/<pid>/net/dev`: two header lines, then one
    /// `name: <8 receive counters> <8 transmit counters>` line per interface,
    /// where the first counter of each group is bytes.
    fn parse(dev: &str) -> Option<Self> {
        let mut lines = dev.lines();
        for _ in 0..2 {
            lines.next().filter(|header| header.contains('|'))?;
        }
        let mut traffic = Self {
            rx_bytes: 0,
            tx_bytes: 0,
        };
        for line in lines {
            let (name, counters) = line.split_once(':')?;
            let counters = counters
                .split_ascii_whitespace()
                .map(|counter| counter.parse::<u64>().ok())
                .collect::<Option<Vec<_>>>()?;
            let [rx_bytes, _, _, _, _, _, _, _, tx_bytes, ..] = counters[..] else {
                return None;
            };
            if name.trim() == "lo" {
                continue;
            }
            traffic.rx_bytes = traffic.rx_bytes.checked_add(rx_bytes)?;
            traffic.tx_bytes = traffic.tx_bytes.checked_add(tx_bytes)?;
        }
        Some(traffic)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CgroupBlockIo, CgroupCpu, DataFilesystem, NetnsTraffic, ProcessCpu, ProcessIo,
        ProcessMemory,
    };

    #[test]
    fn sums_cgroup_block_io_over_devices() {
        // Two devices; discard bytes (dbytes) and read IOs are not reported.
        let stat = "259:0 rbytes=4096 wbytes=1000000 rios=1 wios=244 dbytes=99999 dios=3\n\
            8:16 rbytes=8192 wbytes=24576 rios=2 wios=6 dbytes=0 dios=0\n";
        assert_eq!(
            CgroupBlockIo::parse(stat),
            Some(CgroupBlockIo {
                read_bytes: 12_288,
                write_bytes: 1_024_576,
                write_ios: 250,
            })
        );
        assert_eq!(
            CgroupBlockIo::parse(""),
            Some(CgroupBlockIo {
                read_bytes: 0,
                write_bytes: 0,
                write_ios: 0,
            })
        );
        assert_eq!(CgroupBlockIo::parse("259:0 rbytes=4096 wbytes=x\n"), None);
    }

    #[test]
    fn names_filesystems_by_statfs_magic() {
        assert_eq!(DataFilesystem::from_magic(0x9123_683E).to_string(), "btrfs");
        assert_eq!(DataFilesystem::from_magic(0x2FC1_2FC1).to_string(), "zfs");
        assert_eq!(
            DataFilesystem::from_magic(0x1234_5678).to_string(),
            "0x12345678"
        );
        assert_ne!(
            DataFilesystem::of(&std::env::temp_dir()),
            DataFilesystem::Unknown
        );
        assert_eq!(
            DataFilesystem::of(std::path::Path::new("/nonexistent/mg-data")),
            DataFilesystem::Unknown
        );
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
    fn sums_non_loopback_bytes_from_proc_net_dev() {
        // Columns after the name: receive bytes packets errs drop fifo frame
        // compressed multicast, then transmit bytes packets errs drop fifo
        // colls carrier compressed. lo has the largest counts, so summing it
        // shows. wg0 has no space after its colon, as the kernel prints a
        // wide first counter.
        let dev = "Inter-|   Receive                                                |  Transmit\n \
            face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    \
            lo: 7000000000    9100    0    0    0     0          0         0 7000000000    9100    0    0    0     0       0          0\n  \
            eth0: 5000000000  812345    0   12    0     0          0       310 1234567    8790    0    0    0     0       0          0\n \
            wg0:812345    4400    2    0    0     0          0         0 8790000000   31000    0    0    0     0       0          0\n";
        assert_eq!(
            NetnsTraffic::parse(dev),
            Some(NetnsTraffic {
                // eth0 + wg0, without lo; the receive sum exceeds u32.
                rx_bytes: 5_000_812_345,
                tx_bytes: 8_791_234_567,
            })
        );
    }

    #[test]
    fn parses_this_kernels_proc_net_dev() {
        let dev = std::fs::read_to_string("/proc/self/net/dev").expect("procfs");
        assert!(NetnsTraffic::parse(&dev).is_some(), "unparsed:\n{dev}");
    }

    #[test]
    fn rejects_proc_net_dev_without_headers_or_with_a_short_line() {
        let header = "Inter-|   Receive |  Transmit\n face |bytes packets|bytes packets\n";
        assert_eq!(NetnsTraffic::parse(""), None);
        assert_eq!(
            NetnsTraffic::parse("  eth0: 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16\n"),
            None
        );
        assert_eq!(
            NetnsTraffic::parse(&format!("{header}  eth0: 1 2 3 4 5 6 7 8\n")),
            None
        );
        assert_eq!(
            NetnsTraffic::parse(&format!(
                "{header}  eth0: 1 2 3 4 5 6 7 8 x 10 11 12 13 14 15 16\n"
            )),
            None
        );
        assert_eq!(
            NetnsTraffic::parse(&format!(
                "{header}    lo: 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16\n"
            )),
            Some(NetnsTraffic {
                rx_bytes: 0,
                tx_bytes: 0,
            })
        );
    }
}
