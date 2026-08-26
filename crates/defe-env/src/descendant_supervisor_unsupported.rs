//! Unsupported-platform response for the Linux environment lifetime boundary.

use anyhow::{Result, bail};

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
    pub(crate) fn spawn(
        &self,
        _command: &mut tokio::process::Command,
    ) -> Result<tokio::process::Child> {
        unreachable!("unsupported descendant supervisor cannot be constructed")
    }
}
