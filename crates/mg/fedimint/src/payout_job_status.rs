//! Operator projection of a durable payout job and its native state.

use crate::payout_job::PayoutJob;
use crate::wallet_drain::OutgoingOperation;

/// Durable job information with the current native operation projection.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PayoutJobStatus {
    /// Durable request and native-operation identity.
    pub job: PayoutJob,
    /// Current native state, by the job's method.
    pub native: PayoutNativeStatus,
}

/// The native projection of one job. Each variant is `None` only before an
/// operation has committed.
#[derive(Clone, Debug)]
pub enum PayoutNativeStatus {
    Lightning(Option<OutgoingOperation>),
    Ecash(Option<EcashPayoutState>),
}

impl PayoutJobStatus {
    pub(crate) fn to_wire(&self) -> mg_core::payout_wire::PayoutJobStatusWire {
        mg_core::payout_wire::PayoutJobStatusWire {
            job: self.job.to_wire(),
            payout: self
                .payout
                .as_ref()
                .map(crate::wallet_drain::OutgoingOperation::to_wire),
        }
    }
}
