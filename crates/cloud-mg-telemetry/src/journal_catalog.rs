//! Journal-specific target resolution and revision-fenced commit boundary.

use std::sync::Arc;

use crate::{
    archive::JournalArchive,
    journal_commit::JournalCommit,
    journal_poller::Clock,
    journal_target::{CollectionTarget, CommitOutcome, WorkTarget},
    store::Store,
};

#[derive(Clone)]
pub(crate) struct JournalCatalog {
    store: Store,
    archive: JournalArchive,
    clock: Arc<dyn Clock>,
}

impl JournalCatalog {
    pub(crate) fn new(store: Store, archive: JournalArchive, clock: Arc<dyn Clock>) -> Self {
        Self {
            store,
            archive,
            clock,
        }
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    pub(crate) fn archive(&self) -> &JournalArchive {
        &self.archive
    }

    pub(crate) async fn active_targets(
        &self,
    ) -> Result<Vec<CollectionTarget>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self
            .store
            .active_collection_targets(self.clock.now()?)
            .await?)
    }

    pub(crate) async fn begin_work(
        &self,
        target: &CollectionTarget,
    ) -> Result<Option<WorkTarget>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self
            .store
            .begin_collection_work(target, self.clock.now()?)
            .await?)
    }

    pub(crate) async fn commit_if_current(
        &self,
        target: WorkTarget,
        commit: JournalCommit,
    ) -> Result<CommitOutcome, Box<dyn std::error::Error + Send + Sync>> {
        let outcome = match &commit {
            JournalCommit::Batch {
                state,
                batch,
                frame,
            } => {
                self.store
                    .commit_journal_batch(&target, state, batch, frame.as_ref(), self.clock.now()?)
                    .await?
            }
            JournalCommit::Incarnation { state, incarnation } => {
                self.store
                    .commit_incarnation_change(&target, state, incarnation, self.clock.now()?)
                    .await?
            }
        };
        if outcome == CommitOutcome::Stale
            && let JournalCommit::Batch {
                state,
                frame: Some(frame),
                ..
            } = commit
        {
            tokio::task::block_in_place(|| {
                self.archive.truncate_uncommitted(&state.stream_id, &frame)
            })?;
        }
        Ok(outcome)
    }
}
