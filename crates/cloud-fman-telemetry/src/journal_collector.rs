//! One authenticated safe-journal fetch, validation, and durable frame append.

use std::sync::Arc;

use fedi_decentralized_service_fleet_manager::FetchSafeEventJournalResponse;

use crate::{
    archive::{ArchiveError, JournalArchive},
    iroh_journal_source::JournalSession,
    journal_commit::JournalCommit,
    journal_poller::{Clock, PollError},
    journal_types::{ReceptionDay, ValidatedJournalBatch},
    store::JournalStreamState,
};

pub(crate) struct SingleBatchCollector {
    archive: JournalArchive,
    session: Arc<tokio::sync::Mutex<Box<dyn JournalSession>>>,
    state: JournalStreamState,
    clock: Arc<dyn Clock>,
    deadline: tokio::time::Instant,
}

impl SingleBatchCollector {
    pub(crate) fn new(
        archive: JournalArchive,
        session: Arc<tokio::sync::Mutex<Box<dyn JournalSession>>>,
        state: JournalStreamState,
        clock: Arc<dyn Clock>,
        deadline: tokio::time::Instant,
    ) -> Self {
        Self {
            archive,
            session,
            state,
            clock,
            deadline,
        }
    }
    pub(crate) async fn collect_journals(
        &self,
    ) -> Result<JournalCommit, Box<dyn std::error::Error + Send + Sync>> {
        let response = tokio::time::timeout_at(self.deadline, async {
            self.session.lock().await.fetch(&self.state).await
        })
        .await
        .map_err(|_| PollError::Transient)??;
        match response {
            FetchSafeEventJournalResponse::IncarnationChanged { incarnation } => {
                if incarnation == self.state.incarnation {
                    return Err(Box::new(PollError::Transient));
                }
                Ok(JournalCommit::Incarnation {
                    state: self.state.clone(),
                    incarnation,
                })
            }
            FetchSafeEventJournalResponse::Current {
                incarnation,
                jsonl,
                next_cursor,
                continuity_gap,
            } => {
                let batch = ValidatedJournalBatch::new(
                    &self.state.incarnation,
                    self.state.cursor.as_ref(),
                    incarnation,
                    jsonl,
                    next_cursor,
                    continuity_gap,
                )
                .map_err(|_| PollError::Transient)?;
                let frame = if batch.is_empty() {
                    None
                } else {
                    let day = ReceptionDay::from_unix_seconds(self.clock.now()?)
                        .map_err(|_| PollError::Fatal("journal reception date is out of range"))?;
                    Some(
                        tokio::task::block_in_place(|| {
                            self.archive.append(&self.state.stream_id, &day, &batch)
                        })
                        .map_err(|error| match error {
                            ArchiveError::Capacity => PollError::Capacity,
                            _ => PollError::Fatal("journal archive append failed"),
                        })?,
                    )
                };
                Ok(JournalCommit::Batch {
                    state: self.state.clone(),
                    batch,
                    frame,
                })
            }
        }
    }
}
