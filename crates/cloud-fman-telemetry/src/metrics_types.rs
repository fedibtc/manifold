//! Data exchanged between metrics admission, polling, persistence, and exposition.

use std::collections::BTreeSet;

/// One admitted seat observation. Target identity comes only from the fenced work item.
pub(crate) struct SeatObservation {
    pub(crate) guardian_seat_id: String,
    /// Canonical federation id derived from the discovered seat invite.
    pub(crate) federation_id: String,
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
