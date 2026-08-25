//! Shared mutation passed from one journal fetch to its fenced catalog commit.

use fedi_decentralized_service_fleet_manager::SafeEventJournalIncarnation;

use crate::{
    journal_types::ValidatedJournalBatch,
    store::{ArchiveFrame, JournalStreamState},
};

pub(crate) enum JournalCommit {
    Batch {
        state: JournalStreamState,
        batch: ValidatedJournalBatch,
        frame: Option<ArchiveFrame>,
    },
    Incarnation {
        state: JournalStreamState,
        incarnation: SafeEventJournalIncarnation,
    },
}

impl JournalCommit {
    pub(crate) fn ends_current_drain(&self) -> bool {
        match self {
            Self::Incarnation { .. } => true,
            Self::Batch { batch, .. } => batch.is_empty(),
        }
    }
}
