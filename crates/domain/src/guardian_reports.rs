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
    /// Peers each status reporter explicitly reports disconnected, keyed by the
    /// peer that answered. Missing replies are not included.
    pub disconnected: BTreeMap<PeerId, BTreeSet<PeerId>>,
    /// Newer availability observations; old servers simply have no entry.
    pub availability: BTreeMap<PeerId, PeerAvailability>,
}
