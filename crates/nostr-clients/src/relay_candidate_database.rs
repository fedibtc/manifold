//! Stateless SDK database that preserves every verified relay candidate.

#[cfg(test)]
mod tests;

use nostr_sdk::prelude::{
    Backend, BoxedFuture, DatabaseError, DatabaseEventStatus, Event, EventId, Events, Filter,
    NostrDatabase, SaveEventStatus,
};

/// SDK database that applies no NIP-01 replacement or deletion suppression.
///
/// Role clients perform semantic admission after bounded relay collection. The
/// SDK must therefore deliver an older valid addressable event even if the same
/// query first observes a newer event that is semantically invalid for the role.
#[derive(Debug)]
pub(crate) struct RelayCandidateDatabase;

impl NostrDatabase for RelayCandidateDatabase {
    fn backend(&self) -> Backend {
        Backend::Custom("stateless-relay-candidates".to_owned())
    }

    fn save_event<'a>(
        &'a self,
        _event: &'a Event,
    ) -> BoxedFuture<'a, Result<SaveEventStatus, DatabaseError>> {
        Box::pin(async { Ok(SaveEventStatus::Success) })
    }

    fn check_id<'a>(
        &'a self,
        _event_id: &'a EventId,
    ) -> BoxedFuture<'a, Result<DatabaseEventStatus, DatabaseError>> {
        Box::pin(async { Ok(DatabaseEventStatus::NotExistent) })
    }

    fn event_by_id<'a>(
        &'a self,
        _event_id: &'a EventId,
    ) -> BoxedFuture<'a, Result<Option<Event>, DatabaseError>> {
        Box::pin(async { Ok(None) })
    }

    fn count(&self, _filter: Filter) -> BoxedFuture<'_, Result<usize, DatabaseError>> {
        Box::pin(async { Ok(0) })
    }

    fn query(&self, filter: Filter) -> BoxedFuture<'_, Result<Events, DatabaseError>> {
        Box::pin(async move { Ok(Events::new(&filter)) })
    }

    fn delete(&self, _filter: Filter) -> BoxedFuture<'_, Result<(), DatabaseError>> {
        Box::pin(async { Ok(()) })
    }

    fn wipe(&self) -> BoxedFuture<'_, Result<(), DatabaseError>> {
        Box::pin(async { Ok(()) })
    }
}
