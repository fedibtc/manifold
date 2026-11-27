//! Operator projection of a durable payout job and its native state.

use crate::payout_ecash::EcashPayoutState;
use crate::payout_job::PayoutJob;
use crate::wallet_drain::OutgoingOperation;

/// Durable job information with the current native operation projection.
#[derive(Clone, Debug)]
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
    /// The operator answer. Only a single-payout answer is `with_token`; a
    /// list, which the dashboard polls, never carries it.
    pub(crate) fn to_wire(&self, with_token: bool) -> mg_core::payout_wire::PayoutJobStatusWire {
        use mg_core::payout_wire::PayoutNativeStatusWire;
        mg_core::payout_wire::PayoutJobStatusWire {
            job: self.job.to_wire(),
            native: match &self.native {
                PayoutNativeStatus::Lightning(payout) => PayoutNativeStatusWire::Lightning {
                    payout: payout.as_ref().map(OutgoingOperation::to_wire),
                },
                PayoutNativeStatus::Ecash(state) => PayoutNativeStatusWire::Ecash {
                    ecash: state.as_ref().map(|state| state.to_wire(with_token)),
                },
            },
        }
    }
}
