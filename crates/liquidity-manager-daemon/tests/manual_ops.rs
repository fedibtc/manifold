use fedi_decentralized_service_liquidity_manager::{
    FederationId, ItemAllocationStatus, ReleaseFederationAllocationRequest, Sats, SourceType,
    WalletOperationStatus, WalletOperationType,
};

use super::*;
use crate::Database;
use crate::test_support::{AllocationSeed, ItemSeed, test_sqlite_path};
use crate::wallet::{WalletOperationInput, get_wallet_operation, insert_wallet_operation_tx};

#[tokio::test]
async fn retry_action_required_item_requeues_item_and_wallet_operation() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-retry")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::ActionRequired).await?;
    let operation_id = WalletOperationId("wallet-op-1".to_owned());
    seed_wallet_operation(
        &database,
        &ids.federation_id,
        &ids.item_id,
        &operation_id,
        WalletOperationStatus::Failed,
        None,
    )
    .await?;

    let response = retry_funding_step_with_database(
        &database,
        RetryFundingStepRequest {
            federation_id: ids.federation_id.clone(),
            item_id: Some(ids.item_id.clone()),
            operation_id: Some(operation_id.clone()),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Accepted);

    let operation = get_wallet_operation(&database, &operation_id).await?;
    assert_eq!(operation.status, WalletOperationStatus::Pending);
    assert!(operation.failure.is_none());
    let status =
        allocation_store::load_allocation_status_by_federation(&database, &ids.federation_id)
            .await?
            .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::Pending
    );
    assert!(status.item_statuses[0].failure.is_none());
    assert_eq!(
        audit_count(&database, "retry_funding_step", "accepted").await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn retry_rejects_in_doubt_wallet_operation() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-retry-in-doubt")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Failed).await?;
    let operation_id = WalletOperationId("wallet-op-1".to_owned());
    seed_wallet_operation(
        &database,
        &ids.federation_id,
        &ids.item_id,
        &operation_id,
        WalletOperationStatus::InDoubt,
        None,
    )
    .await?;

    let response = retry_funding_step_with_database(
        &database,
        RetryFundingStepRequest {
            federation_id: ids.federation_id,
            item_id: Some(ids.item_id),
            operation_id: Some(operation_id),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Rejected);
    assert_eq!(
        audit_count(&database, "retry_funding_step", "rejected").await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn retry_rejects_failed_wallet_operation_with_txid() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-retry-failed-txid")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Failed).await?;
    let operation_id = WalletOperationId("wallet-op-1".to_owned());
    seed_wallet_operation(
        &database,
        &ids.federation_id,
        &ids.item_id,
        &operation_id,
        WalletOperationStatus::Failed,
        Some("txid-1"),
    )
    .await?;

    let response = retry_funding_step_with_database(
        &database,
        RetryFundingStepRequest {
            federation_id: ids.federation_id,
            item_id: Some(ids.item_id),
            operation_id: Some(operation_id.clone()),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Rejected);
    assert_eq!(
        get_wallet_operation(&database, &operation_id).await?.status,
        WalletOperationStatus::Failed
    );
    Ok(())
}

#[tokio::test]
async fn retry_rejects_permanent_failed_item_without_wallet() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-retry-permanent-failure")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Failed).await?;
    let response = retry_funding_step_with_database(
        &database,
        RetryFundingStepRequest {
            federation_id: ids.federation_id.clone(),
            item_id: Some(ids.item_id),
            operation_id: None,
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Rejected);
    let status =
        allocation_store::load_allocation_status_by_federation(&database, &ids.federation_id)
            .await?
            .expect("allocation");
    assert_eq!(status.item_statuses[0].status, ItemAllocationStatus::Failed);
    Ok(())
}

#[tokio::test]
async fn retry_returns_already_applied_when_no_failed_work_matches() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-retry-already")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Pending).await?;
    let operation_id = WalletOperationId("wallet-op-1".to_owned());
    seed_wallet_operation(
        &database,
        &ids.federation_id,
        &ids.item_id,
        &operation_id,
        WalletOperationStatus::Pending,
        None,
    )
    .await?;

    let response = retry_funding_step_with_database(
        &database,
        RetryFundingStepRequest {
            federation_id: ids.federation_id,
            item_id: Some(ids.item_id),
            operation_id: Some(operation_id),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::AlreadyApplied);
    assert_eq!(
        audit_count(&database, "retry_funding_step", "already_applied").await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn retry_rejects_wallet_operation_without_item() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-retry-unattached")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Failed).await?;
    let operation_id = WalletOperationId("wallet-op-1".to_owned());
    seed_wallet_operation_without_item(
        &database,
        &ids.federation_id,
        &operation_id,
        WalletOperationStatus::Failed,
    )
    .await?;

    let response = retry_funding_step_with_database(
        &database,
        RetryFundingStepRequest {
            federation_id: ids.federation_id,
            item_id: None,
            operation_id: Some(operation_id),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Rejected);
    Ok(())
}

#[tokio::test]
async fn cancel_pending_allocation_marks_item_and_wallet_cancelled() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-cancel")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Pending).await?;
    let operation_id = WalletOperationId("wallet-op-1".to_owned());
    seed_wallet_operation(
        &database,
        &ids.federation_id,
        &ids.item_id,
        &operation_id,
        WalletOperationStatus::Pending,
        None,
    )
    .await?;

    let response = cancel_allocation_with_database(
        &database,
        CancelAllocationRequest {
            federation_id: ids.federation_id.clone(),
            reason: Some("test cancellation".to_owned()),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Accepted);

    let operation = get_wallet_operation(&database, &operation_id).await?;
    assert_eq!(operation.status, WalletOperationStatus::Cancelled);
    assert_eq!(
        audit_count(&database, "cancel_allocation", "accepted").await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn cancel_action_required_item_is_safe() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-cancel-action-required")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::ActionRequired).await?;
    let response = cancel_allocation_with_database(
        &database,
        CancelAllocationRequest {
            federation_id: ids.federation_id,
            reason: None,
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Accepted);
    assert_eq!(
        response
            .allocation_status
            .expect("allocation")
            .item_statuses[0]
            .status,
        ItemAllocationStatus::Cancelled
    );
    Ok(())
}

#[tokio::test]
async fn cancel_returns_already_applied_for_cancelled_allocation() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-cancel-already")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Cancelled).await?;
    seed_wallet_operation(
        &database,
        &ids.federation_id,
        &ids.item_id,
        &WalletOperationId("wallet-op-1".to_owned()),
        WalletOperationStatus::Cancelled,
        None,
    )
    .await?;

    let response = cancel_allocation_with_database(
        &database,
        CancelAllocationRequest {
            federation_id: ids.federation_id,
            reason: Some("second cancel".to_owned()),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::AlreadyApplied);
    assert_eq!(
        audit_count(&database, "cancel_allocation", "already_applied").await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn cancel_rejects_broadcast_wallet_operation() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-cancel-broadcast")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Running).await?;
    seed_wallet_operation(
        &database,
        &ids.federation_id,
        &ids.item_id,
        &WalletOperationId("wallet-op-1".to_owned()),
        WalletOperationStatus::Broadcast,
        Some("txid-1"),
    )
    .await?;

    let response = cancel_allocation_with_database(
        &database,
        CancelAllocationRequest {
            federation_id: ids.federation_id,
            reason: None,
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Rejected);
    assert_eq!(
        audit_count(&database, "cancel_allocation", "rejected").await?,
        1
    );
    Ok(())
}

#[derive(Clone, Debug)]
struct SeedIds {
    federation_id: FederationId,
    item_id: ItemId,
}

/// Each resolution reaches the state the operator asserted, and every one
/// leaves an audit row. Escalation is what makes these reachable at all:
/// an operation under manual review is rejected by both retry and cancel.
#[tokio::test]
async fn each_manual_review_resolution_reaches_its_state() -> anyhow::Result<()> {
    for (case, resolution, txid, expected_status, expected_txid) in [
        (
            "completed",
            ManualReviewResolution::Completed,
            Some("operator-observed-tx"),
            WalletOperationStatus::Completed,
            Some("operator-observed-tx"),
        ),
        (
            "failed",
            ManualReviewResolution::Failed,
            None,
            WalletOperationStatus::Failed,
            None,
        ),
        (
            "retry",
            ManualReviewResolution::SafeToRetry,
            None,
            WalletOperationStatus::Pending,
            None,
        ),
    ] {
        let database =
            Database::connect(test_sqlite_path(&format!("resolve-review-{case}"))).await?;
        let ids = seed_allocation(&database, ItemAllocationStatus::ActionRequired).await?;
        let operation_id = WalletOperationId(format!("under-review-{case}"));
        seed_wallet_operation(
            &database,
            &ids.federation_id,
            &ids.item_id,
            &operation_id,
            WalletOperationStatus::ManualReviewRequired,
            None,
        )
        .await?;

        let response = resolve_manual_review_with_database(
            &database,
            ResolveManualReviewRequest {
                operation_id: operation_id.clone(),
                resolution,
                txid: txid.map(str::to_owned),
                reason: Some("reconciled with the gateway operator".to_owned()),
            },
        )
        .await?;

        assert_eq!(
            response.status,
            ManualOperationStatus::Accepted,
            "{case}: {:?}",
            response.detail
        );
        let operation = get_wallet_operation(&database, &operation_id).await?;
        assert_eq!(operation.status, expected_status, "{case}");
        assert_eq!(operation.txid.as_deref(), expected_txid, "{case}");
        assert_eq!(
            audit_count(&database, "resolve_manual_review", "accepted").await?,
            1,
            "{case}: the resolution is recorded"
        );
    }
    Ok(())
}

/// A completed resolution is an assertion about a specific transaction, so
/// it cannot be made without naming one, and the resolutions that assert no
/// send happened cannot carry one.
#[tokio::test]
async fn a_manual_review_resolution_must_match_its_txid() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("resolve-review-txid")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::ActionRequired).await?;
    let operation_id = WalletOperationId("txid-mismatch".to_owned());
    seed_wallet_operation(
        &database,
        &ids.federation_id,
        &ids.item_id,
        &operation_id,
        WalletOperationStatus::ManualReviewRequired,
        None,
    )
    .await?;

    let missing = resolve_manual_review_with_database(
        &database,
        ResolveManualReviewRequest {
            operation_id: operation_id.clone(),
            resolution: ManualReviewResolution::Completed,
            txid: None,
            reason: None,
        },
    )
    .await?;
    assert_eq!(missing.status, ManualOperationStatus::Rejected);

    let unexpected = resolve_manual_review_with_database(
        &database,
        ResolveManualReviewRequest {
            operation_id: operation_id.clone(),
            resolution: ManualReviewResolution::SafeToRetry,
            txid: Some("some-tx".to_owned()),
            reason: None,
        },
    )
    .await?;
    assert_eq!(unexpected.status, ManualOperationStatus::Rejected);

    let operation = get_wallet_operation(&database, &operation_id).await?;
    assert_eq!(
        operation.status,
        WalletOperationStatus::ManualReviewRequired,
        "a rejected resolution changes nothing"
    );
    Ok(())
}

/// The *writer* records the operator's txid without claiming exact output
/// attribution: `tx_vout` stays unset, because chain observation owns that
/// and an operator-supplied txid is not it.
///
/// This drives `resolve_manual_review_with_database` directly, so it does not
/// contradict the evidence requirement. That requirement lives one level up, in
/// `resolve_manual_review`, and is pinned by
/// `a_completed_resolution_without_chain_evidence_is_refused`. What this test
/// pins is that neither path fabricates a vout it does not have.
#[tokio::test]
async fn the_manual_review_writer_records_a_txid_without_claiming_a_vout() -> anyhow::Result<()> {
    let database =
        Database::connect(test_sqlite_path("resolve-review-without-output-evidence")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::ActionRequired).await?;
    let operation_id = WalletOperationId("reviewed-without-output-evidence".to_owned());
    seed_wallet_operation(
        &database,
        &ids.federation_id,
        &ids.item_id,
        &operation_id,
        WalletOperationStatus::ManualReviewRequired,
        None,
    )
    .await?;

    let asserted_txid = "a".repeat(64);
    let expected_address = "bcrt1q7sl62f7m9h8cwrphaxl28f4u6dktkcczwz8cks";
    let response = resolve_manual_review_with_database(
        &database,
        ResolveManualReviewRequest {
            operation_id: operation_id.clone(),
            resolution: ManualReviewResolution::Completed,
            txid: Some(asserted_txid.clone()),
            reason: None,
        },
    )
    .await?;

    assert_eq!(response.status, ManualOperationStatus::Accepted);
    let operation = get_wallet_operation(&database, &operation_id).await?;
    assert_eq!(operation.status, WalletOperationStatus::Completed);
    assert_eq!(operation.txid.as_deref(), Some(asserted_txid.as_str()));
    assert_eq!(operation.address.as_deref(), Some(expected_address));
    assert_eq!(operation.amount, Sats(10_000));
    assert_eq!(
        operation.tx_vout, None,
        "the seeded reviewed operation has no claimed exact output"
    );
    Ok(())
}

/// Only an operation actually under review can be resolved. An `in_doubt`
/// operation is one FLIP is still working on, and anything terminal was
/// already decided — by an operator or by evidence that arrived first.
#[tokio::test]
async fn resolution_applies_only_to_operations_under_review() -> anyhow::Result<()> {
    for (case, seeded, expected) in [
        (
            "in-doubt",
            WalletOperationStatus::InDoubt,
            ManualOperationStatus::Rejected,
        ),
        (
            "completed",
            WalletOperationStatus::Completed,
            ManualOperationStatus::AlreadyApplied,
        ),
    ] {
        let database =
            Database::connect(test_sqlite_path(&format!("resolve-guard-{case}"))).await?;
        let ids = seed_allocation(&database, ItemAllocationStatus::ActionRequired).await?;
        let operation_id = WalletOperationId(format!("guarded-{case}"));
        seed_wallet_operation(
            &database,
            &ids.federation_id,
            &ids.item_id,
            &operation_id,
            seeded,
            None,
        )
        .await?;

        let response = resolve_manual_review_with_database(
            &database,
            ResolveManualReviewRequest {
                operation_id: operation_id.clone(),
                resolution: ManualReviewResolution::SafeToRetry,
                txid: None,
                reason: None,
            },
        )
        .await?;

        assert_eq!(response.status, expected, "{case}");
        let operation = get_wallet_operation(&database, &operation_id).await?;
        assert_eq!(operation.status, seeded, "{case}: state is untouched");
    }
    Ok(())
}

/// The operator route out of a wedged binding, and what it refuses.
///
/// `SPEC-flip-rpc`'s second mechanism. The first —
/// takeover inside the admission path — handles the ordinary case with no
/// operator involved; this exists for when nobody else is asking for the
/// federation, or the operator needs it free now.
#[tokio::test]
async fn releasing_a_binding_frees_an_idle_federation() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-release-idle")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Failed).await?;

    let response = release_federation_allocation_with_database(
        &database,
        ReleaseFederationAllocationRequest {
            federation_id: ids.federation_id.clone(),
            reason: "requester vanished before funding".to_owned(),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Accepted);
    assert!(response.previous_requester.is_some());

    // The binding is gone, and so are the items that belonged to it.
    let allocations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM allocations")
        .fetch_one(database.pool())
        .await?;
    assert_eq!(allocations, 0);
    let items: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM allocation_items")
        .fetch_one(database.pool())
        .await?;
    assert_eq!(items, 0);
    assert_eq!(
        audit_count(&database, "release_federation_allocation", "accepted").await?,
        1
    );
    Ok(())
}

/// An allocation that still holds work is refused, and the refusal says
/// what it holds.
///
/// **This verb overrides who holds a federation, not whether the allocation
/// is idle.** Giving up on funding in flight is `cancel_allocation`'s
/// decision and stays a separate one.
#[tokio::test]
async fn releasing_a_binding_refuses_work_in_flight() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-release-busy")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Running).await?;

    let response = release_federation_allocation_with_database(
        &database,
        ReleaseFederationAllocationRequest {
            federation_id: ids.federation_id.clone(),
            reason: "wanted it back".to_owned(),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Rejected);
    let detail = response.detail.unwrap_or_default();
    assert!(
        detail.contains("1 reserving item(s)"),
        "the refusal must name what the allocation holds: {detail}"
    );

    let allocations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM allocations")
        .fetch_one(database.pool())
        .await?;
    assert_eq!(allocations, 1);
    Ok(())
}

/// A settled item does not make the allocation releasable while one of its
/// wallet operations is still awaiting settlement.
///
/// The item statuses and the operation statuses are separate terms because
/// they go terminal separately: the wallet sync path keeps acting on an
/// operation whose item has already finished.
#[tokio::test]
async fn releasing_a_binding_refuses_an_unsettled_wallet_operation() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-release-unsettled")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Failed).await?;
    seed_wallet_operation(
        &database,
        &ids.federation_id,
        &ids.item_id,
        &WalletOperationId("wallet-op-unsettled".to_owned()),
        WalletOperationStatus::Broadcast,
        None,
    )
    .await?;

    let response = release_federation_allocation_with_database(
        &database,
        ReleaseFederationAllocationRequest {
            federation_id: ids.federation_id.clone(),
            reason: "looks idle".to_owned(),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Rejected);
    let detail = response.detail.unwrap_or_default();
    assert!(
        detail.contains("1 wallet operation(s)"),
        "the refusal must name the unsettled operation: {detail}"
    );
    Ok(())
}

/// An empty reason is refused, and the refusal is audited.
#[tokio::test]
async fn releasing_a_binding_requires_a_reason() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-release-reason")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Failed).await?;

    let response = release_federation_allocation_with_database(
        &database,
        ReleaseFederationAllocationRequest {
            federation_id: ids.federation_id.clone(),
            reason: "   ".to_owned(),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Rejected);
    let allocations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM allocations")
        .fetch_one(database.pool())
        .await?;
    assert_eq!(allocations, 1, "a refused release must not delete anything");
    assert_eq!(
        audit_count(&database, "release_federation_allocation", "rejected").await?,
        1
    );
    Ok(())
}

/// An unknown federation is `not_found`, not an error.
#[tokio::test]
async fn releasing_an_unknown_federation_is_not_found() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("manual-release-missing")).await?;
    let response = release_federation_allocation_with_database(
        &database,
        ReleaseFederationAllocationRequest {
            federation_id: FederationId("never-seen".to_owned()),
            reason: "tidying up".to_owned(),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::NotFound);
    Ok(())
}

async fn seed_allocation(
    database: &Database,
    status: ItemAllocationStatus,
) -> anyhow::Result<SeedIds> {
    let federation_id = FederationId("federation-1".to_owned());
    AllocationSeed {
        federation_id: federation_id.clone(),
        items: vec![ItemSeed {
            status,
            failure_json: matches!(
                status,
                ItemAllocationStatus::ActionRequired | ItemAllocationStatus::Failed
            )
            .then(|| r#"{"code":"withdraw_failed","reason":"seeded failure"}"#.to_owned()),
            ..ItemSeed::default()
        }],
        ..AllocationSeed::default()
    }
    .insert(database)
    .await?;
    Ok(SeedIds {
        item_id: allocation_store::item_id(&federation_id, SourceType::Gateway),
        federation_id,
    })
}

/// A `completed` resolution requires chain evidence, and every case where
/// FLIP cannot obtain it is refused.
///
/// Evidence is required in every case. Refusing only a *visible contradiction*
/// is weaker, and leaves three routes to `completed` with no evidence at all —
/// an unreachable observer, a missing persisted address, and a txid the observer
/// does not know.
///
/// The operator route through those three cases is
/// `complete_review_without_evidence`, which records that no evidence
/// existed. See `a_completion_without_evidence_is_recorded_as_unverified`.
#[tokio::test]
async fn a_completed_resolution_without_chain_evidence_is_refused() -> anyhow::Result<()> {
    let address = "bcrt1q7sl62f7m9h8cwrphaxl28f4u6dktkcczwz8cks";
    let amount = Sats(10_000);

    // The control, and the only accepting case: the observer returned the
    // transaction and one of its outputs pays this operation exactly.
    // Without this half, every assertion below would pass against a
    // predicate that refused everything.
    let paying = StaticChainObserver {
        outputs: vec![chain_output("settling-txid", Some(address), amount.0)],
    };
    assert_eq!(
        chain_evidence_gap(&paying, "settling-txid", address, amount).await,
        None,
        "a transaction paying this operation must not be refused"
    );

    // A real transaction with the wrong destination.
    let elsewhere = StaticChainObserver {
        outputs: vec![chain_output(
            "unrelated-txid",
            Some("bcrt1qsomeotheraddressentirely00000000000000"),
            amount.0,
        )],
    };
    let detail = chain_evidence_gap(&elsewhere, "unrelated-txid", address, amount)
        .await
        .expect("a transaction paying another address must be refused");
    assert!(detail.contains("unrelated-txid"), "{detail}");

    // Right destination, wrong amount.
    let wrong_amount = StaticChainObserver {
        outputs: vec![chain_output("short-txid", Some(address), amount.0 - 1)],
    };
    assert!(
        chain_evidence_gap(&wrong_amount, "short-txid", address, amount)
            .await
            .is_some(),
        "a transaction paying the wrong amount must be refused"
    );

    // A txid the observer does not know: without the requirement it would
    // complete the operation on the operator's word alone.
    let unknown = StaticChainObserver { outputs: vec![] };
    let detail = chain_evidence_gap(&unknown, "unknown-txid", address, amount)
        .await
        .expect("an unknown transaction must be refused");
    assert!(
        detail.contains("does not know transaction unknown-txid"),
        "{detail}"
    );
    assert!(
        detail.contains("complete_review_without_evidence"),
        "a refusal must name the operator's route through it: {detail}"
    );

    // The observer cannot be reached at all, which is close to the situation
    // that produces reviewed operations.
    let unreachable = UnreachableChainObserver;
    let detail = chain_evidence_gap(&unreachable, "settling-txid", address, amount)
        .await
        .expect("an unreachable observer must be refused");
    assert!(detail.contains("could not be reached"), "{detail}");
    assert!(
        detail.contains("complete_review_without_evidence"),
        "{detail}"
    );

    Ok(())
}

/// The route through, and the record it leaves.
///
/// An unverified completion must not arrive through the verb that looks
/// verified. It must still be reachable, and it must be distinguishable
/// afterwards.
#[tokio::test]
async fn a_completion_without_evidence_is_recorded_as_unverified() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("complete-without-evidence")).await?;
    let operation_id = WalletOperationId("op-no-evidence".to_owned());
    seed_wallet_operation_without_item(
        &database,
        &FederationId("federation-no-evidence".to_owned()),
        &operation_id,
        WalletOperationStatus::ManualReviewRequired,
    )
    .await?;

    // An empty reason is refused: this writes a settlement FLIP could not
    // verify, so the audit log must say why the operator was sure.
    let rejected = complete_review_without_evidence(
        &database,
        CompleteReviewWithoutEvidenceRequest {
            operation_id: operation_id.clone(),
            txid: "asserted-txid".to_owned(),
            reason: "   ".to_owned(),
        },
    )
    .await?;
    assert_eq!(rejected.status, ManualOperationStatus::Rejected);

    let accepted = complete_review_without_evidence(
        &database,
        CompleteReviewWithoutEvidenceRequest {
            operation_id: operation_id.clone(),
            txid: "asserted-txid".to_owned(),
            reason: "confirmed with the gateway operator by phone".to_owned(),
        },
    )
    .await?;
    assert_eq!(accepted.status, ManualOperationStatus::Accepted);

    let operation = wallet::get_wallet_operation(&database, &operation_id).await?;
    assert_eq!(operation.status, WalletOperationStatus::Completed);
    assert_eq!(operation.txid.as_deref(), Some("asserted-txid"));
    // Chain observation owns exact output attribution, and this path has
    // none, so the vout must stay unset exactly as on the verified path.
    assert_eq!(operation.tx_vout, None);

    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = ? AND detail_json LIKE ?",
    )
    .bind("complete_review_without_evidence")
    .bind("%without chain evidence%")
    .fetch_one(database.pool())
    .await?;
    assert_eq!(
        audited, 1,
        "the completion must leave exactly one audit row saying it was unverified"
    );

    // A second call finds the operation no longer under review and refuses,
    // rather than completing it twice.
    let repeat = complete_review_without_evidence(
        &database,
        CompleteReviewWithoutEvidenceRequest {
            operation_id,
            txid: "asserted-txid".to_owned(),
            reason: "same again".to_owned(),
        },
    )
    .await?;
    assert_eq!(repeat.status, ManualOperationStatus::Rejected);
    Ok(())
}

struct UnreachableChainObserver;

#[async_trait::async_trait]
impl crate::chain_observer::ChainObserver for UnreachableChainObserver {
    async fn health(&self) -> anyhow::Result<crate::chain_observer::ChainObserverHealth> {
        anyhow::bail!("observer down")
    }

    async fn tx_evidence(
        &self,
        _txid: &str,
    ) -> anyhow::Result<Option<crate::chain_observer::TxEvidence>> {
        anyhow::bail!("connection refused")
    }

    async fn address_evidence(
        &self,
        _address: &str,
    ) -> anyhow::Result<crate::chain_observer::AddressEvidence> {
        anyhow::bail!("connection refused")
    }
}

fn chain_output(
    txid: &str,
    address: Option<&str>,
    amount_sats: u64,
) -> crate::chain_observer::ChainOutputEvidence {
    crate::chain_observer::ChainOutputEvidence {
        txid: txid.to_owned(),
        vout: 0,
        address: address.map(str::to_owned),
        script_pubkey: String::new(),
        amount_sats,
        confirmations: 6,
    }
}

struct StaticChainObserver {
    outputs: Vec<crate::chain_observer::ChainOutputEvidence>,
}

#[async_trait::async_trait]
impl crate::chain_observer::ChainObserver for StaticChainObserver {
    async fn health(&self) -> anyhow::Result<crate::chain_observer::ChainObserverHealth> {
        Ok(crate::chain_observer::ChainObserverHealth {
            reachable: true,
            detail: None,
        })
    }

    async fn tx_evidence(
        &self,
        txid: &str,
    ) -> anyhow::Result<Option<crate::chain_observer::TxEvidence>> {
        let outputs = self
            .outputs
            .iter()
            .filter(|output| output.txid == txid)
            .cloned()
            .collect::<Vec<_>>();
        Ok(
            (!outputs.is_empty()).then(|| crate::chain_observer::TxEvidence {
                txid: txid.to_owned(),
                confirmations: 6,
                outputs,
            }),
        )
    }

    async fn address_evidence(
        &self,
        address: &str,
    ) -> anyhow::Result<crate::chain_observer::AddressEvidence> {
        Ok(crate::chain_observer::AddressEvidence {
            address: address.to_owned(),
            outputs: Vec::new(),
        })
    }
}

async fn seed_wallet_operation(
    database: &Database,
    federation_id: &FederationId,
    item_id: &ItemId,
    operation_id: &WalletOperationId,
    status: WalletOperationStatus,
    txid: Option<&str>,
) -> anyhow::Result<()> {
    let mut tx = database.begin_write().await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: operation_id.clone(),
            operation_type: WalletOperationType::GatewayFunding,
            status,
            amount: Sats(10_000),
            address: Some("bcrt1q7sl62f7m9h8cwrphaxl28f4u6dktkcczwz8cks".to_owned()),
            label: Some("seeded op".to_owned()),
            fee_rate_sat_per_vbyte: Some(1),
            federation_id: Some(federation_id.clone()),
            item_id: Some(item_id.clone()),
        },
    )
    .await?;
    tx.commit().await?;
    if let Some(txid) = txid {
        sqlx::query("UPDATE wallet_operations SET txid = ? WHERE operation_id = ?")
            .bind(txid)
            .bind(&operation_id.0)
            .execute(database.pool())
            .await?;
    }
    Ok(())
}

/// A truthfully-resolved `Completed` review keeps charging the wallet until
/// a balance observation covers it.
///
/// Without the watermark this arm writes, the row leaves the
/// pending-settlement statuses on completion and matches neither branch of
/// `active_wallet_withdrawal_amount_tx`, so the debit disappears from the
/// budget while the money has already left. The next admission then sees
/// capacity that was already spent. No lying operator is required: the send
/// really happened and the txid is the settling one.
#[tokio::test]
async fn a_completed_review_stays_charged_until_an_observation_covers_it() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("review-keeps-charging")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::ActionRequired).await?;
    let operation_id = WalletOperationId("reviewed-keeps-charging".to_owned());
    seed_wallet_operation_without_item(
        &database,
        &ids.federation_id,
        &operation_id,
        WalletOperationStatus::ManualReviewRequired,
    )
    .await?;

    // A deployment that has ever read its wallet balance has this row, and
    // the watermark comparison is inert without it.
    crate::wallet::observe_balance_serially(
        &database,
        &crate::wallet::WalletBackendBalance {
            network: fedi_decentralized_service_liquidity_manager::BitcoinNetwork::Regtest,
            spendable: Sats(1_000_000),
            observed_at: crate::now_timestamp(),
        },
    )
    .await?;

    let charged_before = {
        let mut tx = database.begin_write().await?;
        let amount = crate::wallet::active_wallet_withdrawal_amount_tx(&mut tx).await?;
        tx.commit().await?;
        amount
    };
    assert_eq!(
        charged_before,
        Sats(10_000),
        "an operation under review is pending settlement and is charged"
    );

    let response = resolve_manual_review_with_database(
        &database,
        ResolveManualReviewRequest {
            operation_id: operation_id.clone(),
            resolution: ManualReviewResolution::Completed,
            txid: Some("b".repeat(64)),
            reason: None,
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Accepted);

    let watermark: Option<i64> =
        sqlx::query_scalar("SELECT settled_tick FROM wallet_operations WHERE operation_id = ?")
            .bind(&operation_id.0)
            .fetch_one(database.pool())
            .await?;
    assert!(
        watermark.is_some(),
        "settling through manual review must stamp the observation watermark"
    );

    let charged_after = {
        let mut tx = database.begin_write().await?;
        let amount = crate::wallet::active_wallet_withdrawal_amount_tx(&mut tx).await?;
        tx.commit().await?;
        amount
    };
    assert_eq!(
        charged_after,
        Sats(10_000),
        "the debit stays charged until an observation advances past the watermark"
    );

    Ok(())
}

/// The item-linked case, which is the normal one.
///
/// A funding send under manual review belongs to an item that is still
/// reserving, so `active_wallet_withdrawal_amount_tx` excludes its row while
/// the reserved term covers it. Before the release watermark the settle
/// stamp was taken at resolution and then expired unobserved — observations
/// advance every thirty seconds or so, and the item goes terminal a pass
/// later — so the moment the exclusion lifted the debit was already outside
/// both terms and the next admission saw capacity that was spent.
///
/// The previous test for this repair seeded an operation with no item, which
/// is the one shape that never takes the exclusion. It passed without
/// covering any of this.
#[tokio::test]
async fn an_item_linked_send_stays_charged_after_its_item_stops_reserving() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("item-linked-keeps-charging")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Running).await?;
    let operation_id = WalletOperationId("item-linked-keeps-charging".to_owned());
    seed_wallet_operation(
        &database,
        &ids.federation_id,
        &ids.item_id,
        &operation_id,
        WalletOperationStatus::ManualReviewRequired,
        None,
    )
    .await?;
    observe_balance(&database).await?;

    resolve_manual_review_with_database(
        &database,
        ResolveManualReviewRequest {
            operation_id: operation_id.clone(),
            resolution: ManualReviewResolution::Completed,
            txid: Some("c".repeat(64)),
            reason: None,
        },
    )
    .await?;

    // The row is invisible to the term while its item reserves, and the
    // reserved term is what covers the amount here.
    assert_eq!(charged(&database).await?, Sats(0));

    // Time passes as it does in a running deployment: the observation
    // sequence advances well past anything stamped at resolution.
    for _ in 0..4 {
        observe_balance(&database).await?;
    }

    // The exclusion lifts. Any of the four production sites that take an
    // item terminal produces this state; the term cannot tell them apart.
    sqlx::query("UPDATE allocation_items SET status = 'failed' WHERE item_id = ?")
        .bind(&ids.item_id.0)
        .execute(database.pool())
        .await?;

    assert_eq!(
        charged(&database).await?,
        Sats(10_000),
        "the debit must stay charged when the exclusion lifts: no observation has \
         been read since it became chargeable"
    );

    // One observation is enough, because ticks order it against the stamp.
    // `observe_balance` takes a tick before it reads, and that tick is later
    // than the one the release stamp took, so this observation provably
    // began after the row became chargeable. The write-order counter this
    // replaced could not tell the two apart and needed a second cycle.
    observe_balance(&database).await?;
    assert_eq!(
        charged(&database).await?,
        Sats(0),
        "an observation read after the release stamp covers the debit"
    );

    Ok(())
}

/// A reset send must be chargeable again.
///
/// Every watermark writer guards on its column being NULL, so a row that
/// kept its stamps through a reset could never be stamped for the second
/// send, and the second send's debit would never be charged.
#[tokio::test]
async fn resetting_a_send_clears_both_watermarks() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("reset-clears-watermarks")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::Running).await?;
    let operation_id = WalletOperationId("reset-clears-watermarks".to_owned());
    seed_wallet_operation(
        &database,
        &ids.federation_id,
        &ids.item_id,
        &operation_id,
        WalletOperationStatus::ManualReviewRequired,
        None,
    )
    .await?;
    observe_balance(&database).await?;

    resolve_manual_review_with_database(
        &database,
        ResolveManualReviewRequest {
            operation_id: operation_id.clone(),
            resolution: ManualReviewResolution::Completed,
            txid: Some("d".repeat(64)),
            reason: None,
        },
    )
    .await?;
    sqlx::query("UPDATE allocation_items SET status = 'failed' WHERE item_id = ?")
        .bind(&ids.item_id.0)
        .execute(database.pool())
        .await?;
    // Take the release stamp, so the reset has both to clear.
    assert_eq!(charged(&database).await?, Sats(10_000));
    assert!(watermarks(&database, &operation_id).await?.0.is_some());
    assert!(watermarks(&database, &operation_id).await?.1.is_some());

    {
        let mut tx = database.begin_write().await?;
        crate::wallet::reset_wallet_operation_tx(&mut tx, &operation_id).await?;
        tx.commit().await?;
    }

    let (settled, released) = watermarks(&database, &operation_id).await?;
    assert_eq!(settled, None, "reset must clear the settle watermark");
    assert_eq!(released, None, "reset must clear the release watermark");

    Ok(())
}

/// A balance read that began *before* a settlement must not release it.
///
/// `get_funds_with_wallet` and `request_withdrawal_with_wallet` each read the
/// backend, await, and persist. Stamping the observation with a write-order
/// counter taken at persist time would let a slow reply record a balance read
/// from *before* a settlement while advancing the count past that settlement's
/// stamp: the debit stops being subtracted while the money is gone, and the next
/// admission spends it twice.
///
/// The interleaving is explicit here rather than timed: the read point is
/// taken first, the settlement happens, and only then is the stale balance
/// persisted.
#[tokio::test]
async fn a_balance_read_before_a_settlement_does_not_release_it() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("read-before-settle")).await?;
    let ids = seed_allocation(&database, ItemAllocationStatus::ActionRequired).await?;
    let operation_id = WalletOperationId("read-before-settle".to_owned());
    seed_wallet_operation_without_item(
        &database,
        &ids.federation_id,
        &operation_id,
        WalletOperationStatus::ManualReviewRequired,
    )
    .await?;
    observe_balance(&database).await?;

    // The slow reply starts here, before the send settles.
    let stale_read = crate::wallet::begin_balance_read(&database).await?;

    resolve_manual_review_with_database(
        &database,
        ResolveManualReviewRequest {
            operation_id: operation_id.clone(),
            resolution: ManualReviewResolution::Completed,
            txid: Some("e".repeat(64)),
            reason: None,
        },
    )
    .await?;

    // ...and lands after it, carrying a balance that cannot include the debit.
    crate::wallet::upsert_wallet_balance_observation(
        &database,
        &crate::wallet::WalletBackendBalance {
            network: fedi_decentralized_service_liquidity_manager::BitcoinNetwork::Regtest,
            spendable: Sats(1_000_000),
            observed_at: crate::now_timestamp(),
        },
        stale_read,
    )
    .await?;

    assert_eq!(
        charged(&database).await?,
        Sats(10_000),
        "a balance whose read began before the settlement cannot release it"
    );

    // A read that begins afterwards can, and does.
    observe_balance(&database).await?;
    assert_eq!(
        charged(&database).await?,
        Sats(0),
        "a balance read after the settlement accounts for the debit"
    );

    Ok(())
}

async fn observe_balance(database: &Database) -> anyhow::Result<()> {
    crate::wallet::observe_balance_serially(
        database,
        &crate::wallet::WalletBackendBalance {
            network: fedi_decentralized_service_liquidity_manager::BitcoinNetwork::Regtest,
            spendable: Sats(1_000_000),
            observed_at: crate::now_timestamp(),
        },
    )
    .await?;
    Ok(())
}

async fn charged(database: &Database) -> anyhow::Result<Sats> {
    let mut tx = database.begin_write().await?;
    let amount = crate::wallet::active_wallet_withdrawal_amount_tx(&mut tx).await?;
    tx.commit().await?;
    Ok(amount)
}

async fn watermarks(
    database: &Database,
    operation_id: &WalletOperationId,
) -> anyhow::Result<(Option<i64>, Option<i64>)> {
    let row = sqlx::query(
        "SELECT settled_tick, released_tick \
         FROM wallet_operations WHERE operation_id = ?",
    )
    .bind(&operation_id.0)
    .fetch_one(database.pool())
    .await?;
    Ok((row.try_get("settled_tick")?, row.try_get("released_tick")?))
}

async fn seed_wallet_operation_without_item(
    database: &Database,
    federation_id: &FederationId,
    operation_id: &WalletOperationId,
    status: WalletOperationStatus,
) -> anyhow::Result<()> {
    let mut tx = database.begin_write().await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: operation_id.clone(),
            operation_type: WalletOperationType::GatewayFunding,
            status,
            amount: Sats(10_000),
            address: Some("bcrt1q7sl62f7m9h8cwrphaxl28f4u6dktkcczwz8cks".to_owned()),
            label: Some("seeded unattached op".to_owned()),
            fee_rate_sat_per_vbyte: Some(1),
            federation_id: Some(federation_id.clone()),
            item_id: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn audit_count(database: &Database, action: &str, outcome: &str) -> anyhow::Result<i64> {
    let pattern = format!("%\"outcome\":\"{outcome}\"%");
    Ok(
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE action = ? AND detail_json LIKE ?",
        )
        .bind(action)
        .bind(pattern)
        .fetch_one(database.pool())
        .await?,
    )
}
