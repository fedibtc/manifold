//! Durable caller-idempotent payout job vocabulary.

use fedi_decentralized_service_fleet_manager::{FederationId, SeatId};

use crate::payout_operation_id::PayoutOperationId;
pub use fman_core::wallet::PayoutRequestId;

/// A native Fedimint Lightning payout committed to the wallet database.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct Payout {
    pub operation_id: PayoutOperationId,
    pub amount_msat: u64,
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
        /// Seat whose separately derived wallet is swept.
        seat_id: SeatId,
        /// Validated public client invite retained so decommission cannot orphan the job.
        #[serde(serialize_with = "serialize_invite_code")]
        invite_code: fedimint_core::invite_code::InviteCode,
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
    /// Time at which FMan linked the native operation to the job.
    pub committed_at_ms: u64,
}

fn serialize_invite_code<Serializer>(
    invite_code: &fedimint_core::invite_code::InviteCode,
    serializer: Serializer,
) -> Result<Serializer::Ok, Serializer::Error>
where
    Serializer: serde::Serializer,
{
    serializer.serialize_str(&invite_code.to_string())
}

#[cfg(test)]
mod tests;

impl PayoutScope {
    pub(crate) fn to_wire(&self) -> fman_core::payout_wire::PayoutScopeWire {
        match self {
            Self::PaymentFederation { federation_id } => {
                fman_core::payout_wire::PayoutScopeWire::PaymentFederation {
                    federation_id: federation_id.clone(),
                }
            }
            Self::GuardianFee {
                federation_id,
                seat_id,
                invite_code,
            } => fman_core::payout_wire::PayoutScopeWire::GuardianFee {
                federation_id: federation_id.clone(),
                seat_id: seat_id.clone(),
                invite_code: fedi_decentralized_service_fleet_manager::InviteCode(
                    invite_code.to_string(),
                ),
            },
        }
    }
}
impl PayoutJob {
    pub(crate) fn to_wire(&self) -> fman_core::payout_wire::PayoutJobWire {
        fman_core::payout_wire::PayoutJobWire {
            request_id: self.request_id.clone(),
            scope: self.scope.to_wire(),
            destination: self.destination.clone(),
            operation: self.operation.as_ref().map(|operation| {
                fman_core::payout_wire::PayoutJobOperationWire {
                    operation_id: operation.operation_id.to_string(),
                    amount_msat: operation.amount_msat,
                    committed_at_ms: operation.committed_at_ms,
                }
            }),
            created_at_ms: self.created_at_ms,
        }
    }
}
