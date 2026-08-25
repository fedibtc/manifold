use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use fedimint_core::Amount;
use fman_core::guardian_fee::{Collected, CollectionFailure, CollectionFailurePhase};
use stability_pool_client::StabilityPoolWithdrawalOperationState;

use super::{CollectionBalances, CollectionOperations, collect_operations, terminal_withdrawal};

struct FakeOperations {
    balances: Mutex<VecDeque<anyhow::Result<CollectionBalances>>>,
    idle_submit: Mutex<VecDeque<anyhow::Result<u8>>>,
    idle_await: Mutex<VecDeque<anyhow::Result<Amount>>>,
    unlock_submit: Mutex<VecDeque<anyhow::Result<u8>>>,
    unlock_await: Mutex<VecDeque<anyhow::Result<Amount>>>,
    idle_submit_calls: AtomicUsize,
    unlock_submit_calls: AtomicUsize,
}

impl FakeOperations {
    fn new(balances: Vec<anyhow::Result<CollectionBalances>>) -> Self {
        Self {
            balances: Mutex::new(balances.into()),
            idle_submit: Mutex::default(),
            idle_await: Mutex::default(),
            unlock_submit: Mutex::default(),
            unlock_await: Mutex::default(),
            idle_submit_calls: AtomicUsize::new(0),
            unlock_submit_calls: AtomicUsize::new(0),
        }
    }

    fn idle(self, submit: anyhow::Result<u8>, complete: Option<anyhow::Result<Amount>>) -> Self {
        self.idle_submit.lock().unwrap().push_back(submit);
        self.idle_await.lock().unwrap().extend(complete);
        self
    }

    fn unlock(self, submit: anyhow::Result<u8>, complete: Option<anyhow::Result<Amount>>) -> Self {
        self.unlock_submit.lock().unwrap().push_back(submit);
        self.unlock_await.lock().unwrap().extend(complete);
        self
    }
}

#[async_trait::async_trait]
impl CollectionOperations for FakeOperations {
    type Operation = u8;

    async fn balances(&self) -> anyhow::Result<CollectionBalances> {
        self.balances
            .lock()
            .unwrap()
            .pop_front()
            .expect("test supplied a balance result")
    }

    async fn submit_idle(&self, _amount: Amount) -> anyhow::Result<Self::Operation> {
        self.idle_submit_calls.fetch_add(1, Ordering::Relaxed);
        self.idle_submit
            .lock()
            .unwrap()
            .pop_front()
            .expect("test supplied an idle submission result")
    }

    async fn await_idle(&self, _operation: Self::Operation) -> anyhow::Result<Amount> {
        self.idle_await
            .lock()
            .unwrap()
            .pop_front()
            .expect("test supplied an idle completion result")
    }

    async fn submit_unlock(&self) -> anyhow::Result<Self::Operation> {
        self.unlock_submit_calls.fetch_add(1, Ordering::Relaxed);
        self.unlock_submit
            .lock()
            .unwrap()
            .pop_front()
            .expect("test supplied an unlock submission result")
    }

    async fn await_unlock(&self, _operation: Self::Operation) -> anyhow::Result<Amount> {
        self.unlock_await
            .lock()
            .unwrap()
            .pop_front()
            .expect("test supplied an unlock completion result")
    }
}

fn balances(idle: u64, staged: u64, locked: u64) -> anyhow::Result<CollectionBalances> {
    Ok(CollectionBalances {
        idle: Amount::from_msats(idle),
        staged: Amount::from_msats(staged),
        locked: Amount::from_msats(locked),
    })
}

fn incomplete(
    claimed: u64,
    observed: Option<u64>,
    phase: CollectionFailurePhase,
    operation_submitted: bool,
) -> Collected {
    Collected::Incomplete {
        confirmed_claimed: Amount::from_msats(claimed),
        observed_awaiting_cycle: observed.map(Amount::from_msats),
        failure: CollectionFailure {
            phase,
            operation_submitted,
        },
    }
}

#[tokio::test]
async fn pre_read_and_idle_pre_id_failures_remain_outer_errors() {
    let pre_read = FakeOperations::new(vec![Err(anyhow::anyhow!("pre-read failed"))]);
    assert!(collect_operations(&pre_read).await.is_err());
    assert_eq!(pre_read.idle_submit_calls.load(Ordering::Relaxed), 0);
    assert_eq!(pre_read.unlock_submit_calls.load(Ordering::Relaxed), 0);

    let submission = FakeOperations::new(vec![balances(10, 0, 0)])
        .idle(Err(anyhow::anyhow!("submission failed")), None);
    assert!(collect_operations(&submission).await.is_err());
    assert_eq!(submission.balances.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn overflowing_initial_balance_is_rejected_before_submission() {
    let operations = FakeOperations::new(vec![balances(1, u64::MAX, 1)]);

    assert!(collect_operations(&operations).await.is_err());
    assert_eq!(operations.idle_submit_calls.load(Ordering::Relaxed), 0);
    assert_eq!(operations.unlock_submit_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn idle_post_id_wait_failure_reports_zero_confirmed_and_observed_balance() {
    let operations = FakeOperations::new(vec![balances(10, 0, 0), balances(0, 20, 30)])
        .idle(Ok(1), Some(Err(anyhow::anyhow!("subscription failed"))));

    assert_eq!(
        collect_operations(&operations).await.unwrap(),
        incomplete(0, Some(50), CollectionFailurePhase::IdleClaim, true)
    );
}

#[tokio::test]
async fn overflowing_failure_refresh_reports_unknown_balance() {
    let operations = FakeOperations::new(vec![balances(10, 0, 0), balances(0, u64::MAX, 1)])
        .idle(Ok(1), Some(Err(anyhow::anyhow!("subscription failed"))));

    assert_eq!(
        collect_operations(&operations).await.unwrap(),
        incomplete(0, None, CollectionFailurePhase::IdleClaim, true)
    );
}

#[tokio::test]
async fn unlock_pre_id_failure_without_earlier_progress_remains_outer_error() {
    let operations = FakeOperations::new(vec![balances(0, 20, 30)])
        .unlock(Err(anyhow::anyhow!("submission failed")), None);

    assert!(collect_operations(&operations).await.is_err());
}

#[tokio::test]
async fn staged_only_unlock_post_id_wait_failure_reports_zero_confirmed() {
    let operations = FakeOperations::new(vec![balances(0, 20, 0), balances(0, 20, 0)])
        .unlock(Ok(1), Some(Err(anyhow::anyhow!("terminal failed"))));

    assert_eq!(
        collect_operations(&operations).await.unwrap(),
        incomplete(0, Some(20), CollectionFailurePhase::Unlock, true)
    );
}

#[tokio::test]
async fn prior_success_then_unlock_pre_id_and_refresh_failure_preserves_progress() {
    let operations = FakeOperations::new(vec![
        balances(100, 20, 30),
        Err(anyhow::anyhow!("refresh failed")),
    ])
    .idle(Ok(1), Some(Ok(Amount::from_msats(100))))
    .unlock(Err(anyhow::anyhow!("submission failed")), None);

    assert_eq!(
        collect_operations(&operations).await.unwrap(),
        incomplete(100, None, CollectionFailurePhase::Unlock, false)
    );
}

#[tokio::test]
async fn final_refresh_failure_after_operations_reports_combined_progress() {
    let operations = FakeOperations::new(vec![
        balances(100, 20, 30),
        Err(anyhow::anyhow!("final refresh failed")),
    ])
    .idle(Ok(1), Some(Ok(Amount::from_msats(100))))
    .unlock(Ok(2), Some(Ok(Amount::from_msats(50))));

    assert_eq!(
        collect_operations(&operations).await.unwrap(),
        incomplete(150, None, CollectionFailurePhase::BalanceRefresh, false)
    );
}

#[tokio::test]
async fn overflowing_final_refresh_after_operation_is_incomplete() {
    let operations = FakeOperations::new(vec![balances(10, 0, 0), balances(0, u64::MAX, 1)])
        .idle(Ok(1), Some(Ok(Amount::from_msats(10))));

    assert_eq!(
        collect_operations(&operations).await.unwrap(),
        incomplete(10, None, CollectionFailurePhase::BalanceRefresh, false)
    );
}

#[tokio::test]
async fn overflowing_final_refresh_without_operation_remains_outer_error() {
    let operations = FakeOperations::new(vec![balances(0, 0, 0), balances(0, u64::MAX, 1)]);

    assert!(collect_operations(&operations).await.is_err());
}

#[tokio::test]
async fn overflowing_confirmed_claim_total_is_rejected() {
    let operations = FakeOperations::new(vec![balances(1, 1, 0)])
        .idle(Ok(1), Some(Ok(Amount::from_msats(u64::MAX))))
        .unlock(Ok(2), Some(Ok(Amount::from_msats(1))));

    assert!(collect_operations(&operations).await.is_err());
}

#[tokio::test]
async fn all_zero_final_refresh_failure_remains_outer_error() {
    let operations = FakeOperations::new(vec![
        balances(0, 0, 0),
        Err(anyhow::anyhow!("final refresh failed")),
    ]);

    assert!(collect_operations(&operations).await.is_err());
}

#[tokio::test]
async fn complete_collection_reports_combined_claim_and_refreshed_balance() {
    let operations = FakeOperations::new(vec![balances(100, 20, 30), balances(0, 0, 40)])
        .idle(Ok(1), Some(Ok(Amount::from_msats(100))))
        .unlock(Ok(2), Some(Ok(Amount::from_msats(50))));

    assert_eq!(
        collect_operations(&operations).await.unwrap(),
        Collected::Complete {
            claimed: Amount::from_msats(150),
            awaiting_cycle: Amount::from_msats(40),
        }
    );
}

#[tokio::test]
async fn terminal_success_counts_only_confirmed_amount() {
    let mut updates = futures::stream::iter([
        StabilityPoolWithdrawalOperationState::Initiated,
        StabilityPoolWithdrawalOperationState::Success(Amount::from_msats(123)),
    ]);

    assert_eq!(
        terminal_withdrawal(&mut updates, "unlock").await.unwrap(),
        Amount::from_msats(123)
    );
}

#[tokio::test]
async fn terminal_errors_and_ended_stream_are_errors() {
    for terminal in [
        StabilityPoolWithdrawalOperationState::UnlockTxRejected("rejected".to_owned()),
        StabilityPoolWithdrawalOperationState::UnlockProcessingError("processing".to_owned()),
        StabilityPoolWithdrawalOperationState::WithdrawalTxRejected("rejected".to_owned()),
        StabilityPoolWithdrawalOperationState::PrimaryOutputError("output".to_owned()),
    ] {
        let mut updates = futures::stream::iter([terminal]);
        assert!(terminal_withdrawal(&mut updates, "unlock").await.is_err());
    }

    let mut ended = futures::stream::empty();
    assert!(terminal_withdrawal(&mut ended, "unlock").await.is_err());
}
