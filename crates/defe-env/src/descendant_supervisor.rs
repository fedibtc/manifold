//! Linux PID-namespace supervision and subprocess brokering.

use std::fs;
use std::io;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::os::unix::process::CommandExt as _;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};

const TEST_BROKER_FAILURE_ENV: &str = "DEFE_ENV_TEST_BROKER_FAILURE";
const TEST_CHILD_FIRST_PGRP_ENV: &str = "DEFE_ENV_TEST_CHILD_FIRST_PGRP";
const TEST_PRESERVE_CLOSED_STDIO_ENV: &str = "DEFE_ENV_TEST_PRESERVE_CLOSED_STDIO";

/// A command prepared for the namespace-resident spawn broker.
pub(crate) struct NamespacedCommand {
    /// Tokio command which starts the trusted status-proxy helper.
    pub(crate) command: tokio::process::Command,
    pid_report: OwnedFd,
    inherited_report: OwnedFd,
    inherited_user_namespace: OwnedFd,
    inherited_pid_namespace: OwnedFd,
}

/// A spawned status proxy and the host PID of its contained command.
pub(crate) struct NamespacedChild {
    /// Status proxy preserving the contained command's exit status.
    pub(crate) child: tokio::process::Child,
    /// Host PID of the actual command inside the PID namespace.
    pub(crate) command_pid: i32,
}

/// Owns the PID namespace containing every environment subprocess.
pub(crate) struct DescendantSupervisor {
    init_pid: i32,
    init_pidfd: OwnedFd,
    bootstrap_pid: i32,
    user_namespace: OwnedFd,
    pid_namespace: OwnedFd,
    state: Mutex<SupervisorState>,
    faults: Mutex<FaultInjection>,
}

#[derive(Default)]
struct SupervisorState {
    closed: bool,
    cleaned: bool,
    helpers: Vec<ProcessRef>,
    lifetime_guard: Option<ProcessRef>,
}

struct ProcessRef {
    pid: i32,
    pidfd: OwnedFd,
}

#[derive(Default)]
struct FaultInjection {
    inspection_failures: usize,
    signal_failures: usize,
    helper_open_failures: usize,
}

impl DescendantSupervisor {
    /// Establishes a child user/PID namespace before any setup subprocess.
    pub(crate) fn establish() -> Result<Self> {
        let (report_read, report_write) = pipe_cloexec()?;
        let composer_pid = unsafe { libc::getpid() };
        let bootstrap_pid = unsafe { libc::fork() };
        if bootstrap_pid < 0 {
            return Err(io::Error::last_os_error()).context("start namespace bootstrap");
        }
        if bootstrap_pid == 0 {
            drop(report_read);
            namespace_bootstrap(report_write, composer_pid);
        }
        drop(report_write);
        let init_pid = read_i32(&report_read).context("read namespace-init PID")?;
        ensure!(init_pid > 0, "namespace bootstrap failed");
        let user_namespace = duplicate_above_stdio(&open_namespace(init_pid, "user")?)?;
        let pid_namespace = duplicate_above_stdio(&open_namespace(init_pid, "pid")?)?;
        let init_pidfd = duplicate_above_stdio(&pidfd_open(init_pid)?)?;
        Ok(Self {
            init_pid,
            init_pidfd,
            bootstrap_pid,
            user_namespace,
            pid_namespace,
            state: Mutex::new(SupervisorState::default()),
            faults: Mutex::new(FaultInjection::default()),
        })
    }

    /// Copies argv, environment overrides, and cwd into a namespace helper command.
    pub(crate) fn wrap(
        &self,
        original: &tokio::process::Command,
        process_group: bool,
    ) -> Result<NamespacedCommand> {
        let original = original.as_std();
        let preserve_closed_stdio = process_group
            || original
                .get_envs()
                .any(|(key, value)| key == TEST_PRESERVE_CLOSED_STDIO_ENV && value.is_some());
        let closed_stdio = if preserve_closed_stdio {
            (0..=2).fold(0_u8, |mask, fd| {
                mask | (u8::from(unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0) << fd)
            })
        } else {
            0
        };
        let (pid_report, inherited_report) = pipe_cloexec()?;
        let inherited_report = duplicate_above_stdio(&inherited_report)?;
        let inherited_user_namespace = duplicate_above_stdio(&self.user_namespace)?;
        let inherited_pid_namespace = duplicate_above_stdio(&self.pid_namespace)?;
        let user_fd = inherited_user_namespace.as_raw_fd();
        let pid_fd = inherited_pid_namespace.as_raw_fd();
        let report_fd = inherited_report.as_raw_fd();
        let mut command = tokio::process::Command::new(std::env::current_exe()?);
        command
            .arg("--internal-namespace-spawn")
            .arg(user_fd.to_string())
            .arg(pid_fd.to_string())
            .arg(report_fd.to_string())
            .arg(if process_group { "1" } else { "0" })
            .arg(closed_stdio.to_string())
            .arg("--")
            .arg(original.get_program())
            .args(original.get_args());
        for (key, value) in original.get_envs() {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        if let Some(cwd) = original.get_current_dir() {
            command.current_dir(cwd);
        }
        unsafe {
            command.as_std_mut().pre_exec(move || {
                for fd in [user_fd, pid_fd, report_fd] {
                    if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        Ok(NamespacedCommand {
            command,
            pid_report,
            inherited_report,
            inherited_user_namespace,
            inherited_pid_namespace,
        })
    }

    /// Spawns one prepared command unless teardown permanently closed admission.
    pub(crate) fn spawn(&self, mut command: NamespacedCommand) -> Result<NamespacedChild> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state.closed {
            bail!("environment teardown has stopped subprocess admission");
        }
        let mut child = command.command.spawn()?;
        drop(command.inherited_report);
        drop(command.inherited_user_namespace);
        drop(command.inherited_pid_namespace);
        let helper_pid = i32::try_from(child.id().context("namespace helper has no PID")?)?;
        if self.take_helper_open_failure() {
            unsafe { libc::kill(helper_pid, libc::SIGKILL) };
            reap_pid(helper_pid)?;
            bail!("injected helper pidfd-open failure");
        }
        let helper_pidfd = match pidfd_open(helper_pid) {
            Ok(pidfd) => pidfd,
            Err(error) => {
                unsafe { libc::kill(helper_pid, libc::SIGKILL) };
                reap_pid(helper_pid)?;
                return Err(error).context("open namespace helper pidfd");
            }
        };
        state.helpers.push(ProcessRef {
            pid: helper_pid,
            pidfd: helper_pidfd,
        });
        let command_pid = match read_i32(&command.pid_report) {
            Ok(pid) => pid,
            Err(error) => {
                let helper = state.helpers.last().expect("helper was registered");
                let _ = pidfd_send_signal(&helper.pidfd, libc::SIGKILL);
                while !pidfd_has_exited(&helper.pidfd)? {
                    std::thread::sleep(Duration::from_millis(1));
                }
                let _ = child.try_wait();
                return Err(error).context("read contained command PID");
            }
        };
        ensure!(
            command_pid > 0,
            "namespace spawn helper failed before command launch"
        );
        Ok(NamespacedChild { child, command_pid })
    }

    /// Gives a helper custody of the Defe socket until namespace teardown completes.
    pub(crate) fn guard_connection(&self, connection: OwnedFd) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        ensure!(
            state.lifetime_guard.is_none(),
            "lease lifetime guard already exists"
        );
        let inherited_init = duplicate_above_stdio(&self.init_pidfd)?;
        let inherited_connection = duplicate_above_stdio(&connection)?;
        let init_fd = inherited_init.as_raw_fd();
        let connection_fd = inherited_connection.as_raw_fd();
        let mut command = std::process::Command::new(std::env::current_exe()?);
        command
            .arg("--internal-lease-guard")
            .arg(init_fd.to_string())
            .arg(connection_fd.to_string());
        unsafe {
            command.pre_exec(move || {
                for fd in [init_fd, connection_fd] {
                    if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut child = command.spawn().context("start Defe lease lifetime guard")?;
        drop((inherited_init, inherited_connection));
        let pid = i32::try_from(child.id())?;
        let pidfd = match pidfd_open(pid) {
            Ok(pidfd) => pidfd,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).context("register Defe lease lifetime guard");
            }
        };
        let process = ProcessRef { pid, pidfd };
        std::mem::forget(child);
        state.lifetime_guard = Some(process);
        Ok(())
    }

    /// Stops admission and destroys the complete environment PID namespace.
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
            match self.terminate_and_reap_inner(&mut state) {
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

    fn terminate_and_reap_inner(&self, state: &mut SupervisorState) -> Result<()> {
        if !self.pidfd_has_exited()? {
            self.pidfd_send_signal(libc::SIGKILL)?;
        }
        while !self.pidfd_has_exited()? {
            std::thread::sleep(Duration::from_millis(10));
        }
        reap_pid(self.init_pid)?;
        for helper in &state.helpers {
            while !pidfd_has_exited(&helper.pidfd)? {
                std::thread::sleep(Duration::from_millis(1));
            }
            reap_pid(helper.pid)?;
        }
        reap_pid(self.bootstrap_pid)?;
        if let Some(guard) = &state.lifetime_guard {
            while !pidfd_has_exited(&guard.pidfd)? {
                std::thread::sleep(Duration::from_millis(1));
            }
            reap_pid(guard.pid)?;
        }
        Ok(())
    }

    fn pidfd_has_exited(&self) -> Result<bool> {
        let mut faults = self.faults.lock().unwrap_or_else(|p| p.into_inner());
        if faults.inspection_failures > 0 {
            faults.inspection_failures -= 1;
            bail!("injected pidfd inspection failure");
        }
        drop(faults);
        pidfd_has_exited(&self.init_pidfd)
    }

    fn pidfd_send_signal(&self, signal: i32) -> Result<()> {
        let mut faults = self.faults.lock().unwrap_or_else(|p| p.into_inner());
        if faults.signal_failures > 0 {
            faults.signal_failures -= 1;
            bail!("injected pidfd signaling failure");
        }
        drop(faults);
        pidfd_send_signal(&self.init_pidfd, signal)
    }

    /// Injects deterministic failures for the internal adversarial test.
    pub(crate) fn inject_test_failures(&self, inspection: usize, signaling: usize) {
        let mut faults = self.faults.lock().unwrap_or_else(|p| p.into_inner());
        faults.inspection_failures = inspection;
        faults.signal_failures = signaling;
    }

    /// Injects one helper pidfd-open failure for ownership testing.
    pub(crate) fn inject_helper_open_failure(&self) {
        self.faults
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .helper_open_failures = 1;
    }

    fn take_helper_open_failure(&self) -> bool {
        let mut faults = self.faults.lock().unwrap_or_else(|p| p.into_inner());
        let fail = faults.helper_open_failures > 0;
        faults.helper_open_failures = faults.helper_open_failures.saturating_sub(1);
        fail
    }
}

impl Drop for DescendantSupervisor {
    fn drop(&mut self) {
        let _ = self.terminate_and_reap();
    }
}

/// Runs the synchronous namespace spawn helper and never returns.
pub(crate) fn run_namespace_spawn(args: &[std::ffi::OsString]) -> ! {
    if std::env::var_os(TEST_BROKER_FAILURE_ENV).is_some() {
        unsafe { libc::_exit(127) };
    }
    let fd = |index: usize| {
        args.get(index)
            .and_then(|arg| arg.to_str())
            .and_then(|arg| arg.parse::<i32>().ok())
            .unwrap_or_else(|| unsafe { libc::_exit(127) })
    };
    let user_fd = fd(1);
    let pid_fd = fd(2);
    let report_fd = fd(3);
    let process_group = args.get(4).is_some_and(|arg| arg == "1");
    let closed_stdio = args
        .get(5)
        .and_then(|arg| arg.to_str())
        .and_then(|arg| arg.parse::<u8>().ok())
        .unwrap_or_else(|| unsafe { libc::_exit(127) });
    let command = args.get(7).unwrap_or_else(|| unsafe { libc::_exit(127) });
    if unsafe { libc::setns(user_fd, libc::CLONE_NEWUSER) } != 0
        || unsafe { libc::setns(pid_fd, libc::CLONE_NEWPID) } != 0
    {
        unsafe { libc::_exit(127) };
    }
    let child = unsafe { libc::fork() };
    if child < 0 {
        unsafe { libc::_exit(127) };
    }
    if child == 0 {
        if process_group && unsafe { libc::setpgid(0, 0) } != 0 {
            unsafe { libc::_exit(127) };
        }
        unsafe {
            libc::close(user_fd);
            libc::close(pid_fd);
            libc::close(report_fd);
        }
        let mut workload = std::process::Command::new(command);
        workload.args(&args[8..]);
        unsafe {
            workload.pre_exec(move || {
                for fd in 0..=2 {
                    if closed_stdio & (1 << fd) != 0 {
                        libc::close(fd);
                    }
                }
                Ok(())
            });
        }
        let error = workload.exec();
        eprintln!("defe env: execute {}: {error}", command.to_string_lossy());
        unsafe { libc::_exit(127) };
    }
    if process_group {
        if std::env::var_os(TEST_CHILD_FIRST_PGRP_ENV).is_some() {
            std::thread::sleep(Duration::from_millis(50));
        }
        if unsafe { libc::setpgid(child, child) } != 0 && unsafe { libc::getpgid(child) } != child {
            unsafe { libc::_exit(127) };
        }
    }
    if unsafe { libc::write(report_fd, (&raw const child).cast(), 4) } != 4 {
        unsafe { libc::_exit(127) };
    }
    unsafe {
        libc::close(user_fd);
        libc::close(pid_fd);
        libc::close(report_fd);
    }
    let mut status = 0;
    loop {
        if unsafe { libc::waitpid(child, &mut status, 0) } == child {
            proxy_status(status);
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            unsafe { libc::_exit(127) };
        }
    }
}

/// Holds a Defe connection open without touching its protocol until teardown proof.
pub(crate) fn run_lease_guard(args: &[std::ffi::OsString]) -> ! {
    let fd = |index: usize| {
        args.get(index)
            .and_then(|arg| arg.to_str())
            .and_then(|arg| arg.parse::<i32>().ok())
            .unwrap_or_else(|| unsafe { libc::_exit(127) })
    };
    let init_pidfd = fd(1);
    let _connection_custody = unsafe { OwnedFd::from_raw_fd(fd(2)) };
    let init_pidfd = unsafe { OwnedFd::from_raw_fd(init_pidfd) };
    loop {
        match pidfd_has_exited(&init_pidfd) {
            Ok(true) => unsafe { libc::_exit(0) },
            Ok(false) | Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn namespace_bootstrap(report: OwnedFd, expected_parent: i32) -> ! {
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0
        || unsafe { libc::getppid() } != expected_parent
    {
        unsafe { libc::_exit(127) };
    }
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    if unsafe { libc::unshare(libc::CLONE_NEWUSER) } != 0
        || fs::write("/proc/self/setgroups", "deny\n").is_err()
        || fs::write("/proc/self/uid_map", format!("0 {uid} 1\n")).is_err()
        || fs::write("/proc/self/gid_map", format!("0 {gid} 1\n")).is_err()
        || unsafe { libc::unshare(libc::CLONE_NEWPID) } != 0
    {
        unsafe { libc::_exit(127) };
    }
    let (init_gate_read, init_gate_write) =
        pipe_cloexec().unwrap_or_else(|_| unsafe { libc::_exit(127) });
    let (armed_read, armed_write) = pipe_cloexec().unwrap_or_else(|_| unsafe { libc::_exit(127) });
    let init = unsafe { libc::fork() };
    if init < 0 {
        unsafe { libc::_exit(127) };
    }
    if init == 0 {
        drop(init_gate_write);
        drop(armed_read);
        namespace_init(init_gate_read, armed_write);
    }
    drop(init_gate_read);
    drop(armed_write);
    let mut armed = 0_u8;
    if unsafe { libc::read(armed_read.as_raw_fd(), (&raw mut armed).cast(), 1) } != 1 {
        unsafe { libc::_exit(127) };
    }
    drop(armed_read);
    if unsafe { libc::write(init_gate_write.as_raw_fd(), b"\n".as_ptr().cast(), 1) } != 1 {
        unsafe { libc::_exit(127) };
    }
    drop(init_gate_write);
    if unsafe { libc::write(report.as_raw_fd(), (&raw const init).cast(), 4) } != 4 {
        unsafe { libc::_exit(127) };
    }
    drop(report);
    let mut status = 0;
    while unsafe { libc::waitpid(init, &mut status, 0) } < 0 {
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            unsafe { libc::_exit(127) };
        }
    }
    unsafe { libc::_exit(0) };
}

fn namespace_init(gate: OwnedFd, armed: OwnedFd) -> ! {
    let mut byte = 0_u8;
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0 {
        unsafe { libc::_exit(127) };
    }
    if unsafe { libc::write(armed.as_raw_fd(), b"\n".as_ptr().cast(), 1) } != 1 {
        unsafe { libc::_exit(127) };
    }
    drop(armed);
    if unsafe { libc::read(gate.as_raw_fd(), (&raw mut byte).cast(), 1) } != 1 {
        unsafe { libc::_exit(127) };
    }
    drop(gate);
    loop {
        let mut status = 0;
        if unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) } <= 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn proxy_status(status: i32) -> ! {
    if libc::WIFEXITED(status) {
        unsafe { libc::_exit(libc::WEXITSTATUS(status)) };
    }
    let signal = libc::WTERMSIG(status);
    unsafe {
        let mut signals = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(&raw mut signals);
        libc::sigaddset(&raw mut signals, signal);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const signals, std::ptr::null_mut());
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
        libc::_exit(128 + signal);
    }
}

fn pipe_cloexec() -> Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0_i32; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error()).context("pipe2");
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn duplicate_above_stdio(fd: &OwnedFd) -> Result<OwnedFd> {
    let duplicate = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error()).context("duplicate broker descriptor");
    }
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

fn read_i32(fd: &OwnedFd) -> Result<i32> {
    let mut bytes = [0_u8; 4];
    let mut offset = 0;
    while offset < bytes.len() {
        let read = unsafe {
            libc::read(
                fd.as_raw_fd(),
                bytes[offset..].as_mut_ptr().cast(),
                bytes.len() - offset,
            )
        };
        if read == 0 {
            bail!("unexpected EOF");
        }
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("read");
        }
        offset += usize::try_from(read)?;
    }
    Ok(i32::from_ne_bytes(bytes))
}

fn open_namespace(pid: i32, name: &str) -> Result<OwnedFd> {
    let path = format!("/proc/{pid}/ns/{name}");
    let file = fs::File::open(&path).with_context(|| format!("open {path}"))?;
    Ok(file.into())
}

fn pidfd_open(pid: i32) -> Result<OwnedFd> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("pidfd_open");
    }
    Ok(unsafe { OwnedFd::from_raw_fd(i32::try_from(fd)?) })
}

fn pidfd_send_signal(pidfd: &OwnedFd, signal: i32) -> Result<()> {
    if unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    } < 0
    {
        return Err(io::Error::last_os_error()).context("pidfd_send_signal");
    }
    Ok(())
}

fn pidfd_has_exited(pidfd: &OwnedFd) -> Result<bool> {
    let mut pollfd = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&raw mut pollfd, 1, 0) };
    if result < 0 {
        return Err(io::Error::last_os_error()).context("poll pidfd");
    }
    ensure!(
        pollfd.revents & (libc::POLLERR | libc::POLLNVAL) == 0,
        "pidfd poll failed with events {:#x}",
        pollfd.revents
    );
    Ok(result == 1 && pollfd.revents & libc::POLLIN != 0)
}

fn reap_pid(pid: i32) -> Result<()> {
    let mut status = 0;
    loop {
        let result = unsafe { libc::waitpid(pid, &mut status, 0) };
        if result == pid {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if error.raw_os_error() == Some(libc::ECHILD) {
            return Ok(());
        }
        return Err(error).context("waitpid");
    }
}
