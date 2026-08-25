use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const REAPER_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Trailing bytes of a resource log quoted into a start failure message.
const LOG_TAIL_BYTES: u64 = 4096;

/// Configuration for a generic resource child process.
#[derive(Debug, Clone)]
pub struct ResourceProcessConfig {
    /// Executable path or program name to launch.
    pub program: OsString,
    /// Command-line arguments passed to the resource process.
    pub args: Vec<OsString>,
    /// Environment variables added or overridden for the resource process.
    pub envs: Vec<(OsString, OsString)>,
    /// Optional working directory for the resource process.
    pub current_dir: Option<PathBuf>,
    /// File that receives the child process standard output stream.
    pub stdout_log: PathBuf,
    /// File that receives the child process standard error stream.
    pub stderr_log: PathBuf,
}

impl ResourceProcessConfig {
    /// Create a process configuration with no arguments or environment overrides.
    #[must_use]
    pub fn new(
        program: impl Into<OsString>,
        stdout_log: impl Into<PathBuf>,
        stderr_log: impl Into<PathBuf>,
    ) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            envs: Vec::new(),
            current_dir: None,
            stdout_log: stdout_log.into(),
            stderr_log: stderr_log.into(),
        }
    }

    /// Append one command-line argument to the process configuration.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append multiple command-line arguments to the process configuration.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Add or override one environment variable for the resource process.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.envs.push((key.into(), value.into()));
        self
    }

    /// Set the working directory used when spawning the resource process.
    #[must_use]
    pub fn current_dir(mut self, current_dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(current_dir.into());
        self
    }
}

/// A supervised child process owned by a server-side resource.
///
/// The wrapper redirects stdout and stderr to configured log files, reaps exited
/// children when observed, and terminates any still-running child on explicit
/// stop or drop.
pub struct ResourceProcess {
    /// Operating-system process id captured immediately after spawn.
    pid: u32,
    /// Path where stdout is redirected.
    stdout_log: PathBuf,
    /// Path where stderr is redirected.
    stderr_log: PathBuf,
    /// Shared child-process state observed by the owner and reaper thread.
    state: Arc<Mutex<ResourceProcessState>>,
    /// Background thread that notices child exit without blocking the owner.
    reaper: Mutex<Option<JoinHandle<()>>>,
}

impl ResourceProcess {
    /// Spawn a child process from a [`ResourceProcessConfig`].
    pub fn spawn(config: ResourceProcessConfig) -> io::Result<Self> {
        let mut command = Command::new(config.program);
        command.args(config.args);
        command.envs(config.envs);
        if let Some(current_dir) = config.current_dir {
            command.current_dir(current_dir);
        }
        Self::spawn_command(command, config.stdout_log, config.stderr_log)
    }

    /// Spawn a preconfigured command with stdout and stderr redirected to log files.
    pub fn spawn_command(
        mut command: Command,
        stdout_log: impl Into<PathBuf>,
        stderr_log: impl Into<PathBuf>,
    ) -> io::Result<Self> {
        let stdout_log = stdout_log.into();
        let stderr_log = stderr_log.into();
        let stdout = create_log_file(&stdout_log)?;
        let stderr = if stdout_log == stderr_log {
            Stdio::from(stdout.try_clone()?)
        } else {
            Stdio::from(create_log_file(&stderr_log)?)
        };
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(stderr)
            .spawn()?;
        let pid = child.id();

        let state = Arc::new(Mutex::new(ResourceProcessState {
            child: Some(child),
            exit_status: None,
        }));
        let reaper = spawn_reaper(Arc::clone(&state));

        Ok(Self {
            pid,
            stdout_log,
            stderr_log,
            state,
            reaper: Mutex::new(Some(reaper)),
        })
    }

    /// Return the operating-system process id captured at spawn time.
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// Return the log file path receiving the child process standard output.
    #[must_use]
    pub fn stdout_log(&self) -> &Path {
        &self.stdout_log
    }

    /// Return the log file path receiving the child process standard error.
    #[must_use]
    pub fn stderr_log(&self) -> &Path {
        &self.stderr_log
    }

    /// Return whether the child process still appears to be running.
    pub fn is_running(&self) -> bool {
        self.lock_state()
            .and_then(|mut state| state.refresh_exit_status().map(|status| status.is_none()))
            .unwrap_or(false)
    }

    /// Return the recorded exit status if the child process has exited.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.lock_state()
            .ok()
            .and_then(|mut state| state.refresh_exit_status().ok().flatten())
    }

    /// Wait for the child process to exit and return its final status.
    pub fn wait(&self) -> io::Result<ExitStatus> {
        let status = self.lock_state()?.wait()?;
        self.join_reaper();
        Ok(status)
    }

    /// Terminate the child process if it is still running and return its final status.
    pub fn stop(&self) -> io::Result<ExitStatus> {
        let status = self.lock_state()?.stop()?;
        self.join_reaper();
        Ok(status)
    }

    fn lock_state(&self) -> io::Result<MutexGuard<'_, ResourceProcessState>> {
        self.state
            .lock()
            .map_err(|_| io::Error::other("resource process mutex poisoned"))
    }

    fn join_reaper(&self) {
        let Ok(mut reaper) = self.reaper.lock() else {
            return;
        };
        if let Some(reaper) = reaper.take() {
            let _ = reaper.join();
        }
    }
}

impl Drop for ResourceProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn spawn_reaper(state: Arc<Mutex<ResourceProcessState>>) -> JoinHandle<()> {
    thread::spawn(move || {
        loop {
            let done = match state.lock() {
                Ok(mut state) => match state.refresh_exit_status() {
                    Ok(Some(_status)) => true,
                    Ok(None) => state.child.is_none(),
                    Err(_err) => true,
                },
                Err(_poisoned) => true,
            };

            if done {
                break;
            }
            thread::sleep(REAPER_POLL_INTERVAL);
        }
    })
}

struct ResourceProcessState {
    /// Live child handle while the process is still running and not yet waited on.
    child: Option<Child>,
    /// Cached exit status once the child has been reaped.
    exit_status: Option<ExitStatus>,
}

impl ResourceProcessState {
    fn refresh_exit_status(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.exit_status.is_some() {
            return Ok(self.exit_status);
        }

        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };

        if let Some(status) = child.try_wait()? {
            self.child = None;
            self.exit_status = Some(status);
        }

        Ok(self.exit_status)
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.exit_status {
            return Ok(status);
        }

        let Some(mut child) = self.child.take() else {
            return Err(io::Error::other(
                "resource process has no child and no recorded exit status",
            ));
        };

        let status = child.wait()?;
        self.exit_status = Some(status);
        Ok(status)
    }

    fn stop(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.refresh_exit_status()? {
            return Ok(status);
        }

        let Some(mut child) = self.child.take() else {
            return Err(io::Error::other(
                "resource process has no child and no recorded exit status",
            ));
        };

        let kill_result = child.kill();
        let wait_result = child.wait();

        match wait_result {
            Ok(status) => {
                self.exit_status = Some(status);
                Ok(status)
            }
            Err(wait_err) => match kill_result {
                Ok(()) => Err(wait_err),
                Err(kill_err) => Err(io::Error::other(format!(
                    "failed to kill resource process: {kill_err}; failed to wait for it: {wait_err}"
                ))),
            },
        }
    }
}

/// Quote the end of a resource process log for a start failure message.
///
/// A failure that only cites a log path is unreadable wherever the log does not
/// outlive the run: a Nix build sandbox discards its build directory, so the
/// cited file is already gone when somebody reads the error. Carrying the tail
/// in the message keeps the evidence with the failure that needs it.
pub fn log_tail(path: &Path) -> String {
    match read_log_tail(path) {
        Ok(tail) if tail.trim().is_empty() => format!("log {} is empty", path.display()),
        Ok(tail) => format!("log {} ends with:\n{tail}", path.display()),
        Err(err) => format!("log {} could not be read: {err}", path.display()),
    }
}

fn read_log_tail(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL_BYTES)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn create_log_file(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    File::create(path)
}

#[cfg(test)]
mod tests;
