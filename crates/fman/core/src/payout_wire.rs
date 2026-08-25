//! Stable operator-facing payout and wallet-drain response vocabulary.

use fedi_decentralized_service_fleet_manager::{FederationId, InviteCode, SeatId};

use crate::wallet::PayoutRequestId;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PayoutScopeWire {
    PaymentFederation {
        federation_id: FederationId,
    },
    GuardianFee {
        federation_id: FederationId,
        seat_id: SeatId,
        invite_code: InviteCode,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PayoutJobOperationWire {
    pub operation_id: String,
    pub amount_msat: u64,
    pub committed_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PayoutJobWire {
    pub request_id: PayoutRequestId,
    pub scope: PayoutScopeWire,
    pub destination: String,
    pub operation: Option<PayoutJobOperationWire>,
    pub created_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutgoingRailWire {
    Lnv1,
    Lnv2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutgoingStateWire {
    Pending,
    Succeeded,
    FailedOrRefunded,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct OutgoingOperationWire {
    pub operation_id: String,
    pub rail: OutgoingRailWire,
    pub state: OutgoingStateWire,
    pub recipient_amount_msat: u64,
    pub contract_amount_msat: u64,
    pub encumbered_msat: Option<u64>,
    pub has_active_state_machines: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletDrainQueryWire {
    AvailableEcash,
    EconomicallySweepable,
    OutgoingOperations,
    InconsistentSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DrainStateWire {
    Drained,
    Sweepable,
    PendingWalletWork,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct WalletDrainStatusWire {
    pub available_ecash_msat: Option<u64>,
    pub economically_sweepable_recipient_msat: Option<u64>,
    pub encumbered_outgoing_msat: Option<u64>,
    pub outgoing: Option<Vec<OutgoingOperationWire>>,
    pub active_operation_count: usize,
    pub query_errors: Vec<WalletDrainQueryWire>,
    pub drain_state: DrainStateWire,
}

impl WalletDrainStatusWire {
    pub fn unavailable() -> Self {
        Self {
            available_ecash_msat: None,
            economically_sweepable_recipient_msat: None,
            encumbered_outgoing_msat: None,
            outgoing: None,
            active_operation_count: 0,
            query_errors: vec![
                WalletDrainQueryWire::AvailableEcash,
                WalletDrainQueryWire::EconomicallySweepable,
                WalletDrainQueryWire::OutgoingOperations,
            ],
            drain_state: DrainStateWire::Unknown,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PayoutJobStatusWire {
    pub job: PayoutJobWire,
    pub payout: Option<OutgoingOperationWire>,
}
