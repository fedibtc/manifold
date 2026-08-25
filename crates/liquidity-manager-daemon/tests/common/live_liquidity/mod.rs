#![allow(dead_code)]

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};

pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub mod bitcoin;
pub mod daemon;
pub mod esplora;
pub mod fedimint;
pub mod gateway;
pub mod trust;

pub fn unique_test_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_nanos();
    format!("{prefix}_{}_{}", std::process::id(), nanos)
}

pub fn locate_binary(env_var: &str, binary_name: &str) -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os(env_var).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        ensure!(
            path.is_file(),
            "{env_var} points to missing binary {}",
            path.display()
        );
        return Ok(path);
    }

    let path = std::env::var_os("PATH").context("PATH is not set")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(binary_name))
        .find(|candidate| candidate.is_file())
        .with_context(|| format!("locate {binary_name}; set {env_var} or add the binary to PATH"))
}

pub fn run_command(command: &mut Command) -> anyhow::Result<String> {
    let description = format!("{command:?}");
    let output = command
        .output()
        .with_context(|| format!("execute {description}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    ensure!(
        output.status.success(),
        "command failed: {description}, status={}, stdout={stdout}, stderr={stderr}",
        output.status
    );
    Ok(stdout)
}

pub fn use_devimint_if_under_defe(command: &mut Command) {
    if std::env::var_os("DEV_DEFE_SOCKET_PATH").is_some() {
        command.env("FM_IN_DEVIMINT", "1");
    }
}

pub struct ManagedProcess {
    name: String,
    child: Child,
    log_path: PathBuf,
}

impl ManagedProcess {
    pub fn spawn(
        name: impl Into<String>,
        command: &mut Command,
        log_path: impl Into<PathBuf>,
    ) -> anyhow::Result<Self> {
        let name = name.into();
        let log_path = log_path.into();
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create process log directory {}", parent.display()))?;
        }
        let stdout = File::create(&log_path)
            .with_context(|| format!("create {} log {}", name, log_path.display()))?;
        let stderr = stdout
            .try_clone()
            .with_context(|| format!("clone {} log handle", name))?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let child = command
            .spawn()
            .with_context(|| format!("spawn {name}: {command:?}"))?;
        Ok(Self {
            name,
            child,
            log_path,
        })
    }

    pub fn ensure_running(&mut self) -> anyhow::Result<()> {
        if let Some(status) = self.child.try_wait()? {
            let log = self.log_tail();
            anyhow::bail!("{} exited with {status}; log:\n{log}", self.name);
        }
        Ok(())
    }

    pub async fn wait_for_log(&mut self, needle: &str, timeout: Duration) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            self.ensure_running()?;
            if self.read_log_lossy().contains(needle) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "timed out waiting for {} log to contain {needle:?}; log:\n{}",
                    self.name,
                    self.log_tail()
                );
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    pub fn stop(&mut self) -> anyhow::Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }
        terminate_child(&mut self.child).with_context(|| format!("terminate {}", self.name))?;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.child
            .kill()
            .with_context(|| format!("kill {} after SIGTERM timeout", self.name))?;
        let _ = self.child.wait();
        anyhow::bail!("{} did not stop after SIGTERM", self.name)
    }

    /// Read the whole log, tolerating non-UTF-8 bytes so a stray invalid byte
    /// can't hide the needle `wait_for_log` is polling for.
    fn read_log_lossy(&self) -> String {
        String::from_utf8_lossy(&fs::read(&self.log_path).unwrap_or_default()).into_owned()
    }

    fn log_tail(&self) -> String {
        const MAX_LOG_BYTES: usize = 16 * 1024;
        let bytes = fs::read(&self.log_path).unwrap_or_default();
        let start = bytes.len().saturating_sub(MAX_LOG_BYTES);
        String::from_utf8_lossy(&bytes[start..]).into_owned()
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(unix)]
fn terminate_child(child: &mut Child) -> anyhow::Result<()> {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    const SIGTERM: i32 = 15;
    let result = unsafe { kill(child.id() as i32, SIGTERM) };
    ensure!(result == 0, "failed to send SIGTERM");
    Ok(())
}

#[cfg(not(unix))]
fn terminate_child(child: &mut Child) -> anyhow::Result<()> {
    child.kill().context("terminate process")
}

pub fn process_log_path(data_root: &Path, process_name: &str) -> PathBuf {
    data_root.join("logs").join(format!("{process_name}.log"))
}
