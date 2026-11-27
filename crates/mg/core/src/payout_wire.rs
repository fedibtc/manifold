//! Stable operator-facing payout and wallet-drain response vocabulary.

use fedi_decentralized_service_manifold_guardian::{FederationId, InviteCode, SeatId};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct DestinationCapWire {
    pub maximum_msat: u64,
    pub remaining_msat: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PayoutJobOperationWire {
    pub operation_id: String,
    pub amount_msat: u64,
    pub capped: Option<DestinationCapWire>,
    pub committed_at_ms: u64,
    /// When the operator said an ecash payout's token was redeemed: their
    /// word, not the federation's, so a later cancel is still accepted.
    /// Always null for a Lightning payout.
    pub confirmed_at_ms: Option<u64>,
}

/// How a payout leaves the wallet, fixed when its request is first recorded.
///
/// A Lightning job serializes as `method: "lightning"` plus the destination
/// snapshot it was bound to; an ecash job carries no destination, because the
/// bearer token is handed to the operator instead.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum PayoutMethodWire {
    Lightning { destination: String },
    Ecash,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PayoutJobWire {
    pub request_id: PayoutRequestId,
    pub scope: PayoutScopeWire,
    #[serde(flatten)]
    pub method: PayoutMethodWire,
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
    /// The wallet is unreadable because its client is recovering.
    pub recovering: bool,
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
            recovering: false,
        }
    }

    /// Unreadable because the wallet's recovery scan is still running.
    pub fn recovering() -> Self {
        Self {
            recovering: true,
            ..Self::unavailable()
        }
    }
}

/// One job with its native state.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PayoutJobStatusWire {
    pub job: PayoutJobWire,
    #[serde(flatten)]
    pub native: PayoutNativeStatusWire,
}

/// The native projection of a job, keyed by its method: `payout` for a
/// Lightning job, `ecash` for an ecash job. Either is null before the job has
/// a committed native operation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(untagged)]
pub enum PayoutNativeStatusWire {
    Lightning {
        payout: Option<OutgoingOperationWire>,
    },
    Ecash {
        ecash: Option<EcashPayoutStateWire>,
    },
}

/// Where an ecash payout's notes are, as the wallet database reads it. The
/// operator's confirmation is not a state: it is `confirmed_at_ms` on the
/// job's operation, and an issued payout without it is still open.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EcashPayoutStateWire {
    /// The notes left this wallet and nothing has reclaimed them. `token` is
    /// the bearer token in a single-payout answer, and null in a list: answers
    /// that are polled never carry it.
    Issued { token: Option<EcashTokenWire> },
    /// A reclaim of the notes is submitted and not yet terminal.
    Reclaiming,
    /// The reclaim succeeded: the notes are back in this wallet.
    Reclaimed,
    /// The federation refused the reclaim, almost always because the notes'
    /// inputs were already spent: the token was redeemed first. `token` is
    /// carried as in `Issued`, so notes a refusal for another reason left
    /// behind stay reachable.
    AlreadySpent { token: Option<EcashTokenWire> },
}

/// A bearer ecash token returned to the authorized operator. Whoever holds the
/// string can redeem the notes, so `Debug` redacts it and nothing formats it.
#[derive(Clone, Eq, PartialEq, serde::Serialize)]
#[serde(transparent)]
pub struct EcashTokenWire(pub String);

impl std::fmt::Debug for EcashTokenWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EcashTokenWire(<redacted>)")
    }
}

/// Payouts of both methods, newest first, without tokens: the newest jobs,
/// plus every older ecash job whose token is issued or whose reclaim runs.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PayoutsWire {
    pub payouts: Vec<PayoutJobStatusWire>,
}
