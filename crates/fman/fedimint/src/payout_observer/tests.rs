use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

struct RecordingObserver {
    /// Stable operation set; observation must never add to it.
    operations: Vec<PayoutOperationId>,
    /// Rail returned by every lookup.
    rail: OutgoingRail,
    /// Cached state returned after the await.
    cached_state: OutgoingState,
    /// Direct terminal returned by await.
    terminal: ObservedTerminal,
    /// Independently reported active-state-machine fact.
    active: bool,
    /// Number of durable lookups.
    lookups: AtomicUsize,
    /// Exact operation IDs passed to native await.
    awaits: Mutex<Vec<PayoutOperationId>>,
}

impl RecordingObserver {
    fn operation(&self, operation_id: &PayoutOperationId) -> OutgoingOperation {
        OutgoingOperation::new(
            operation_id.clone(),
            self.rail,
            self.cached_state,
            1_000,
            1_010,
            self.active,
        )
    }
}

#[async_trait::async_trait]
impl PayoutObservation for RecordingObserver {
    async fn status(&self, operation_id: &PayoutOperationId) -> anyhow::Result<OutgoingOperation> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        anyhow::ensure!(self.operations.contains(operation_id), "unknown operation");
        Ok(self.operation(operation_id))
    }

    async fn await_terminal(
        &self,
        operation_id: &PayoutOperationId,
        _rail: OutgoingRail,
    ) -> anyhow::Result<ObservedTerminal> {
        self.awaits.lock().unwrap().push(operation_id.clone());
        Ok(self.terminal)
    }
}

fn observer(
    rail: OutgoingRail,
    cached_state: OutgoingState,
    terminal: ObservedTerminal,
    active: bool,
) -> RecordingObserver {
    RecordingObserver {
        operations: vec![PayoutOperationId::parse(&"01".repeat(32)).unwrap()],
        rail,
        cached_state,
        terminal,
        active,
        lookups: AtomicUsize::new(0),
        awaits: Mutex::new(Vec::new()),
    }
}

#[tokio::test]
async fn repeated_status_and_await_cannot_add_a_payment_operation() {
    let observer = observer(
        OutgoingRail::Lnv2,
        OutgoingState::Unknown,
        ObservedTerminal::Succeeded,
        false,
    );
    let operation_id = observer.operations[0].clone();
    let operation_count = observer.operations.len();

    status_with(&observer, &operation_id).await.unwrap();
    status_with(&observer, &operation_id).await.unwrap();
    await_with(&observer, &operation_id).await.unwrap();
    await_with(&observer, &operation_id).await.unwrap();

    assert_eq!(observer.operations.len(), operation_count);
    assert_eq!(observer.lookups.load(Ordering::SeqCst), 6);
    assert_eq!(
        observer.awaits.lock().unwrap().as_slice(),
        &[operation_id.clone(), operation_id]
    );
}

#[tokio::test]
async fn observed_success_survives_a_missing_cache_and_keeps_active_state_independent() {
    for rail in [OutgoingRail::Lnv1, OutgoingRail::Lnv2] {
        let observer = observer(
            rail,
            OutgoingState::Unknown,
            ObservedTerminal::Succeeded,
            true,
        );
        let result = await_with(&observer, &observer.operations[0])
            .await
            .unwrap();
        assert_eq!(result.state(), OutgoingState::Succeeded);
        assert_eq!(result.encumbered_msat(), Some(0));
        assert!(result.has_active_state_machines());
    }
}

#[tokio::test]
async fn observed_refund_recomputes_encumbrance_from_independent_active_state() {
    for (active, expected) in [(true, Some(1_010)), (false, Some(0))] {
        let observer = observer(
            OutgoingRail::Lnv2,
            OutgoingState::Unknown,
            ObservedTerminal::Refunded,
            active,
        );
        let result = await_with(&observer, &observer.operations[0])
            .await
            .unwrap();
        assert_eq!(result.state(), OutgoingState::FailedOrRefunded);
        assert_eq!(result.encumbered_msat(), expected);
        assert_eq!(result.has_active_state_machines(), active);
    }
}

#[tokio::test]
async fn v2_failure_stays_unknown_and_v1_uses_only_a_cached_refund_distinction() {
    let v2 = observer(
        OutgoingRail::Lnv2,
        OutgoingState::FailedOrRefunded,
        ObservedTerminal::V2Failure,
        false,
    );
    let v2_result = await_with(&v2, &v2.operations[0]).await.unwrap();
    assert_eq!(v2_result.state(), OutgoingState::Unknown);
    assert_eq!(v2_result.encumbered_msat(), None);

    for (cached, expected) in [
        (
            OutgoingState::FailedOrRefunded,
            OutgoingState::FailedOrRefunded,
        ),
        (OutgoingState::Unknown, OutgoingState::Unknown),
    ] {
        let v1 = observer(
            OutgoingRail::Lnv1,
            cached,
            ObservedTerminal::V1Failure,
            false,
        );
        assert_eq!(
            await_with(&v1, &v1.operations[0]).await.unwrap().state(),
            expected
        );
    }
}
