//! Exact-operation payout observation, deliberately without payout-start authority.

use std::str::FromStr as _;

use crate::payout_operation_id::PayoutOperationId;
use crate::wallet_drain::{OutgoingOperation, OutgoingRail, OutgoingState};
use anyhow::Context as _;
use fedimint_client::ClientHandleArc;
use fedimint_core::core::OperationId;
use fedimint_ln_client::{LightningClientModule, LightningPaymentOutcome};
use fedimint_lnv2_client::{
    FinalSendOperationState, LightningClientModule as LightningV2ClientModule,
};

/// Terminal rail fact observed directly from a native await subscription.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObservedTerminal {
    /// Either rail proved payment success.
    Succeeded,
    /// Lightning v2 proved a completed refund.
    Refunded,
    /// Lightning v1 reported aggregate failure; cached state may distinguish refund.
    V1Failure,
    /// Lightning v2 reached its dependency-defined failure state.
    V2Failure,
}

/// Authority available to exact-operation orchestration.
///
/// This capability intentionally has no destination, invoice, or payment-start
/// operation.
#[async_trait::async_trait]
trait PayoutObservation: Sync {
    /// Read durable metadata, cached rail state, and active state machines.
    async fn status(&self, operation_id: &PayoutOperationId) -> anyhow::Result<OutgoingOperation>;

    /// Await the selected existing operation's native rail terminal.
    async fn await_terminal(
        &self,
        operation_id: &PayoutOperationId,
        rail: OutgoingRail,
    ) -> anyhow::Result<ObservedTerminal>;
}

/// Fedimint-backed exact-operation authority.
struct FedimintPayoutObserver<'a> {
    /// Wallet client whose existing operation is being observed.
    client: &'a ClientHandleArc,
}

impl FedimintPayoutObserver<'_> {
    /// Convert the validated domain identity to Fedimint's native type.
    fn native_id(operation_id: &PayoutOperationId) -> OperationId {
        OperationId::from_str(operation_id.as_str())
            .expect("PayoutOperationId validates Fedimint's native encoding")
    }
}

#[async_trait::async_trait]
impl PayoutObservation for FedimintPayoutObserver<'_> {
    async fn status(&self, operation_id: &PayoutOperationId) -> anyhow::Result<OutgoingOperation> {
        crate::drain_status::payout_status(self.client, Self::native_id(operation_id), None, None)
            .await
    }

    async fn await_terminal(
        &self,
        operation_id: &PayoutOperationId,
        rail: OutgoingRail,
    ) -> anyhow::Result<ObservedTerminal> {
        let operation_id = Self::native_id(operation_id);
        match rail {
            OutgoingRail::Lnv1 => {
                let lightning = self
                    .client
                    .get_first_module::<LightningClientModule>()
                    .context("federation has no Lightning v1 module")?;
                Ok(
                    match lightning.await_outgoing_payment(operation_id).await? {
                        LightningPaymentOutcome::Success { .. } => ObservedTerminal::Succeeded,
                        LightningPaymentOutcome::Failure { .. } => ObservedTerminal::V1Failure,
                    },
                )
            }
            OutgoingRail::Lnv2 => {
                let lightning = self
                    .client
                    .get_first_module::<LightningV2ClientModule>()
                    .context("federation has no Lightning v2 module")?;
                Ok(
                    match lightning
                        .await_final_send_operation_state(operation_id)
                        .await?
                    {
                        FinalSendOperationState::Success(_) => ObservedTerminal::Succeeded,
                        FinalSendOperationState::Refunded => ObservedTerminal::Refunded,
                        FinalSendOperationState::Failure => ObservedTerminal::V2Failure,
                    },
                )
            }
        }
    }
}

/// Read one exact payout through the observation-only production seam.
#[cfg(test)]
async fn status_with(
    observer: &dyn PayoutObservation,
    operation_id: &PayoutOperationId,
) -> anyhow::Result<OutgoingOperation> {
    observer.status(operation_id).await
}

/// Await one exact payout and merge the observed terminal with best-effort
/// cached state while retaining the independently reread active-state fact.
async fn await_with(
    observer: &dyn PayoutObservation,
    operation_id: &PayoutOperationId,
) -> anyhow::Result<OutgoingOperation> {
    let before = observer.status(operation_id).await?;
    let terminal = observer.await_terminal(operation_id, before.rail).await?;
    let after = observer.status(operation_id).await?;
    anyhow::ensure!(
        before.rail == after.rail,
        "native payout rail changed while awaiting"
    );
    let state = match terminal {
        ObservedTerminal::Succeeded => OutgoingState::Succeeded,
        ObservedTerminal::Refunded => OutgoingState::FailedOrRefunded,
        ObservedTerminal::V1Failure if after.state() == OutgoingState::FailedOrRefunded => {
            OutgoingState::FailedOrRefunded
        }
        ObservedTerminal::V1Failure | ObservedTerminal::V2Failure => OutgoingState::Unknown,
    };
    Ok(after.with_state(state))
}

/// Await one exact payout through a Fedimint observation-only capability.
pub(crate) async fn await_terminal(
    client: &ClientHandleArc,
    operation_id: &PayoutOperationId,
) -> anyhow::Result<OutgoingOperation> {
    await_with(&FedimintPayoutObserver { client }, operation_id).await
}

#[cfg(test)]
mod tests;
