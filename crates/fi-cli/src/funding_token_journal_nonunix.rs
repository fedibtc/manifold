use std::path::Path;

/// Unsupported-platform adapter for the Unix-secured bearer-token journal.
pub(crate) enum FundingTokenJournal {}

impl FundingTokenJournal {
    /// Rejects journal use where required Unix filesystem primitives are absent.
    pub(crate) fn prepare(_source: &Path) -> anyhow::Result<Self> {
        anyhow::bail!("--funding-token-file is supported only on Unix platforms")
    }

    /// Returns no token because construction always fails.
    pub(crate) fn token(&self) -> &str {
        match *self {}
    }

    /// Cannot complete because construction always fails.
    pub(crate) fn complete(self) -> anyhow::Result<()> {
        match self {}
    }
}
