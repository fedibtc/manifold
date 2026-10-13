//! Owner-authorized terminal FI seat release in every environment.
//!
//! The FI forfeits its own seat without a refund. Decommission retains the
//! seat records and guardian data, exactly as operator decommission does
//! ([`ARCH-fleet-manager-product-boundary`](../../fman/specs/ARCH-fleet-manager-product-boundary.md)).
//! Older production daemons can refuse this verb with
//! [`crate::FleetManagerError::UnsupportedVerb`].

use crate::{FiId, SeatId, Timestamp};

/// Request to release the FI's own seat, ending it exactly as an operator
/// decommission would.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DecommissionSeatRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity; must own the named seat.
    pub fi_id: FiId,

    /// Seat to release.
    pub seat_id: SeatId,
}

/// Outcome of a release. Terminal and idempotent: a repeat call on an
/// already-released seat succeeds with `already_decommissioned: true` rather
/// than failing, so a retrying FI never has to distinguish the two.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct DecommissionSeatResponse {
    /// Whether the seat was already terminal before this call.
    pub already_decommissioned: bool,
}
