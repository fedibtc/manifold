//! Periodic shareable resource samples, so CPU, memory and disk limits on
//! operator hosts are visible in telemetry.
//!
//! The daemon and every bundled `fedimintd` sample their own process and write
//! to their own safe-event journal, so a seat's samples arrive in that seat's
//! journal without the daemon tracking child process ids. The daemon also
//! samples the cgroup and the network namespace that it shares with its
//! seats, the whole host's pressure (to tell the MG's own contention from
//! other services'), and names the filesystem of its data directory and its
//! size.
//!
//! Counters are cumulative. A consumer derives rates from the deltas between
//! two samples. A counter whose source the kernel does not provide is omitted
//! from the event.

use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use mg_core::host_pressure::{Pressure, cgroup_v2_dir};
use mg_core::seat_capacity::FilesystemKind;

/// Early enough to see post-startup memory, after RocksDB and Iroh have opened.
const FIRST_SAMPLE_DELAY: Duration = Duration::from_secs(60);
/// The peak field covers the time between samples.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// The process that takes the samples.
#[derive(Clone)]
pub(crate) enum Role {
    /// The MG daemon. It also samples the cgroup and network namespace it
    /// shares with its seats, the host's pressure, the backing block device's
    /// IO counters, and names the filesystem that holds the data directory
    /// and its size, `None` where it cannot be read.
    Daemon {
        data_dir: PathBuf,
        data_filesystem: FilesystemKind,
        data_filesystem_bytes: Option<u64>,
    },
    /// One bundled `fedimintd` seat process.
    Fedimintd,
}

impl Role {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Daemon { .. } => "mg",
            Self::Fedimintd => "fedimintd",
        }
    }
}

/// Sample this process, and for the daemon also its cgroup, backing block
/// device, and network namespace, until the runtime shuts down.
pub(crate) fn spawn(role: Role) {
    let data_device_source = match &role {
        Role::Daemon { data_dir, .. } => DataDeviceIoSource::resolve(data_dir),
        Role::Fedimintd => None,
    };
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
            if let Role::Daemon {
                data_filesystem,
                data_filesystem_bytes,
                ..
            } = &role
            {
                report_cgroup(
                    *data_filesystem,
                    *data_filesystem_bytes,
                    data_device_source.as_ref(),
                );
            }
            tokio::time::sleep(SAMPLE_INTERVAL).await;
        }
    });
}

/// Report the cgroup and the network namespace that the daemon shares with
/// its seats, the backing block device's cumulative IO counters, and the
/// host's pressure. The cgroup path, device numbers, mount points, pool
/// names, and interface names stay local.
fn report_cgroup(
    data_filesystem: FilesystemKind,
    data_filesystem_bytes: Option<u64>,
    data_device_source: Option<&DataDeviceIoSource>,
) {
    let cgroup = read("/proc/self/cgroup", cgroup_v2_dir);
    let cgroup_file = |name: &str| cgroup.as_ref().map(|dir| dir.join(name));
    let cpu = cgroup_file("cpu.stat").and_then(|path| read(&path, CgroupCpu::parse));
    let cpu_pressure = cgroup_file("cpu.pressure").and_then(|path| read(&path, Pressure::parse));
    let io_pressure = cgroup_file("io.pressure").and_then(|path| read(&path, Pressure::parse));
    let memory_pressure =
        cgroup_file("memory.pressure").and_then(|path| read(&path, Pressure::parse));
    let block_io = cgroup_file("io.stat").and_then(|path| read(&path, CgroupBlockIo::parse));
    let data_device_io = data_device_source.and_then(DataDeviceIoSource::sample);
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
        data_device_read_bytes = data_device_io.as_ref().map(|io| io.read_bytes),
        data_device_write_bytes = data_device_io.as_ref().map(|io| io.write_bytes),
        data_device_write_ios = data_device_io.as_ref().map(|io| io.write_ios),
        host_cpu_pressure_us = host_pressure("cpu"),
        host_io_pressure_us = host_pressure("io"),
        host_memory_pressure_us = host_pressure("memory"),
        data_filesystem = %data_filesystem,
        data_filesystem_bytes,
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

/// Block-layer IO counters of the device (or ZFS pool) backing the MG's data
/// directory, read directly from sysfs (`/sys/dev/block/<maj>:<min>/stat`) or
/// OpenZFS kstats (`/proc/spl/kstat/zfs/<pool>/io`).
///
/// Unlike cgroup `io.stat`, these counters include kernel-thread writeback that
/// is not charged to the MG's cgroup — specifically btrfs transaction/metadata
/// worker threads (`btrfs-transacti`, which add ~1.8–2.2x on top of cgroup
/// writes) and ZFS `txg` sync threads (where cgroup `io.stat` reads ~0).
/// On a shared volume these counters reflect all writers to that device/pool.
#[derive(Debug, PartialEq, Eq)]
struct DataDeviceIo {
    read_bytes: u64,
    write_bytes: u64,
    write_ios: u64,
}

impl DataDeviceIo {
    /// Parse Linux `/sys/dev/block/<maj>:<min>/stat` (sectors are always 512 B
    /// per kernel `Documentation/block/stat.rst`).
    fn parse_sysfs_stat(stat: &str) -> Option<Self> {
        let mut fields = stat.split_ascii_whitespace();
        let _read_ios = fields.next()?;
        let _read_merges = fields.next()?;
        let read_sectors: u64 = fields.next()?.parse().ok()?;
        let _read_ticks = fields.next()?;
        let write_ios: u64 = fields.next()?.parse().ok()?;
        let _write_merges = fields.next()?;
        let write_sectors: u64 = fields.next()?.parse().ok()?;
        Some(Self {
            read_bytes: read_sectors.checked_mul(512)?,
            write_bytes: write_sectors.checked_mul(512)?,
            write_ios,
        })
    }

    /// Parse OpenZFS `/proc/spl/kstat/zfs/<pool>/io`.
    /// Line 1 is the kstat header, line 2 lists column names (`nread nwritten
    /// reads writes ...`), and line 3 holds the cumulative values.
    fn parse_zfs_kstat_io(kstat: &str) -> Option<Self> {
        let mut lines = kstat.lines();
        let _header = lines.next()?;
        let cols: Vec<&str> = lines.next()?.split_ascii_whitespace().collect();
        let vals: Vec<&str> = lines.next()?.split_ascii_whitespace().collect();
        let col = |name: &str| -> Option<u64> {
            let idx = cols.iter().position(|c| *c == name)?;
            vals.get(idx)?.parse().ok()
        };
        Some(Self {
            read_bytes: col("nread")?,
            write_bytes: col("nwritten")?,
            write_ios: col("writes")?,
        })
    }
}

/// Kernel counter source backing the MG data directory, resolved once at start.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DataDeviceIoSource {
    SysfsBlockStat(PathBuf),
    ZfsPoolKstatIo(PathBuf),
}

impl DataDeviceIoSource {
    fn resolve(data_dir: &Path) -> Option<Self> {
        let canonical = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        if let Ok(meta) = std::fs::metadata(&canonical) {
            let dev = meta.dev();
            let major = libc::major(dev);
            let minor = libc::minor(dev);
            if major != 0 {
                let path = PathBuf::from(format!("/sys/dev/block/{major}:{minor}/stat"));
                if path.is_file() {
                    return Some(Self::SysfsBlockStat(path));
                }
            }
        }
        // Anonymous/virtual filesystems (e.g. btrfs reports st_dev major 0 for
        // subvolumes; ZFS reports anonymous st_dev) resolve via mountinfo.
        let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
        let entry = MountInfoEntry::find_for_path(&mountinfo, &canonical)?;
        if entry.fs_type == "zfs" {
            let pool = entry.source.split('/').next()?;
            if !pool.is_empty() && !pool.contains("..") && !pool.contains('\0') {
                let path = PathBuf::from(format!("/proc/spl/kstat/zfs/{pool}/io"));
                if path.is_file() {
                    return Some(Self::ZfsPoolKstatIo(path));
                }
            }
            return None;
        }
        if entry.major != 0 {
            let path = PathBuf::from(format!(
                "/sys/dev/block/{}:{}/stat",
                entry.major, entry.minor
            ));
            if path.is_file() {
                return Some(Self::SysfsBlockStat(path));
            }
        }
        if entry.source.starts_with('/') {
            let meta = std::fs::metadata(&entry.source).ok()?;
            if meta.file_type().is_block_device() {
                let rdev = meta.rdev();
                let major = libc::major(rdev);
                let minor = libc::minor(rdev);
                let path = PathBuf::from(format!("/sys/dev/block/{major}:{minor}/stat"));
                if path.is_file() {
                    return Some(Self::SysfsBlockStat(path));
                }
            }
        }
        None
    }

    fn sample(&self) -> Option<DataDeviceIo> {
        match self {
            Self::SysfsBlockStat(path) => read(path, DataDeviceIo::parse_sysfs_stat),
            Self::ZfsPoolKstatIo(path) => read(path, DataDeviceIo::parse_zfs_kstat_io),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct MountInfoEntry {
    major: u32,
    minor: u32,
    mount_point: PathBuf,
    fs_type: String,
    source: String,
}

impl MountInfoEntry {
    fn parse_line(line: &str) -> Option<Self> {
        let (left, right) = line.split_once(" - ")?;
        let mut left_fields = left.split(' ');
        let _mount_id = left_fields.next()?;
        let _parent_id = left_fields.next()?;
        let (maj, min) = left_fields.next()?.split_once(':')?;
        let major: u32 = maj.parse().ok()?;
        let minor: u32 = min.parse().ok()?;
        let _root = left_fields.next()?;
        let mount_point = PathBuf::from(unescape_mountinfo(left_fields.next()?));
        let mut right_fields = right.split(' ');
        let fs_type = right_fields.next()?.to_owned();
        let source = unescape_mountinfo(right_fields.next()?);
        Some(Self {
            major,
            minor,
            mount_point,
            fs_type,
            source,
        })
    }

    /// Find the deepest mount point that is a path-component prefix of `target`.
    /// Ties go to the later entry so stacked mounts resolve to the top mount.
    fn find_for_path(mountinfo: &str, target: &Path) -> Option<Self> {
        let mut best: Option<(usize, Self)> = None;
        for line in mountinfo.lines() {
            let Some(entry) = Self::parse_line(line) else {
                continue;
            };
            if target.starts_with(&entry.mount_point) {
                let depth = entry.mount_point.components().count();
                if best
                    .as_ref()
                    .is_none_or(|(best_depth, _)| depth >= *best_depth)
                {
                    best = Some((depth, entry));
                }
            }
        }
        best.map(|(_, entry)| entry)
    }
}

/// Decode kernel `\ooo` octal escapes used in `/proc/self/mountinfo`.
fn unescape_mountinfo(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Some(oct) = std::str::from_utf8(&bytes[i + 1..i + 4])
                .ok()
                .and_then(|s| u8::from_str_radix(s, 8).ok())
            {
                out.push(oct);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| raw.to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        CgroupBlockIo, CgroupCpu, DataDeviceIo, DataDeviceIoSource, MountInfoEntry, NetnsTraffic,
        ProcessCpu, ProcessIo, ProcessMemory,
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

    #[test]
    fn parses_sysfs_block_stat_sectors_to_bytes() {
        let stat = "     668        3     7642       39   439985   160912  4885024   745506        0   571889  1306775        0        0        0        0   193712   561229\n";
        assert_eq!(
            DataDeviceIo::parse_sysfs_stat(stat),
            Some(DataDeviceIo {
                read_bytes: 7642 * 512,
                write_bytes: 4_885_024 * 512,
                write_ios: 439_985,
            })
        );
        assert_eq!(DataDeviceIo::parse_sysfs_stat("1 2 3 4 5"), None);
        assert_eq!(DataDeviceIo::parse_sysfs_stat(""), None);
    }

    #[test]
    fn parses_zfs_kstat_pool_io() {
        let kstat = "31 1 0x01 1 144 5032123456 9988776655\n\
            nread    nwritten reads    writes   wtime    wlentime wupdate  rtime    rlentime rupdate  wcnt     rcnt\n\
            1048576  9876543210 42     123456   0        0        0        0        0        0        0        0\n";
        assert_eq!(
            DataDeviceIo::parse_zfs_kstat_io(kstat),
            Some(DataDeviceIo {
                read_bytes: 1_048_576,
                write_bytes: 9_876_543_210,
                write_ios: 123_456,
            })
        );
        assert_eq!(DataDeviceIo::parse_zfs_kstat_io("header only\n"), None);
    }

    #[test]
    fn resolves_deepest_mountinfo_entry_including_btrfs_and_zfs() {
        let mountinfo = "\
            22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n\
            12078 22 0:184 / /var/lib/fedi\\040data rw,relatime shared:2 - btrfs /dev/loop32 rw,ssd,subvol=/\n\
            12090 22 0:99 / /tank/mg rw,noatime shared:3 - zfs tank/mg rw,xattr\n";
        assert_eq!(
            MountInfoEntry::find_for_path(mountinfo, Path::new("/var/lib/fedi data/seats/0")),
            Some(MountInfoEntry {
                major: 0,
                minor: 184,
                mount_point: PathBuf::from("/var/lib/fedi data"),
                fs_type: "btrfs".to_owned(),
                source: "/dev/loop32".to_owned(),
            })
        );
        assert_eq!(
            MountInfoEntry::find_for_path(mountinfo, Path::new("/tank/mg/seats")),
            Some(MountInfoEntry {
                major: 0,
                minor: 99,
                mount_point: PathBuf::from("/tank/mg"),
                fs_type: "zfs".to_owned(),
                source: "tank/mg".to_owned(),
            })
        );
        // Must not match sibling directory sharing only a string prefix.
        assert_eq!(
            MountInfoEntry::find_for_path(mountinfo, Path::new("/tank/mg2/data")),
            Some(MountInfoEntry {
                major: 8,
                minor: 1,
                mount_point: PathBuf::from("/"),
                fs_type: "ext4".to_owned(),
                source: "/dev/sda1".to_owned(),
            })
        );
    }

    #[test]
    fn resolves_live_data_dir_block_device_when_backed_by_block_storage() {
        let dir = std::env::current_dir().expect("cwd");
        if let Some(source) = DataDeviceIoSource::resolve(&dir) {
            assert!(
                source.sample().is_some(),
                "resolved source {source:?} should parse cleanly"
            );
        }
    }
}
