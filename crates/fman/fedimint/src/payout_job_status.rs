//! Operator projection of a durable payout job and its native state.

use crate::payout_job::PayoutJob;
use crate::wallet_drain::OutgoingOperation;

/// Durable job information with the current native operation projection.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PayoutJobStatus {
    /// Durable request and native-operation identity.
    pub job: PayoutJob,
    /// Current native state, absent only before an operation has committed.
    pub payout: Option<OutgoingOperation>,
}

impl PayoutJobStatus {
    pub(crate) fn to_wire(&self) -> fman_core::payout_wire::PayoutJobStatusWire {
        fman_core::payout_wire::PayoutJobStatusWire {
            job: self.job.to_wire(),
            payout: self
                .payout
                .as_ref()
                .map(crate::wallet_drain::OutgoingOperation::to_wire),
        }
    }
}
