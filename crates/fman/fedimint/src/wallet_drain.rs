//! Operator-facing wallet facts used to decide whether wallet storage may be
//! destroyed.

use crate::payout_operation_id::PayoutOperationId;
use fman_core::wallet::Msats;

/// One native outgoing Lightning operation discovered in a wallet scope.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct OutgoingOperation {
    /// Native Fedimint operation identifier.
    pub operation_id: PayoutOperationId,
    /// Lightning client generation which owns the operation.
    pub rail: OutgoingRail,
    /// Last read-only state that can be established from durable client data.
    state: OutgoingState,
    /// Amount requested by the recipient's invoice.
    pub recipient_amount_msat: u64,
    /// Amount placed in the outgoing contract, including the gateway fee.
    contract_amount_msat: u64,
    /// Contract or refund value known not currently available as ecash, or
    /// `None` when cached state cannot establish the amount.
    encumbered_msat: Option<u64>,
    /// Whether any state machine for this operation remains active.
    has_active_state_machines: bool,
}

impl OutgoingOperation {
    /// Build a projection with encumbrance derived from state and activity.
    pub fn new(
        operation_id: PayoutOperationId,
        rail: OutgoingRail,
        state: OutgoingState,
        recipient_amount_msat: u64,
        contract_amount_msat: u64,
        has_active_state_machines: bool,
    ) -> Self {
        Self {
            operation_id,
            rail,
            state,
            recipient_amount_msat,
            contract_amount_msat,
            encumbered_msat: Self::encumbered(
                state,
                contract_amount_msat,
                has_active_state_machines,
            ),
            has_active_state_machines,
        }
    }

    /// Return the normalized Lightning rail state.
    pub fn state(&self) -> OutgoingState {
        self.state
    }

    /// Return value currently encumbered by this payout when known.
    pub fn encumbered_msat(&self) -> Option<u64> {
        self.encumbered_msat
    }

    /// Return the outgoing contract amount including the gateway fee.
    pub fn contract_amount_msat(&self) -> u64 {
        self.contract_amount_msat
    }

    /// Return whether any state machine for this operation remains active.
    pub fn has_active_state_machines(&self) -> bool {
        self.has_active_state_machines
    }

    /// Replace the rail state and atomically rederive coupled encumbrance.
    pub fn with_state(mut self, state: OutgoingState) -> Self {
        self.state = state;
        self.encumbered_msat = Self::encumbered(
            state,
            self.contract_amount_msat,
            self.has_active_state_machines,
        );
        self
    }

    fn encumbered(state: OutgoingState, contract_amount_msat: u64, active: bool) -> Option<u64> {
        match state {
            OutgoingState::Succeeded => Some(0),
            OutgoingState::FailedOrRefunded if !active => Some(0),
            OutgoingState::FailedOrRefunded => Some(contract_amount_msat),
            OutgoingState::Pending | OutgoingState::Unknown => None,
        }
    }
}

/// Lightning client generation which owns an outgoing operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutgoingRail {
    /// Legacy Lightning client module.
    Lnv1,
    /// Lightning v2 client module.
    Lnv2,
}

/// Read-only state of an outgoing operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutgoingState {
    /// No terminal outcome is cached and state machines remain active.
    Pending,
    /// The payment succeeded; change state machines may still be active.
    Succeeded,
    /// The payment failed or was refunded; refund state machines may remain active.
    FailedOrRefunded,
    /// No state machine is active, but no terminal outcome can be established.
    Unknown,
}

/// A wallet query whose failure prevents a destruction-safe conclusion.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletDrainQuery {
    /// Reading currently available ecash failed.
    AvailableEcash,
    /// Computing a fee-aware recipient amount failed.
    EconomicallySweepable,
    /// Reading or decoding outgoing operation history failed.
    OutgoingOperations,
    /// Repeated reads observed wallet state changing during the query.
    InconsistentSnapshot,
}

/// Fail-closed conclusion about whether wallet storage retains useful work or
/// economically sweepable value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DrainState {
    /// No known payout work remains and no recipient amount is economical.
    Drained,
    /// Available ecash can economically fund a recipient amount.
    Sweepable,
    /// An operation or state machine in the wallet has not fully settled.
    PendingWalletWork,
    /// A query failed or durable operation data cannot establish settlement.
    Unknown,
}

/// Destruction-safety projection for one Fedimint client wallet scope.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct WalletDrainStatus {
    /// Ecash notes currently available to construct a new transaction.
    pub available_ecash_msat: Option<u64>,
    /// Largest recipient amount currently affordable after known fees.
    pub economically_sweepable_recipient_msat: Option<u64>,
    /// Outgoing value absent from available ecash while payment/refund work settles.
    pub encumbered_outgoing_msat: Option<u64>,
    /// Native outgoing operations created by FMan payouts.
    pub outgoing: Option<Vec<OutgoingOperation>>,
    /// Number of active operation IDs in this wallet scope.
    pub active_operation_count: usize,
    /// Queries which could not establish a value.
    pub query_errors: Vec<WalletDrainQuery>,
    /// Conservative destruction-safety conclusion.
    pub drain_state: DrainState,
}

impl WalletDrainStatus {
    /// Build the projection and derive its fail-closed conclusion from facts.
    pub fn new(
        available_ecash: Result<Msats, WalletDrainQuery>,
        economically_sweepable: Result<Msats, WalletDrainQuery>,
        outgoing: Result<Vec<OutgoingOperation>, WalletDrainQuery>,
        active_operation_count: usize,
    ) -> Self {
        let mut query_errors = Vec::new();
        let available_ecash_msat = available_ecash
            .map(|amount| amount.0)
            .map_err(|error| query_errors.push(error))
            .ok();
        let economically_sweepable_recipient_msat = economically_sweepable
            .map(|amount| amount.0)
            .map_err(|error| query_errors.push(error))
            .ok();
        let outgoing = outgoing.map_err(|error| query_errors.push(error)).ok();
        let encumbered_outgoing_msat = outgoing.as_ref().and_then(|operations| {
            operations
                .iter()
                .map(OutgoingOperation::encumbered_msat)
                .sum()
        });

        let has_unknown_operation = outgoing.as_ref().is_some_and(|operations| {
            operations
                .iter()
                .any(|operation| operation.state() == OutgoingState::Unknown)
        });
        let drain_state = if !query_errors.is_empty() || has_unknown_operation {
            DrainState::Unknown
        } else if active_operation_count != 0 {
            DrainState::PendingWalletWork
        } else if economically_sweepable_recipient_msat.is_some_and(|amount| amount != 0) {
            DrainState::Sweepable
        } else {
            DrainState::Drained
        };

        Self {
            available_ecash_msat,
            economically_sweepable_recipient_msat,
            encumbered_outgoing_msat,
            outgoing,
            active_operation_count,
            query_errors,
            drain_state,
        }
    }

    /// Return a fail-closed status when no wallet client can be queried.
    pub fn unavailable() -> Self {
        Self::new(
            Err(WalletDrainQuery::AvailableEcash),
            Err(WalletDrainQuery::EconomicallySweepable),
            Err(WalletDrainQuery::OutgoingOperations),
            0,
        )
    }

    /// Return an unknown status for one projection-wide consistency failure.
    pub fn unknown(error: WalletDrainQuery) -> Self {
        Self {
            available_ecash_msat: None,
            economically_sweepable_recipient_msat: None,
            encumbered_outgoing_msat: None,
            outgoing: None,
            active_operation_count: 0,
            query_errors: vec![error],
            drain_state: DrainState::Unknown,
        }
    }
}

#[cfg(test)]
mod tests;

impl OutgoingOperation {
    pub(crate) fn to_wire(&self) -> fman_core::payout_wire::OutgoingOperationWire {
        fman_core::payout_wire::OutgoingOperationWire {
            operation_id: self.operation_id.to_string(),
            rail: match self.rail {
                OutgoingRail::Lnv1 => fman_core::payout_wire::OutgoingRailWire::Lnv1,
                OutgoingRail::Lnv2 => fman_core::payout_wire::OutgoingRailWire::Lnv2,
            },
            state: match self.state {
                OutgoingState::Pending => fman_core::payout_wire::OutgoingStateWire::Pending,
                OutgoingState::Succeeded => fman_core::payout_wire::OutgoingStateWire::Succeeded,
                OutgoingState::FailedOrRefunded => {
                    fman_core::payout_wire::OutgoingStateWire::FailedOrRefunded
                }
                OutgoingState::Unknown => fman_core::payout_wire::OutgoingStateWire::Unknown,
            },
            recipient_amount_msat: self.recipient_amount_msat,
            contract_amount_msat: self.contract_amount_msat,
            encumbered_msat: self.encumbered_msat,
            has_active_state_machines: self.has_active_state_machines,
        }
    }
}
impl WalletDrainStatus {
    pub(crate) fn to_wire(&self) -> fman_core::payout_wire::WalletDrainStatusWire {
        fman_core::payout_wire::WalletDrainStatusWire {
            available_ecash_msat: self.available_ecash_msat,
            economically_sweepable_recipient_msat: self.economically_sweepable_recipient_msat,
            encumbered_outgoing_msat: self.encumbered_outgoing_msat,
            outgoing: self
                .outgoing
                .as_ref()
                .map(|operations| operations.iter().map(OutgoingOperation::to_wire).collect()),
            active_operation_count: self.active_operation_count,
            query_errors: self
                .query_errors
                .iter()
                .map(|error| match error {
                    WalletDrainQuery::AvailableEcash => {
                        fman_core::payout_wire::WalletDrainQueryWire::AvailableEcash
                    }
                    WalletDrainQuery::EconomicallySweepable => {
                        fman_core::payout_wire::WalletDrainQueryWire::EconomicallySweepable
                    }
                    WalletDrainQuery::OutgoingOperations => {
                        fman_core::payout_wire::WalletDrainQueryWire::OutgoingOperations
                    }
                    WalletDrainQuery::InconsistentSnapshot => {
                        fman_core::payout_wire::WalletDrainQueryWire::InconsistentSnapshot
                    }
                })
                .collect(),
            drain_state: match self.drain_state {
                DrainState::Drained => fman_core::payout_wire::DrainStateWire::Drained,
                DrainState::Sweepable => fman_core::payout_wire::DrainStateWire::Sweepable,
                DrainState::PendingWalletWork => {
                    fman_core::payout_wire::DrainStateWire::PendingWalletWork
                }
                DrainState::Unknown => fman_core::payout_wire::DrainStateWire::Unknown,
            },
        }
    }
}
