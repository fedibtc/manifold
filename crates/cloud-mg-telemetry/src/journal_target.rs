//! Private target tokens used only by safe-journal collection.

use fedi_decentralized_service_manifold_guardian::TelemetryCapability;

/// Non-secret target snapshot used to schedule a journal poll.
pub(crate) struct CollectionTarget {
    /// Collector-owned opaque target id.
    pub(crate) target_id: String,
    /// Revision observed while scheduling.
    pub(crate) registration_revision: u64,
}

/// Secret-bearing journal work token resolved after checking its target fence.
///
/// This type deliberately does not implement `Debug` because it contains a bearer.
pub(crate) struct WorkTarget {
    target_id: String,
    registration_revision: u64,
    endpoint_id: String,
    capability: TelemetryCapability,
    mg_id: String,
    mg_name: String,
}

impl WorkTarget {
    pub(crate) fn new(
        target_id: String,
        registration_revision: u64,
        endpoint_id: String,
        capability: TelemetryCapability,
        mg_id: String,
        mg_name: String,
    ) -> Self {
        Self {
            target_id,
            registration_revision,
            endpoint_id,
            capability,
            mg_id,
            mg_name,
        }
    }

    pub(crate) fn target_id(&self) -> &str {
        &self.target_id
    }

    pub(crate) fn registration_revision(&self) -> u64 {
        self.registration_revision
    }

    pub(crate) fn endpoint_id(&self) -> &str {
        &self.endpoint_id
    }

    pub(crate) fn capability(&self) -> &TelemetryCapability {
        &self.capability
    }

    pub(crate) fn mg_id(&self) -> &str {
        &self.mg_id
    }

    pub(crate) fn mg_name(&self) -> &str {
        &self.mg_name
    }
}

/// Result of a revision-fenced journal mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommitOutcome {
    /// Archive state and its source cursor committed together.
    Committed,
    /// Lease, status, or revision changed and the output was discarded.
    Stale,
}
