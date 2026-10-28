//! Owner-authorized terminal FA seat release in every environment.
//!
//! The FA forfeits its own seat without a refund. Decommission retains the
//! seat records and guardian data, exactly as operator decommission does
//! ([`ARCH-manifold-guardian-product-boundary`](../../mg/specs/ARCH-manifold-guardian-product-boundary.md)).
//! Older production daemons can refuse this verb with
//! [`crate::ManifoldGuardianError::UnsupportedVerb`].

use crate::{FaId, SeatId, Timestamp};

/// Request to release the FA's own seat, ending it exactly as an operator
/// decommission would.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DecommissionSeatRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Ambassador identity; must own the named seat.
    #[serde(rename = "fi_id")]
    pub fa_id: FaId,

    /// Seat to release.
    pub seat_id: SeatId,
}

/// Outcome of a release. Terminal and idempotent: a repeat call on an
/// already-released seat succeeds with `already_decommissioned: true` rather
/// than failing, so a retrying FA never has to distinguish the two.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct DecommissionSeatResponse {
    /// Whether the seat was already terminal before this call.
    pub already_decommissioned: bool,
}
