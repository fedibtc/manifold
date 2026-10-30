//! Data exchanged between metrics admission, polling, persistence, and exposition.

use std::collections::BTreeSet;

/// One admitted seat observation. Target identity comes only from the fenced work item.
pub(crate) struct SeatObservation {
    pub(crate) guardian_seat_id: String,
    /// Canonical federation id derived from the discovered seat invite.
    pub(crate) federation_id: String,
    /// Canonical form of the invite the authenticated MG asserted for this seat,
    /// absent when that invite has no canonical form.
    pub(crate) invite_code: Option<String>,
    pub(crate) observed_at_ms: i64,
    pub(crate) samples: Vec<String>,
}

/// One revision-fenced durable mutation from a metrics attempt.
pub(crate) struct MetricsCommit {
    /// Authoritative seats when discovery succeeded; `None` retains prior seats.
    pub(crate) listed_seats: Option<BTreeSet<String>>,
    /// Independently successful fresh seat observations.
    pub(crate) snapshots: Vec<SeatObservation>,
    /// Whether discovery and every advertised seat scrape succeeded.
    pub(crate) complete: bool,
}

/// Result of one revision-fenced metrics commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MetricsCommitOutcome {
    /// Every observation was stored.
    Committed,
    /// The commit was applied, but at least one observation did not fit the
    /// persisted-state cap and that seat kept its previous snapshot.
    Saturated,
    /// Lease, status, or revision changed and the output was discarded.
    Stale,
}
