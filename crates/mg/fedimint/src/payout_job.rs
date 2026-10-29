//! Durable caller-idempotent payout job vocabulary.

use fedi_decentralized_service_manifold_guardian::{FederationId, SeatId};

use crate::payout_operation_id::PayoutOperationId;
pub use mg_core::wallet::PayoutRequestId;

/// A native Fedimint Lightning payout committed to the wallet database.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct Payout {
    pub operation_id: PayoutOperationId,
    pub amount_msat: u64,
    pub capped: Option<DestinationCap>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct DestinationCap {
    pub maximum_msat: u64,
    pub remaining_msat: u64,
}

/// The exact wallet scope a payout job owns.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PayoutScope {
    /// One setup-payment federation wallet.
    PaymentFederation {
        /// Federation whose payment wallet is swept.
        federation_id: FederationId,
    },
    /// One seat's guardian-fee wallet.
    GuardianFee {
        /// Federation guarded by the seat when the request was created.
        federation_id: FederationId,
        /// Seat whose separately derived wallet is swept. Joining that wallet
        /// for the first time takes the seat's invite, built from its
        /// installed config; an already joined wallet needs none, so
        /// decommission cannot orphan the job.
        seat_id: SeatId,
    },
}

/// One immutable durable payout request and its optional committed operation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PayoutJob {
    /// Caller-generated idempotency identity.
    pub request_id: PayoutRequestId,
    /// Wallet scope permanently bound to the identity.
    pub scope: PayoutScope,
    /// Destination snapshot permanently bound to the identity.
    pub destination: String,
    /// Native operation recorded after its wallet commit.
    pub operation: Option<PayoutJobOperation>,
    /// Time at which the durable request row was created.
    pub created_at_ms: u64,
}

/// Native wallet operation committed for a payout job.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PayoutJobOperation {
    /// Native Fedimint operation identity.
    pub operation_id: PayoutOperationId,
    /// Amount requested by the destination invoice.
    pub amount_msat: u64,
    pub capped: Option<DestinationCap>,
    /// Time at which MG linked the native operation to the job.
    pub committed_at_ms: u64,
}

#[cfg(test)]
mod tests;

impl PayoutScope {
    pub(crate) fn to_wire(&self) -> mg_core::payout_wire::PayoutScopeWire {
        match self {
            Self::PaymentFederation { federation_id } => {
                mg_core::payout_wire::PayoutScopeWire::PaymentFederation {
                    federation_id: federation_id.clone(),
                }
            }
            Self::GuardianFee {
                federation_id,
                seat_id,
            } => mg_core::payout_wire::PayoutScopeWire::GuardianFee {
                federation_id: federation_id.clone(),
                seat_id: seat_id.clone(),
            },
        }
    }
}
impl PayoutJob {
    pub(crate) fn to_wire(&self) -> mg_core::payout_wire::PayoutJobWire {
        mg_core::payout_wire::PayoutJobWire {
            request_id: self.request_id.clone(),
            scope: self.scope.to_wire(),
            destination: self.destination.clone(),
            operation: self.operation.as_ref().map(|operation| {
                mg_core::payout_wire::PayoutJobOperationWire {
                    operation_id: operation.operation_id.to_string(),
                    amount_msat: operation.amount_msat,
                    capped: operation
                        .capped
                        .map(|cap| mg_core::payout_wire::DestinationCapWire {
                            maximum_msat: cap.maximum_msat,
                            remaining_msat: cap.remaining_msat,
                        }),
                    committed_at_ms: operation.committed_at_ms,
                }
            }),
            created_at_ms: self.created_at_ms,
        }
    }
}
