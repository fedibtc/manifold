//! Metrics-specific worker interfaces.

use async_trait::async_trait;

pub(crate) use crate::journal_target::{CollectionTarget, CommitOutcome, WorkTarget};

#[async_trait]
#[allow(dead_code)]
pub(crate) trait TargetCatalog: Send + Sync {
    type Commit: Send;

    async fn active_targets(
        &self,
    ) -> Result<Vec<CollectionTarget>, Box<dyn std::error::Error + Send + Sync>>;

    async fn begin_work(
        &self,
        target: &CollectionTarget,
    ) -> Result<Option<WorkTarget>, Box<dyn std::error::Error + Send + Sync>>;

    async fn commit_if_current(
        &self,
        target: WorkTarget,
        commit: Self::Commit,
    ) -> Result<CommitOutcome, Box<dyn std::error::Error + Send + Sync>>;
}

#[async_trait]
#[allow(dead_code)]
pub(crate) trait MetricsCollector: Send + Sync {
    type Commit: Send;

    async fn collect_metrics(
        &self,
        target: &WorkTarget,
    ) -> Result<Self::Commit, Box<dyn std::error::Error + Send + Sync>>;
}
