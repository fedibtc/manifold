//! Unsupported-platform response for the Linux environment lifetime boundary.

use std::os::fd::OwnedFd;

use anyhow::{Result, bail};

/// Placeholder prepared command; construction is unreachable off Linux.
pub(crate) struct NamespacedCommand {
    /// Placeholder Tokio command.
    pub(crate) command: tokio::process::Command,
}

/// Placeholder spawned child; construction is unreachable off Linux.
pub(crate) struct NamespacedChild {
    /// Placeholder Tokio child.
    pub(crate) child: tokio::process::Child,
    /// Placeholder command PID.
    pub(crate) command_pid: i32,
}

/// Rejects strict environment lifetime supervision on non-Linux systems.
pub(crate) struct DescendantSupervisor;

impl DescendantSupervisor {
    /// Reports that the required process-tree primitives are unavailable.
    pub(crate) fn establish() -> Result<Self> {
        bail!("defe env descendant supervision requires Linux")
    }

    /// Cannot run because construction always fails on this platform.
    pub(crate) fn terminate_and_reap(&self) -> Result<()> {
        unreachable!("unsupported descendant supervisor cannot be constructed")
    }

    /// Cannot run because construction always fails on this platform.
    pub(crate) fn wrap(
        &self,
        _command: &tokio::process::Command,
        _process_group: bool,
    ) -> Result<NamespacedCommand> {
        unreachable!("unsupported descendant supervisor cannot be constructed")
    }

    /// Cannot run because construction always fails on this platform.
    pub(crate) fn spawn(&self, _command: NamespacedCommand) -> Result<NamespacedChild> {
        unreachable!("unsupported descendant supervisor cannot be constructed")
    }

    /// Cannot run because construction always fails on this platform.
    pub(crate) fn guard_connection(&self, _connection: OwnedFd) -> Result<()> {
        unreachable!("unsupported descendant supervisor cannot be constructed")
    }

    /// Cannot run because construction always fails on this platform.
    pub(crate) fn inject_test_failures(&self, _inspection: usize, _signaling: usize) {
        unreachable!("unsupported descendant supervisor cannot be constructed")
    }

    /// Cannot run because construction always fails on this platform.
    pub(crate) fn inject_helper_open_failure(&self) {
        unreachable!("unsupported descendant supervisor cannot be constructed")
    }
}

/// Cannot run because internal namespace helpers require Linux.
pub(crate) fn run_namespace_spawn(_args: &[std::ffi::OsString]) -> ! {
    std::process::exit(127)
}

/// Cannot run because internal lease guards require Linux pidfds.
pub(crate) fn run_lease_guard(_args: &[std::ffi::OsString]) -> ! {
    std::process::exit(127)
}
