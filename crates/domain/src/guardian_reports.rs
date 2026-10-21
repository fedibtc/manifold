//! Unmerged guardian observations. Only the reporting peer supplies each row.

use std::collections::{BTreeMap, BTreeSet};

use fedimint_core::PeerId;
use fedimint_core::peer_availability::PeerAvailability;

use crate::FederationId;

/// Responses to one bounded read, including peers that did not answer.
#[derive(Clone, Debug)]
pub struct GuardianReports {
    /// Federation authenticated by the invite/config download.
    pub federation_id: FederationId,
    /// Complete peer set from that config, not just respondents.
    pub peers: BTreeSet<PeerId>,
    /// Legacy status observations, keyed by the peer that answered.
    pub sessions: BTreeMap<PeerId, GuardianSessionReport>,
    /// Newer availability observations; old servers simply have no entry.
    pub availability: BTreeMap<PeerId, PeerAvailability>,
}

/// One guardian's view of consensus participation.
#[derive(Clone, Debug)]
pub struct GuardianSessionReport {
    /// Number of sessions completed by this reporter.
    pub session_count: u64,
    /// Last contribution per peer. None means unknown, never session zero.
    pub last_contribution: BTreeMap<PeerId, Option<u64>>,
}
