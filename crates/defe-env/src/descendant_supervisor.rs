//! Linux process-tree supervision for the disposable environment boundary.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

/// Owns every process descended from the environment composer.
///
/// Linux's child-subreaper facility keeps orphaned and daemonized descendants
/// attached to the composer. `/proc` supplies the complete descendant census,
/// while pidfds prevent a recycled numeric PID from receiving a signal intended
/// for an earlier process.
pub(crate) struct DescendantSupervisor {
    composer_pid: i32,
    state: Mutex<SupervisorState>,
}

#[derive(Default)]
struct SupervisorState {
    closed: bool,
    cleaned: bool,
}

#[derive(Clone, Copy)]
struct ProcessStat {
    parent_pid: i32,
    start_time: u64,
    stopped: bool,
    zombie: bool,
}

struct ProcessRef {
    pid: i32,
    start_time: u64,
    pidfd: OwnedFd,
}

impl DescendantSupervisor {
    /// Establishes the boundary before the composer starts any setup command.
    pub(crate) fn establish() -> Result<Self> {
        #[cfg(not(target_os = "linux"))]
        bail!("defe env descendant supervision requires Linux");

        #[cfg(target_os = "linux")]
        {
            if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("become environment child subreaper");
            }
            let composer_pid = i32::try_from(std::process::id())?;
            verify_pidfd_support(composer_pid)?;
            Ok(Self {
                composer_pid,
                state: Mutex::new(SupervisorState::default()),
            })
        }
    }

    /// Stops admission, terminates, and reaps all current environment descendants.
    pub(crate) fn terminate_and_reap(&self) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.closed = true;
        if state.cleaned {
            return Ok(());
        }
        loop {
            match self.terminate_and_reap_inner() {
                Ok(()) => {
                    state.cleaned = true;
                    return Ok(());
                }
                Err(error) => {
                    eprintln!(
                        "defe env: descendant cleanup incomplete; retaining resources and retrying: {error:#}"
                    );
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    /// Spawns one descendant unless teardown has permanently closed admission.
    pub(crate) fn spawn(
        &self,
        command: &mut tokio::process::Command,
    ) -> Result<tokio::process::Child> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state.closed {
            bail!("environment teardown has stopped subprocess admission");
        }
        Ok(command.spawn()?)
    }

    fn terminate_and_reap_inner(&self) -> Result<()> {
        let mut processes = HashMap::<(i32, u64), ProcessRef>::new();
        self.stop_to_fixed_point(&mut processes)?;

        signal_all(&processes, libc::SIGTERM);
        signal_all(&processes, libc::SIGCONT);
        let graceful_deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < graceful_deadline {
            reap_children();
            let _ = self.discover(&mut processes)?;
            processes.retain(|_, process| process_is_current(process));
            if processes.is_empty() {
                reap_children();
                if !self.has_descendants()? {
                    return Ok(());
                }
            }
            signal_all(&processes, libc::SIGTERM);
            std::thread::sleep(Duration::from_millis(10));
        }

        // Freeze the tree again before SIGKILL. A TERM handler may have forked,
        // but once every discovered process is stopped no descendant can admit
        // another process between the final census and the kill.
        self.stop_to_fixed_point(&mut processes)?;
        signal_all(&processes, libc::SIGKILL);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            reap_children();
            let _ = self.discover(&mut processes)?;
            processes.retain(|_, process| process_is_current(process));
            if processes.is_empty() {
                reap_children();
                if !self.has_descendants()? {
                    return Ok(());
                }
            }
            signal_all(&processes, libc::SIGKILL);
            std::thread::sleep(Duration::from_millis(10));
        }
        bail!("environment descendants survived SIGKILL")
    }

    fn stop_to_fixed_point(&self, processes: &mut HashMap<(i32, u64), ProcessRef>) -> Result<()> {
        loop {
            signal_all(processes, libc::SIGSTOP);
            wait_until_stopped(processes)?;
            let added = self.discover(processes)?;
            processes.retain(|_, process| process_is_current(process));
            if !added {
                return Ok(());
            }
        }
    }

    fn discover(&self, processes: &mut HashMap<(i32, u64), ProcessRef>) -> Result<bool> {
        let stats = process_stats()?;
        let descendants = descendants(&stats, self.composer_pid);
        let mut added = false;
        for pid in descendants {
            let Some(stat) = stats.get(&pid) else {
                continue;
            };
            if stat.zombie || processes.contains_key(&(pid, stat.start_time)) {
                continue;
            }
            if let Some(process) = open_stable_process(pid, stat.start_time) {
                processes.insert((pid, stat.start_time), process);
                added = true;
            }
        }
        Ok(added)
    }

    fn has_descendants(&self) -> Result<bool> {
        Ok(!descendants(&process_stats()?, self.composer_pid).is_empty())
    }
}

fn descendants(stats: &HashMap<i32, ProcessStat>, root: i32) -> HashSet<i32> {
    let mut descendants = HashSet::from([root]);
    loop {
        let previous_len = descendants.len();
        for (&pid, stat) in stats {
            if descendants.contains(&stat.parent_pid) {
                descendants.insert(pid);
            }
        }
        if descendants.len() == previous_len {
            break;
        }
    }
    descendants.remove(&root);
    descendants
}

impl Drop for DescendantSupervisor {
    fn drop(&mut self) {
        let _ = self.terminate_and_reap();
    }
}

fn verify_pidfd_support(pid: i32) -> Result<()> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("open composer pidfd");
    }
    let pidfd = unsafe { OwnedFd::from_raw_fd(i32::try_from(fd)?) };
    if unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            0,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error()).context("signal composer pidfd");
    }
    Ok(())
}

fn process_stats() -> Result<HashMap<i32, ProcessStat>> {
    let mut stats = HashMap::new();
    for entry in fs::read_dir("/proc").context("enumerate Linux processes")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        if let Ok(stat) = read_process_stat(&entry.path().join("stat")) {
            stats.insert(pid, stat);
        }
    }
    Ok(stats)
}

fn read_process_stat(path: &Path) -> Result<ProcessStat> {
    let contents = fs::read_to_string(path)?;
    let fields = contents
        .rsplit_once(") ")
        .context("malformed Linux process stat")?
        .1
        .split_ascii_whitespace()
        .collect::<Vec<_>>();
    Ok(ProcessStat {
        stopped: matches!(fields.first(), Some(&"T" | &"t")),
        zombie: fields.first() == Some(&"Z"),
        parent_pid: fields.get(1).context("missing process parent")?.parse()?,
        start_time: fields
            .get(19)
            .context("missing process start time")?
            .parse()?,
    })
}

fn wait_until_stopped(processes: &HashMap<(i32, u64), ProcessRef>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let all_stopped = processes.values().all(|process| {
            match read_process_stat(
                Path::new("/proc")
                    .join(process.pid.to_string())
                    .join("stat")
                    .as_path(),
            ) {
                Ok(stat) => stat.start_time != process.start_time || stat.stopped || stat.zombie,
                Err(_) => true,
            }
        });
        if all_stopped {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    bail!("environment descendants did not stop")
}

fn open_stable_process(pid: i32, expected_start_time: u64) -> Option<ProcessRef> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return None;
    }
    let pidfd = unsafe { OwnedFd::from_raw_fd(i32::try_from(fd).ok()?) };
    let stat = read_process_stat(
        Path::new("/proc")
            .join(pid.to_string())
            .join("stat")
            .as_path(),
    )
    .ok()?;
    (stat.start_time == expected_start_time).then_some(ProcessRef {
        pid,
        start_time: expected_start_time,
        pidfd,
    })
}

fn process_is_current(process: &ProcessRef) -> bool {
    read_process_stat(
        Path::new("/proc")
            .join(process.pid.to_string())
            .join("stat")
            .as_path(),
    )
    .is_ok_and(|stat| !stat.zombie && stat.start_time == process.start_time)
}

fn signal_all(processes: &HashMap<(i32, u64), ProcessRef>, signal: i32) {
    for process in processes.values() {
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                process.pidfd.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            );
        }
    }
}

fn reap_children() {
    let mut status = 0;
    while unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) } > 0 {}
}
