use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use fedimint_client::db::OperationLogKey;
use fedimint_client_module::oplog::{JsonStringed, OperationLogEntry};
use fedimint_core::Amount;
use fedimint_core::core::OperationId;
use fedimint_core::db::{Database, IDatabaseTransactionOpsCoreTyped};
use fman_core::guardian_fee::{Collected, CollectionFailure, CollectionFailurePhase};
use stability_pool_client::StabilityPoolWithdrawalOperationState;

use super::{
    CollectionBalances, CollectionOperations, Reconciled, collect_operations,
    collection_receipt_prefix, operation_log_entries, read_collection_receipt,
    read_recorded_claimed, record_collection_receipt, terminal_withdrawal,
};

struct FakeOperations {
    balances: Mutex<VecDeque<anyhow::Result<CollectionBalances>>>,
    idle_submit: Mutex<VecDeque<anyhow::Result<u8>>>,
    idle_await: Mutex<VecDeque<anyhow::Result<Amount>>>,
    unlock_submit: Mutex<VecDeque<anyhow::Result<u8>>>,
    unlock_await: Mutex<VecDeque<anyhow::Result<Amount>>>,
    idle_submit_calls: AtomicUsize,
    unlock_submit_calls: AtomicUsize,
    receipts: Mutex<std::collections::BTreeMap<u8, Amount>>,
    fail_reconcile_total_read: bool,
    fail_receipt_write: bool,
    committed_total: Option<Amount>,
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
            receipts: Mutex::default(),
            fail_reconcile_total_read: false,
            fail_receipt_write: false,
            committed_total: None,
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

    fn failing_total_read(mut self) -> Self {
        self.fail_reconcile_total_read = true;
        self
    }

    fn failing_receipt_write(mut self) -> Self {
        self.fail_receipt_write = true;
        self
    }

    fn committed_total(mut self, total: u64) -> Self {
        self.committed_total = Some(Amount::from_msats(total));
        self
    }
}

#[async_trait::async_trait]
impl CollectionOperations for FakeOperations {
    type Operation = u8;

    async fn reconcile_unobserved(
        &self,
    ) -> Result<Reconciled, (CollectionFailurePhase, Reconciled)> {
        if self.fail_reconcile_total_read {
            return Err((
                CollectionFailurePhase::BalanceRefresh,
                Reconciled::default(),
            ));
        }
        Ok(Reconciled::default())
    }

    async fn receipt(&self, operation: &Self::Operation) -> anyhow::Result<Option<Amount>> {
        Ok(self.receipts.lock().unwrap().get(operation).copied())
    }

    async fn recorded_claimed(&self) -> anyhow::Result<Amount> {
        self.receipts
            .lock()
            .unwrap()
            .values()
            .copied()
            .try_fold(Amount::ZERO, |total, amount| {
                super::checked_amount_sum(total, amount, "fake receipt total")
            })
    }

    async fn record_receipt(
        &self,
        operation: &Self::Operation,
        amount: Amount,
    ) -> anyhow::Result<Amount> {
        anyhow::ensure!(!self.fail_receipt_write, "injected receipt failure");
        self.receipts.lock().unwrap().insert(*operation, amount);
        match self.committed_total {
            Some(total) => Ok(total),
            None => self.recorded_claimed().await,
        }
    }

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

/// A native operation survives cancellation while its FMan stack frame does
/// not. Reconciliation is the only route from `pending` back to the caller.
struct CancellationOperations {
    idle: AtomicU64,
    pending: AtomicU64,
    recorded: AtomicU64,
    await_completes: bool,
    block_refresh: std::sync::atomic::AtomicBool,
    submit_calls: AtomicUsize,
    submitted: tokio::sync::Notify,
    receipt_recorded: tokio::sync::Notify,
}

impl CancellationOperations {
    fn new(idle: u64, await_completes: bool) -> Self {
        Self {
            idle: AtomicU64::new(idle),
            pending: AtomicU64::new(0),
            recorded: AtomicU64::new(0),
            await_completes,
            block_refresh: std::sync::atomic::AtomicBool::new(await_completes),
            submit_calls: AtomicUsize::new(0),
            submitted: tokio::sync::Notify::new(),
            receipt_recorded: tokio::sync::Notify::new(),
        }
    }
}

#[async_trait::async_trait]
impl CollectionOperations for CancellationOperations {
    type Operation = u8;

    async fn reconcile_unobserved(
        &self,
    ) -> Result<Reconciled, (CollectionFailurePhase, Reconciled)> {
        let recovered = self.pending.swap(0, Ordering::SeqCst);
        if recovered != 0 {
            self.recorded.store(recovered, Ordering::SeqCst);
        }
        Ok(Reconciled {
            newly_claimed: Amount::from_msats(recovered),
            recorded_claimed: Amount::from_msats(self.recorded.load(Ordering::SeqCst)),
        })
    }

    async fn receipt(&self, _operation: &Self::Operation) -> anyhow::Result<Option<Amount>> {
        let amount = self.recorded.load(Ordering::SeqCst);
        Ok((amount != 0).then(|| Amount::from_msats(amount)))
    }

    async fn recorded_claimed(&self) -> anyhow::Result<Amount> {
        Ok(Amount::from_msats(self.recorded.load(Ordering::SeqCst)))
    }

    async fn record_receipt(
        &self,
        _operation: &Self::Operation,
        amount: Amount,
    ) -> anyhow::Result<Amount> {
        self.recorded.store(amount.msats, Ordering::SeqCst);
        self.pending.store(0, Ordering::SeqCst);
        self.receipt_recorded.notify_waiters();
        Ok(amount)
    }

    async fn balances(&self) -> anyhow::Result<CollectionBalances> {
        let idle = self.idle.load(Ordering::SeqCst);
        if idle == 0 && self.block_refresh.load(Ordering::SeqCst) {
            std::future::pending().await
        } else {
            balances(idle, 0, 0)
        }
    }

    async fn submit_idle(&self, amount: Amount) -> anyhow::Result<Self::Operation> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        self.idle.store(0, Ordering::SeqCst);
        self.pending.store(amount.msats, Ordering::SeqCst);
        self.submitted.notify_waiters();
        Ok(1)
    }

    async fn await_idle(&self, _operation: Self::Operation) -> anyhow::Result<Amount> {
        if self.await_completes {
            Ok(Amount::from_msats(self.pending.load(Ordering::SeqCst)))
        } else {
            std::future::pending().await
        }
    }

    async fn submit_unlock(&self) -> anyhow::Result<Self::Operation> {
        panic!("no locked value in cancellation regression")
    }

    async fn await_unlock(&self, _operation: Self::Operation) -> anyhow::Result<Amount> {
        panic!("no unlock operation in cancellation regression")
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
        recorded_claimed: Amount::from_msats(claimed),
        observed_awaiting_cycle: observed.map(Amount::from_msats),
        failure: CollectionFailure {
            phase,
            operation_submitted,
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn collection_receipts_use_user_data_namespace_and_survive_reopen() {
    assert_eq!(
        collection_receipt_prefix().first().copied(),
        Some(fedimint_client::db::DbKeyPrefix::UserData as u8)
    );
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("receipts.db");
    let operation = OperationId([7; 32]);
    {
        let database: Database = fedimint_rocksdb::RocksDb::build(path.clone())
            .open()
            .await
            .unwrap()
            .into();
        let receipts = database.with_prefix(collection_receipt_prefix());
        record_collection_receipt(&receipts, &operation, Amount::from_msats(100))
            .await
            .unwrap();
        // An exact replay cannot advance the durable total twice.
        record_collection_receipt(&receipts, &operation, Amount::from_msats(100))
            .await
            .unwrap();
    }
    {
        let database: Database = fedimint_rocksdb::RocksDb::build(path)
            .open()
            .await
            .unwrap()
            .into();
        let receipts = database.with_prefix(collection_receipt_prefix());
        assert_eq!(
            read_collection_receipt(&receipts, &operation)
                .await
                .unwrap(),
            Some(Amount::from_msats(100))
        );
        assert_eq!(
            read_recorded_claimed(&receipts).await.unwrap(),
            Amount::from_msats(100)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_enumerates_operation_ids_without_a_chronological_entry() {
    let temp = tempfile::tempdir().unwrap();
    let database: Database = fedimint_rocksdb::RocksDb::build(temp.path().join("operations.db"))
        .open()
        .await
        .unwrap()
        .into();
    let operation_id = OperationId([8; 32]);
    let mut tx = database.begin_transaction().await;
    tx.insert_entry(
        &OperationLogKey { operation_id },
        &OperationLogEntry::new(
            "stability_pool".to_owned(),
            JsonStringed(serde_json::json!({})),
            None,
        ),
    )
    .await;
    tx.commit_tx().await;

    let entries = operation_log_entries(&database).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0.operation_id, operation_id);
}

#[tokio::test]
async fn cancellation_after_durable_operation_id_is_reconciled_without_resubmission() {
    let operations = Arc::new(CancellationOperations::new(100, false));
    let submitted = operations.submitted.notified();
    let first_operations = operations.clone();
    let first = tokio::spawn(async move { collect_operations(&*first_operations).await });

    submitted.await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());

    assert_eq!(
        collect_operations(&*operations).await.unwrap(),
        Collected::Complete {
            claimed: Amount::from_msats(100),
            recorded_claimed: Amount::from_msats(100),
            awaiting_cycle: Amount::ZERO,
        }
    );
    assert_eq!(operations.submit_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_after_receipt_does_not_repeat_or_forget_claimed_value() {
    let operations = Arc::new(CancellationOperations::new(100, true));
    let receipt_recorded = operations.receipt_recorded.notified();
    let first_operations = operations.clone();
    let first = tokio::spawn(async move { collect_operations(&*first_operations).await });

    receipt_recorded.await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    operations.block_refresh.store(false, Ordering::SeqCst);

    assert_eq!(
        collect_operations(&*operations).await.unwrap(),
        Collected::Complete {
            claimed: Amount::ZERO,
            recorded_claimed: Amount::from_msats(100),
            awaiting_cycle: Amount::ZERO,
        }
    );
    assert_eq!(operations.submit_calls.load(Ordering::SeqCst), 1);
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
async fn durable_total_read_failure_never_fabricates_an_incomplete_zero() {
    let operations = FakeOperations::new(vec![]).failing_total_read();

    assert!(collect_operations(&operations).await.is_err());
}

#[tokio::test]
async fn receipt_write_failure_is_explicit_and_does_not_claim_success() {
    let operations = FakeOperations::new(vec![balances(100, 0, 0), balances(0, 0, 0)])
        .idle(Ok(1), Some(Ok(Amount::from_msats(100))))
        .failing_receipt_write();

    assert_eq!(
        collect_operations(&operations).await.unwrap(),
        incomplete(0, Some(0), CollectionFailurePhase::Receipt, true)
    );
}

#[tokio::test]
async fn receipt_commit_result_supplies_the_cumulative_total_without_a_reread() {
    let operations = FakeOperations::new(vec![balances(100, 0, 0), balances(0, 0, 0)])
        .idle(Ok(1), Some(Ok(Amount::from_msats(100))))
        .committed_total(500);

    assert_eq!(
        collect_operations(&operations).await.unwrap(),
        Collected::Complete {
            claimed: Amount::from_msats(100),
            recorded_claimed: Amount::from_msats(500),
            awaiting_cycle: Amount::ZERO,
        }
    );
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
    let operations = FakeOperations::new(vec![balances(1, 1, 0), balances(0, 0, 0)])
        .idle(Ok(1), Some(Ok(Amount::from_msats(u64::MAX))))
        .unlock(Ok(2), Some(Ok(Amount::from_msats(1))));

    assert_eq!(
        collect_operations(&operations).await.unwrap(),
        incomplete(u64::MAX, Some(0), CollectionFailurePhase::Receipt, true)
    );
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
            recorded_claimed: Amount::from_msats(150),
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

    let mut contradictory = futures::stream::iter([
        StabilityPoolWithdrawalOperationState::Success(Amount::from_msats(1)),
        StabilityPoolWithdrawalOperationState::Success(Amount::from_msats(2)),
    ]);
    assert!(
        terminal_withdrawal(&mut contradictory, "unlock")
            .await
            .is_err()
    );
}
