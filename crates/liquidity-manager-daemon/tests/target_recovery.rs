use std::sync::Arc;

use async_trait::async_trait;
use fedi_decentralized_service_liquidity_manager::{FederationId, Sats, SourceType};
use tokio::sync::Mutex;

use super::*;
use crate::allocation_store::FundingTargetRecord;
use crate::stability_pool::{
    PegInStatus, StabilityPoolReport, TargetDepositOperation, TargetDepositScan,
};
use crate::test_support::{AllocationSeed, ItemSeed, test_sqlite_path};

/// The signature of the interrupted-submit window, seen from the Admin
/// surface: the client holds a deposit that the item does not name.
#[tokio::test]
async fn inspection_shows_a_deposit_the_item_never_recorded() -> anyhow::Result<()> {
    let database =
        seeded_database("inspect-orphan", ItemAllocationStatus::ActionRequired, None).await?;
    let backend = FakeBackend::with_deposits(vec![orphan_deposit(
        "3333333333333333333333333333333333333333333333333333333333333333",
        Sats(10_000),
    )]);

    let response = inspect_target_client(
        &database,
        &backend,
        InspectTargetClientRequest {
            federation_id: FederationId("federation-1".to_owned()),
        },
    )
    .await?;

    assert_eq!(response.recorded_deposit_operation_id, None);
    assert_eq!(response.deposit_operations.len(), 1);
    assert_eq!(
        response.deposit_operations[0].operation_id,
        "3333333333333333333333333333333333333333333333333333333333333333"
    );
    assert_eq!(
        response.deposit_operations[0].outcome, None,
        "a deposit the client never observed to completion reports no outcome"
    );
    assert_eq!(response.spendable_balance, Sats(2_500));
    Ok(())
}

/// Binding hands the deposit back to the worker: the item names it and
/// leaves `action_required`, and observation restarts from the beginning so
/// the normal completion gate still applies.
#[tokio::test]
async fn binding_records_the_deposit_and_resumes_the_item() -> anyhow::Result<()> {
    let database =
        seeded_database("bind-resume", ItemAllocationStatus::ActionRequired, None).await?;
    let backend = FakeBackend::with_deposits(vec![orphan_deposit(
        "3333333333333333333333333333333333333333333333333333333333333333",
        Sats(10_000),
    )]);

    let response = bind(
        &database,
        &backend,
        "3333333333333333333333333333333333333333333333333333333333333333",
    )
    .await?;
    assert_eq!(
        response.status,
        ManualOperationStatus::Accepted,
        "{:?}",
        response.detail
    );

    let item =
        allocation_store::stability_pool_item(&database, &FederationId("federation-1".to_owned()))
            .await?
            .expect("seeded item");
    assert_eq!(item.status, ItemAllocationStatus::Running);
    assert_eq!(
        item.step
            .sp_deposit_operation_id
            .map(|operation_id| operation_id.to_string())
            .as_deref(),
        Some("3333333333333333333333333333333333333333333333333333333333333333")
    );
    assert_eq!(
        item.step.sp_deposit_status,
        Some(allocation_store::SpDepositStatus::Initiated)
    );
    assert_eq!(audit_count(&database, "accepted").await?, 1);
    Ok(())
}

/// An operation the target client does not have is a typo, not a finding.
/// Accepting it would attach the item to nothing and let a later sibling
/// deposit complete it.
#[tokio::test]
async fn binding_rejects_an_operation_the_client_does_not_have() -> anyhow::Result<()> {
    let database =
        seeded_database("bind-unknown", ItemAllocationStatus::ActionRequired, None).await?;
    let backend = FakeBackend::with_deposits(vec![orphan_deposit(
        "3333333333333333333333333333333333333333333333333333333333333333",
        Sats(10_000),
    )]);

    let response = bind(
        &database,
        &backend,
        "4444444444444444444444444444444444444444444444444444444444444444",
    )
    .await?;

    assert_eq!(response.status, ManualOperationStatus::NotFound);
    assert_unchanged(&database).await
}

/// A deposit smaller than the item committed cannot be the one that
/// discharges it; binding it would complete the item for less than it
/// promised.
#[tokio::test]
async fn binding_rejects_a_deposit_below_the_committed_amount() -> anyhow::Result<()> {
    let database =
        seeded_database("bind-short", ItemAllocationStatus::ActionRequired, None).await?;
    let backend = FakeBackend::with_deposits(vec![orphan_deposit(
        "3333333333333333333333333333333333333333333333333333333333333333",
        Sats(9_999),
    )]);

    let response = bind(
        &database,
        &backend,
        "3333333333333333333333333333333333333333333333333333333333333333",
    )
    .await?;

    assert_eq!(response.status, ManualOperationStatus::Rejected);
    assert_unchanged(&database).await
}

/// Only an item FLIP has given up on may be bound. A running item is still
/// the worker's, and binding under it would race the worker.
#[tokio::test]
async fn binding_rejects_an_item_that_is_not_awaiting_action() -> anyhow::Result<()> {
    let database = seeded_database("bind-running", ItemAllocationStatus::Running, None).await?;
    let backend = FakeBackend::with_deposits(vec![orphan_deposit(
        "3333333333333333333333333333333333333333333333333333333333333333",
        Sats(10_000),
    )]);

    let response = bind(
        &database,
        &backend,
        "3333333333333333333333333333333333333333333333333333333333333333",
    )
    .await?;

    assert_eq!(response.status, ManualOperationStatus::Rejected);
    assert_unchanged(&database).await
}

/// Binding over an id the item already carries would silently redirect its
/// completion evidence to a different deposit.
#[tokio::test]
async fn binding_will_not_replace_a_recorded_operation() -> anyhow::Result<()> {
    let database = seeded_database(
        "bind-replace",
        ItemAllocationStatus::ActionRequired,
        Some(r#"{"sp_deposit_operation_id":"2222222222222222222222222222222222222222222222222222222222222222","sp_deposit_status":"initiated"}"#),
    )
    .await?;
    let backend = FakeBackend::with_deposits(vec![orphan_deposit(
        "3333333333333333333333333333333333333333333333333333333333333333",
        Sats(10_000),
    )]);

    let replace = bind(
        &database,
        &backend,
        "3333333333333333333333333333333333333333333333333333333333333333",
    )
    .await?;
    assert_eq!(replace.status, ManualOperationStatus::Rejected);

    // Re-binding the same id is a retry, not a conflict.
    let repeat = bind(
        &database,
        &backend,
        "2222222222222222222222222222222222222222222222222222222222222222",
    )
    .await?;
    assert_eq!(repeat.status, ManualOperationStatus::AlreadyApplied);

    let item =
        allocation_store::stability_pool_item(&database, &FederationId("federation-1".to_owned()))
            .await?
            .expect("seeded item");
    assert_eq!(
        item.step
            .sp_deposit_operation_id
            .map(|operation_id| operation_id.to_string())
            .as_deref(),
        Some("2222222222222222222222222222222222222222222222222222222222222222")
    );
    Ok(())
}

/// The point of the operation: an item nothing else can move stops holding
/// provider capacity.
///
/// After the peg-in is claimed the funding send has settled, so
/// `cancel_allocation` and `retry_funding_step` both refuse, and a pool that
/// will never accept the deposit means the item can never complete either.
/// `action_required` reserves capacity, so without this one such federation
/// consumes it permanently.
#[tokio::test]
async fn abandoning_releases_the_capacity_the_item_held() -> anyhow::Result<()> {
    let database = seeded_database(
        "abandon-releases",
        ItemAllocationStatus::ActionRequired,
        Some(r#"{"peg_in_status":"claimed","peg_in_amount":10000}"#),
    )
    .await?;
    assert_eq!(reserved_amount(&database).await?, 10_000);

    let response = abandon(&database, "the pool rejects provision permanently").await?;

    assert_eq!(
        response.status,
        ManualOperationStatus::Accepted,
        "{:?}",
        response.detail
    );
    assert_eq!(
        response.abandoned_amount.map(|amount| amount.0),
        Some(10_000)
    );
    assert_eq!(
        reserved_amount(&database).await?,
        0,
        "a failed item reserves nothing"
    );

    let item =
        allocation_store::stability_pool_item(&database, &FederationId("federation-1".to_owned()))
            .await?
            .expect("seeded item");
    assert_eq!(item.status, ItemAllocationStatus::Failed);
    Ok(())
}

/// The record must say the value is still out there. Releasing capacity
/// silently would leave nothing pointing at funds that need recovering by
/// hand.
#[tokio::test]
async fn abandoning_records_the_value_left_behind() -> anyhow::Result<()> {
    let database = seeded_database(
        "abandon-records",
        ItemAllocationStatus::ActionRequired,
        Some(r#"{"peg_in_status":"claimed","peg_in_amount":10000}"#),
    )
    .await?;

    let response = abandon(&database, "pool rejected the provide twice").await?;
    let detail = response.detail.unwrap_or_default();

    assert!(detail.contains("10000"), "names the amount: {detail}");
    assert!(
        detail.contains("outside FLIP"),
        "says the value needs out-of-band recovery: {detail}"
    );
    assert!(
        detail.contains("pool rejected the provide twice"),
        "carries the operator's reason: {detail}"
    );
    assert_eq!(
        abandon_audit_count(&database, "accepted").await?,
        1,
        "the decision is audited"
    );
    Ok(())
}

/// Before the peg-in is claimed the provider's funds have not reached the
/// target client, so nothing needs writing off and the ordinary operations
/// still apply.
#[tokio::test]
async fn abandoning_is_refused_before_value_reaches_the_client() -> anyhow::Result<()> {
    let database = seeded_database(
        "abandon-pre-pegin",
        ItemAllocationStatus::ActionRequired,
        Some(r#"{"peg_in_status":"waiting_for_transaction"}"#),
    )
    .await?;

    let response = abandon(&database, "give up").await?;

    assert_eq!(response.status, ManualOperationStatus::Rejected);
    assert_eq!(reserved_amount(&database).await?, 10_000);
    Ok(())
}

/// Writing off value FLIP already sent should not be possible without
/// saying why.
#[tokio::test]
async fn abandoning_requires_a_reason() -> anyhow::Result<()> {
    let database = seeded_database(
        "abandon-no-reason",
        ItemAllocationStatus::ActionRequired,
        Some(r#"{"peg_in_status":"claimed","peg_in_amount":10000}"#),
    )
    .await?;

    let response = abandon(&database, "   ").await?;

    assert_eq!(response.status, ManualOperationStatus::Rejected);
    assert_eq!(reserved_amount(&database).await?, 10_000);
    Ok(())
}

/// A running item is still the worker's to advance.
#[tokio::test]
async fn abandoning_is_refused_for_an_item_not_awaiting_action() -> anyhow::Result<()> {
    let database = seeded_database(
        "abandon-running",
        ItemAllocationStatus::Running,
        Some(r#"{"peg_in_status":"claimed","peg_in_amount":10000}"#),
    )
    .await?;

    let response = abandon(&database, "give up").await?;

    assert_eq!(response.status, ManualOperationStatus::Rejected);
    assert_eq!(reserved_amount(&database).await?, 10_000);
    Ok(())
}

/// An unreadable whole step cannot be safely reconstructed for binding, but
/// the operator retains an atomic audited reservation-release escape.
#[tokio::test]
async fn unreadable_step_rejects_bind_and_abandons_atomically() -> anyhow::Result<()> {
    let raw = r#"{"peg_in_status":42,"sp_deposit_amount":"bad"}"#;
    let database = seeded_database(
        "corrupt-step-operator",
        ItemAllocationStatus::ActionRequired,
        Some(raw),
    )
    .await?;
    let backend = FakeBackend::with_deposits(vec![orphan_deposit(
        "3333333333333333333333333333333333333333333333333333333333333333",
        Sats(10_000),
    )]);

    let bound = bind(
        &database,
        &backend,
        "3333333333333333333333333333333333333333333333333333333333333333",
    )
    .await?;
    assert_eq!(bound.status, ManualOperationStatus::Rejected);
    let retained: String = sqlx::query_scalar(
        "SELECT step_json FROM allocation_items WHERE federation_id = 'federation-1'",
    )
    .fetch_one(database.pool())
    .await?;
    assert_eq!(retained, raw);

    let abandoned = abandon(&database, "unreadable state reconciled externally").await?;
    assert_eq!(abandoned.status, ManualOperationStatus::Accepted);
    assert_eq!(reserved_amount(&database).await?, 0);
    assert_eq!(abandon_audit_count(&database, "accepted").await?, 1);
    Ok(())
}

async fn abandon(
    database: &Database,
    reason: &str,
) -> ServiceResult<AbandonTargetClientValueResponse> {
    abandon_target_client_value(
        database,
        AbandonTargetClientValueRequest {
            federation_id: FederationId("federation-1".to_owned()),
            reason: reason.to_owned(),
        },
    )
    .await
}

async fn reserved_amount(database: &Database) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar::<_, Option<i64>>(
        "SELECT SUM(reserved_amount_sats) FROM allocation_items \
         WHERE status IN ('pending', 'running', 'action_required')",
    )
    .fetch_one(database.pool())
    .await?
    .unwrap_or_default())
}

async fn abandon_audit_count(database: &Database, outcome: &str) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'abandon_target_client_value' \
         AND detail_json LIKE ?",
    )
    .bind(format!("%\"outcome\":\"{outcome}\"%"))
    .fetch_one(database.pool())
    .await?)
}

async fn bind(
    database: &Database,
    backend: &FakeBackend,
    operation_id: &str,
) -> ServiceResult<BindTargetDepositResponse> {
    bind_target_deposit(
        database,
        backend,
        BindTargetDepositRequest {
            federation_id: FederationId("federation-1".to_owned()),
            operation_id: operation_id.to_owned(),
            reason: Some("reconciled with the target client".to_owned()),
        },
    )
    .await
}

async fn assert_unchanged(database: &Database) -> anyhow::Result<()> {
    let item =
        allocation_store::stability_pool_item(database, &FederationId("federation-1".to_owned()))
            .await?
            .expect("seeded item");
    assert_eq!(
        item.step.sp_deposit_operation_id, None,
        "a refused binding records nothing"
    );
    assert_eq!(audit_count(database, "accepted").await?, 0);
    Ok(())
}

async fn seeded_database(
    name: &str,
    status: ItemAllocationStatus,
    step_json: Option<&str>,
) -> anyhow::Result<Database> {
    let database = Database::connect(test_sqlite_path(name)).await?;
    AllocationSeed {
        items: vec![ItemSeed {
            source_type: SourceType::StabilityPool,
            status,
            committed_amount: Sats(10_000),
            reserved_amount: Sats(10_000),
            step_json: step_json.map(str::to_owned),
            ..ItemSeed::default()
        }],
        ..AllocationSeed::default()
    }
    .insert(&database)
    .await?;
    Ok(database)
}

async fn audit_count(database: &Database, outcome: &str) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'bind_target_deposit' \
         AND detail_json LIKE ?",
    )
    .bind(format!("%\"outcome\":\"{outcome}\"%"))
    .fetch_one(database.pool())
    .await?)
}

fn orphan_deposit(operation_id: &str, amount: Sats) -> TargetDepositOperation {
    TargetDepositOperation {
        operation_id: operation_id.to_owned(),
        amount,
        // No cached outcome: the crash happened before anything drained the
        // operation's stream.
        outcome: None,
        created_at: 1_700_000_000,
    }
}

#[derive(Default)]
struct FakeBackend {
    deposits: Arc<Mutex<Vec<TargetDepositOperation>>>,
}

impl FakeBackend {
    fn with_deposits(deposits: Vec<TargetDepositOperation>) -> Self {
        Self {
            deposits: Arc::new(Mutex::new(deposits)),
        }
    }
}

#[async_trait]
impl StabilityPoolBackend for FakeBackend {
    async fn ensure_client(&self, _target: &FundingTargetRecord) -> anyhow::Result<()> {
        Ok(())
    }

    async fn check_target(
        &self,
        _target: &FundingTargetRecord,
    ) -> anyhow::Result<crate::stability_pool::TargetCheck> {
        unreachable!("reconciliation never funds a target")
    }

    async fn allocate_peg_in_address(
        &self,
        _target: &FundingTargetRecord,
    ) -> anyhow::Result<crate::stability_pool::PegInAllocation> {
        unreachable!("reconciliation never allocates an address")
    }

    async fn observe_peg_in(
        &self,
        _target: &FundingTargetRecord,
        _operation_id: &str,
    ) -> anyhow::Result<PegInStatus> {
        unreachable!("reconciliation never observes a peg-in")
    }

    async fn recheck_peg_in(
        &self,
        _target: &FundingTargetRecord,
        _operation_id: &str,
    ) -> anyhow::Result<()> {
        unreachable!("reconciliation never drives a peg-in")
    }

    async fn target_wallet_balance(&self, _target: &FundingTargetRecord) -> anyhow::Result<Sats> {
        Ok(Sats(2_500))
    }

    async fn submit_deposit_to_provide(
        &self,
        _target: &FundingTargetRecord,
        _submission: crate::stability_deposit::StabilityDepositSubmission,
        _diagnostic_item_id: &str,
    ) -> anyhow::Result<crate::stability_pool::SubmissionReceipt> {
        unreachable!("reconciliation never submits a deposit")
    }

    async fn observe_deposit(
        &self,
        _target: &FundingTargetRecord,
        _operation_id: crate::stability_deposit::StabilityDepositOperationId,
    ) -> anyhow::Result<StabilityDepositStatus> {
        unreachable!("reconciliation never observes a deposit")
    }

    async fn report(&self, _target: &FundingTargetRecord) -> anyhow::Result<StabilityPoolReport> {
        Ok(StabilityPoolReport {
            observed_provided_amount: Sats(0),
            liquidity_stats_json: "{}".to_owned(),
        })
    }

    async fn list_deposit_operations(
        &self,
        _target: &FundingTargetRecord,
    ) -> anyhow::Result<TargetDepositScan> {
        Ok(TargetDepositScan {
            operations: self.deposits.lock().await.clone(),
            complete: true,
        })
    }

    async fn get_deposit_operation(
        &self,
        _target: &FundingTargetRecord,
        operation_id: crate::stability_deposit::StabilityDepositOperationId,
    ) -> anyhow::Result<Option<TargetDepositOperation>> {
        Ok(self
            .deposits
            .lock()
            .await
            .iter()
            .find(|operation| operation.operation_id == operation_id.to_string())
            .cloned())
    }
}
