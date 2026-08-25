use std::sync::Arc;

use async_trait::async_trait;
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use bitcoin::{Address, CompressedPublicKey, Network};
use fedi_decentralized_service_liquidity_manager::Timestamp;
use fedi_decentralized_service_liquidity_manager::WalletOperationType;
use fedi_decentralized_service_liquidity_manager::{
    AdvertisementConfig, AttestationSummary, BitcoinNetwork, CapacityConfig, CapacityMode,
    ChainObserverBackendView, ChainObserverConfigView, CompletionEvidence, DurationSecs,
    FederationId, FundingPolicyConfig, GatewayConfigView, GatewayId, GatewayName,
    ItemAllocationStatus, ItemId, LiquidityFailureCode, ProviderPolicy, ReplenishmentConfig,
    RpcEndpointAddress, RpcEndpointConfig, RpcEndpointId, RpcProtocolName, RpcTransport,
    SourceType, Url,
};
use tokio::sync::{Mutex, Notify};

use super::*;
use crate::allocation_store::{
    FundingTargetRecord, StabilityPoolAllocationStep, load_allocation_status_by_federation,
};
use crate::stability_pool::PegInAddress;
use crate::test_support::{AllocationSeed, ItemSeed, test_sqlite_path};
use crate::wallet::{SyncedWalletStatus, TestFundsWallet, WalletOperationSync};
use crate::wallet::{WalletOperationInput, insert_wallet_operation_tx};
use crate::wallet::{apply_sync_update, wallet_operation_for_item};
use fedi_decentralized_service_liquidity_manager::WalletOperationId;

#[tokio::test]
async fn stability_pool_allocation_completes_with_deposit_evidence() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("stability-completion")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) =
        seed_stability_pool_allocation(&database, &setup, Sats(25_000), None).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let backend = FakeStabilityPoolBackend::new(regtest_address());

    process_stability_pool_allocations_with(&database, &setup, &wallet, &backend).await?;
    assert_eq!(wallet.submitted_count().await, 1);
    let operation = wallet_operation_for_item(
        &database,
        WalletOperationType::StabilityPoolFunding,
        &item_id,
    )
    .await?
    .expect("wallet operation exists");
    assert_eq!(operation.status, WalletOperationStatus::Broadcast);
    assert_eq!(operation.amount, Sats(25_100));

    apply_sync_update(
        &database,
        &WalletOperationSync {
            operation_id: operation.operation_id.clone(),
            status: SyncedWalletStatus::Completed,
            txid: Some("txid-1".to_owned()),
            confirmation_count: Some(1),
            amount: None,
            detail: None,
        },
    )
    .await?;
    backend
        .set_peg_in_status(PegInStatus::Claimed {
            amount: Sats(25_000),
        })
        .await;

    process_stability_pool_allocations_with(&database, &setup, &wallet, &backend).await?;
    assert_eq!(backend.submitted_deposit_count().await, 1);
    let submitted_amount = backend.submitted_deposit_amount().await.expect("submitted");
    assert_eq!(submitted_amount, Sats(25_000));
    backend
        .set_deposit_status(StabilityDepositStatus::Success)
        .await;
    backend.set_observed_provided_amount(submitted_amount).await;

    process_stability_pool_allocations_with(&database, &setup, &wallet, &backend).await?;
    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::Completed
    );
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::Completed
    );
    assert_eq!(
        status.item_statuses[0].fulfilled_amount,
        Some(submitted_amount)
    );
    match status.item_statuses[0]
        .completion_evidence
        .as_ref()
        .expect("completion evidence exists")
    {
        CompletionEvidence::StabilityPool(evidence) => {
            assert_eq!(evidence.fulfilled_amount, submitted_amount);
            assert_eq!(evidence.observed_provided_amount, submitted_amount);
            assert!(evidence.peg_in_operation_id.is_some());
            assert!(evidence.stability_pool_deposit_operation_id.is_some());
        }
        other => panic!("expected stability-pool evidence, got {other:?}"),
    }
    Ok(())
}

/// A target allocation is already durable in the target client before the
/// worker records its operation/address in SQLite. Model that window by
/// dropping the worker between the two, then run the normal retry from the
/// unchanged durable item.
///
/// The retry must recover the *same* target operation, not mint a second
/// one. Upstream takes no caller-supplied operation id here, so the fence
/// is the client's unused-address pool: an address nothing has claimed is
/// reused rather than replaced.
///
/// Two operation ids — `["peg-in-op-1", "peg-in-op-2"]` — is the failure this
/// guards against.
#[tokio::test]
async fn crash_after_target_peg_in_allocation_reuses_the_same_target_operation()
-> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("peg-in-allocation-crash")).await?;
    let setup = test_setup_config();
    seed_stability_pool_allocation(
        &database,
        &setup,
        Sats(25_000),
        Some(StabilityPoolAllocationStep {
            target_client_opened_at: Some(now_timestamp()),
            ..StabilityPoolAllocationStep::default()
        }),
    )
    .await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let backend = FakeStabilityPoolBackend::new(regtest_address());
    let crash_point = backend.pause_after_next_peg_in_allocation().await;

    // Drive the real worker to its test-only pause after the external
    // allocation and drop it there, modelling a hard process crash before
    // its next SQLite write.
    {
        let processing =
            process_stability_pool_allocations_with(&database, &setup, &wallet, &backend);
        tokio::pin!(processing);
        tokio::select! {
            result = &mut processing => panic!("worker exited before crash point: {result:?}"),
            () = crash_point.allocated.notified() => {}
        }
    }

    let before_retry = load_stability_step(&database).await?;
    assert_eq!(before_retry.peg_in_operation_id, None);
    assert_eq!(before_retry.peg_in_address, None);

    // The retry runs to the wallet boundary, where a second test-only pause
    // stops it before any provider value moves.
    let (wallet_submission_started, _) = wallet.pause_before_submission().await;
    {
        let processing =
            process_stability_pool_allocations_with(&database, &setup, &wallet, &backend);
        tokio::pin!(processing);
        tokio::select! {
            result = &mut processing => panic!("worker exited before wallet boundary: {result:?}"),
            () = wallet_submission_started.notified() => {}
        }
    }

    // One target operation exists, not two: the retry reused the address the
    // lost call minted.
    let allocated = backend.allocated_peg_in_operations().await;
    assert_eq!(
        allocated
            .iter()
            .map(|peg_in| &peg_in.operation_id)
            .collect::<Vec<_>>(),
        vec!["peg-in-op-1"],
        "the retry must recover the lost allocation, not mint a second one"
    );
    let persisted = load_stability_step(&database).await?;
    assert_eq!(
        persisted.peg_in_operation_id.as_deref(),
        Some("peg-in-op-1"),
        "the item must record the operation the target client actually holds"
    );
    assert_eq!(wallet.submitted_count().await, 0);
    assert_eq!(backend.submitted_deposit_count().await, 0);

    Ok(())
}

/// A spent budget must stop the item *between* phases, not inside one.
///
/// A `tokio::time::timeout` wrapped around the whole item would drop its future
/// at whatever await it happens to be suspended on, including a durable write,
/// so a terminal state could be observed and then lost.
///
/// The budget is a deadline the item consults at phase boundaries, so a spent
/// budget declines to *start* the next piece of work. That is observable
/// only as work the item did not do: with the budget already spent on
/// entry, the item must not ask the target anything and must not allocate,
/// and it must leave state the next pass resumes from.
///
/// The item is seeded with its client already open so the run reaches the
/// first phase boundary rather than erroring inside the bounded client
/// open, and the budget is zero so the test turns on ordering rather than
/// on timing — it cannot become load-dependent.
#[tokio::test]
async fn a_spent_budget_stops_an_item_between_phases() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("stability-phase-boundary")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) = seed_stability_pool_allocation(
        &database,
        &setup,
        Sats(25_000),
        Some(StabilityPoolAllocationStep {
            target_client_opened_at: Some(now_timestamp()),
            ..StabilityPoolAllocationStep::default()
        }),
    )
    .await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let backend = FakeStabilityPoolBackend::new(regtest_address());

    // Spent before the item starts, so the first phase boundary stops it.
    process_with_item_budget(
        &database,
        &setup,
        &wallet,
        &backend,
        std::time::Duration::ZERO,
    )
    .await?;

    assert_eq!(
        backend.check_target_calls().await,
        0,
        "a spent budget must not start the target check"
    );
    assert!(
        backend.allocated_peg_in_operations().await.is_empty(),
        "a spent budget must not mint a peg-in address"
    );
    assert!(
        wallet_operation_for_item(
            &database,
            WalletOperationType::StabilityPoolFunding,
            &item_id,
        )
        .await?
        .is_none(),
        "a spent budget must not begin a funding send"
    );

    // Stopped, not failed: exceeding a budget says nothing about whether the
    // target is usable, and the next pass must be able to resume.
    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert!(
        !matches!(
            status.item_statuses[0].status,
            ItemAllocationStatus::Failed | ItemAllocationStatus::Completed
        ),
        "a budget stop must leave the item resumable, found {:?}",
        status.item_statuses[0].status
    );

    // The same item completes normally once it has a budget, which proves
    // the stop above left nothing behind that blocks it.
    process_stability_pool_allocations_with(&database, &setup, &wallet, &backend).await?;
    assert_eq!(
        backend.check_target_calls().await,
        1,
        "the next pass must resume the item it stopped"
    );
    assert_eq!(
        backend.allocated_peg_in_operations().await.len(),
        1,
        "the resumed pass must mint exactly one address"
    );

    Ok(())
}

#[tokio::test]
async fn cancelled_wallet_operation_marks_stability_item_failed() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("stability-cancelled-wallet")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) =
        seed_stability_pool_allocation(&database, &setup, Sats(25_000), None).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let backend = FakeStabilityPoolBackend::new(regtest_address());

    process_stability_pool_allocations_with(&database, &setup, &wallet, &backend).await?;
    let operation = wallet_operation_for_item(
        &database,
        WalletOperationType::StabilityPoolFunding,
        &item_id,
    )
    .await?
    .expect("wallet operation exists");
    sqlx::query("UPDATE wallet_operations SET status = 'cancelled' WHERE operation_id = ?")
        .bind(&operation.operation_id.0)
        .execute(database.pool())
        .await?;

    process_stability_pool_allocations_with(&database, &setup, &wallet, &backend).await?;
    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(status.item_statuses[0].status, ItemAllocationStatus::Failed);
    assert_eq!(
        status.item_statuses[0]
            .failure
            .as_ref()
            .map(|failure| failure.code),
        Some(LiquidityFailureCode::WithdrawFailed)
    );
    Ok(())
}

/// The worker's own fence, independent of what acceptance checked.
///
/// Acceptance verifies the previewed config, but the client the worker
/// funds is cached by federation id alone, and a federation id does not
/// commit the module map. An earlier initialization for the same id can
/// therefore have stored a different config than the one the request was
/// accepted against, and nothing reconciled the two: the worker allocated
/// an ordinary wallet peg-in address — which an unusable target accepts as
/// readily as a usable one — created its wallet operation, and sent. The
/// first stability-module lookup came after the peg-in was claimed, by
/// which time provider value was already inside a client that could never
/// deposit it.
#[tokio::test]
async fn an_unusable_target_fails_the_item_before_any_funding() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("stability-unusable-target")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) =
        seed_stability_pool_allocation(&database, &setup, Sats(25_000), None).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let backend = FakeStabilityPoolBackend::new(regtest_address());
    backend
        .set_target_check(Some(TargetCheck::Unusable(
            "target federation has no usable stability-pool module".to_owned(),
        )))
        .await;

    process_stability_pool_allocations_with(&database, &setup, &wallet, &backend).await?;

    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(status.item_statuses[0].status, ItemAllocationStatus::Failed);
    assert_eq!(
        status.item_statuses[0]
            .failure
            .as_ref()
            .map(|failure| failure.code),
        Some(LiquidityFailureCode::StabilityPoolFailed)
    );
    // The point of the fence is the absence of this row. Failing after the
    // send would release the reservation just the same, and leave the value
    // stranded.
    assert!(
        wallet_operation_for_item(
            &database,
            WalletOperationType::StabilityPoolFunding,
            &item_id,
        )
        .await?
        .is_none(),
        "no wallet operation may exist for a target that cannot be funded"
    );
    Ok(())
}

/// One unresponsive target must not stop every other allocation.
///
/// The worker advances its snapshot serially and a target Fedimint client
/// can leave an await pending indefinitely, so before the per-item budget
/// the first stuck federation held the pass forever — and held every later
/// tick too, since the interval task awaits the whole pass. The healthy
/// item behind it was never reached, on that pass or any other.
#[tokio::test]
async fn a_stuck_target_does_not_stop_the_item_behind_it() -> anyhow::Result<()> {
    use crate::test_support::{AllocationSeed, ItemSeed};

    let database = Database::connect(test_sqlite_path("stability-starvation")).await?;
    let setup = test_setup_config();
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let backend = FakeStabilityPoolBackend::new(regtest_address());

    // Ids chosen so the stuck target sorts first, which is the only
    // arrangement that exercises the property. `active_item_rows` orders by
    // `updated_at ASC, item_id ASC`, both rows are seeded inside one
    // `unixepoch()` second, so the item id decides — and with the obvious
    // names the *healthy* item would run first and the test would pass
    // without ever reaching an item behind a stuck one.
    for federation_id in ["federation-a-stuck", "federation-b-healthy"] {
        AllocationSeed {
            federation_id: FederationId(federation_id.to_owned()),
            committed_amount: Sats(5_000),
            reserved_amount: Sats(6_000),
            items: vec![ItemSeed {
                source_type: SourceType::StabilityPool,
                committed_amount: Sats(5_000),
                reserved_amount: Sats(6_000),
                ..ItemSeed::default()
            }],
            ..AllocationSeed::default()
        }
        .insert(&database)
        .await?;
    }
    backend.set_unresponsive("federation-a-stuck").await;

    // Guards the ordering the rest of this test depends on: if a future
    // rename flips it, this fails loudly rather than passing vacuously.
    let ordered = allocation_store::active_stability_pool_items(&database).await?;
    assert_eq!(
        ordered.first().map(|item| item.federation_id.0.as_str()),
        Some("federation-a-stuck"),
        "the stuck item must be processed first for this test to mean anything"
    );

    // Without a budget this call never returns.
    process_with_item_budget(
        &database,
        &setup,
        &wallet,
        &backend,
        std::time::Duration::from_millis(50),
    )
    .await?;

    // The healthy item behind the stuck one was reached and funded.
    let healthy_item = allocation_store::item_id(
        &FederationId("federation-b-healthy".to_owned()),
        SourceType::StabilityPool,
    );
    assert!(
        wallet_operation_for_item(
            &database,
            WalletOperationType::StabilityPoolFunding,
            &healthy_item,
        )
        .await?
        .is_some(),
        "the item behind an unresponsive target must still be processed"
    );

    // The stuck item is untouched rather than failed: it exceeded a budget,
    // which says nothing about whether its target is usable.
    let stuck_status = load_allocation_status_by_federation(
        &database,
        &FederationId("federation-a-stuck".to_owned()),
    )
    .await?
    .expect("stuck allocation exists");
    assert_ne!(
        stuck_status.item_statuses[0].status,
        ItemAllocationStatus::Failed
    );
    Ok(())
}

/// A responsive deposit stream must record its terminal state even when the
/// provider-account report does not answer.
///
/// With the durable write behind `observe_stability_pool`, which issues two
/// federation calls no budget bounds, every invocation can drain the deposit
/// stream to `Success`, return `Ok`, and still leave the item recorded as
/// `initiated` — for an unbounded number of passes. The report gate decides
/// completion; it must not decide whether the observation was recorded at all.
#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_deposit_is_recorded_even_when_the_report_hangs() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("stability-report-hangs")).await?;
    let setup = test_setup_config();
    let item_id = ItemId("federation-1:stability_pool".to_owned());
    let (federation_id, _) = seed_stability_pool_allocation(
        &database,
        &setup,
        Sats(25_000),
        Some(StabilityPoolAllocationStep {
            target_client_opened_at: Some(now_timestamp()),
            peg_in_operation_id: Some("peg-in-op-1".to_owned()),
            peg_in_address: Some(regtest_address()),
            wallet_operation_id: Some(
                "wallet-stability-pool-funding-federation-1-stability_pool".to_owned(),
            ),
            peg_in_status: Some(PegInProgress::Claimed),
            peg_in_amount: Some(Sats(25_000)),
            sp_deposit_status: Some(SpDepositStatus::Initiated),
            // Has to parse as a Fedimint operation id: the deposit is now
            // looked up by the caller-owned id rather than inferred, so an
            // unparseable placeholder would fail before reaching the arm
            // under test.
            sp_deposit_operation_id: Some(
                "1111111111111111111111111111111111111111111111111111111111111111".to_owned(),
            ),
            sp_deposit_amount: Some(Sats(25_000)),
            sp_deposit_min_fee_rate_ppb: Some(0),
            observed_provided_amount: Some(Sats(0)),
        }),
    )
    .await?;
    seed_completed_wallet_operation(&database, &federation_id, &item_id).await?;

    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let backend = FakeStabilityPoolBackend::new(regtest_address());
    backend
        .set_peg_in_status(PegInStatus::Claimed {
            amount: Sats(25_000),
        })
        .await;
    backend
        .set_deposit_status(StabilityDepositStatus::Success)
        .await;
    // The deposit stream answers; the provider report does not. Those are
    // different federation calls, and only the first is what a responsive
    // invocation is defined over.
    backend.set_report_hangs().await;

    // Generous on purpose. The budget only has to outlast the item's real
    // work and cut the report, which never returns — and a tight one made
    // this fail under full-suite contention while passing in isolation,
    // which is the worst kind of test.
    process_with_item_budget(
        &database,
        &setup,
        &wallet,
        &backend,
        std::time::Duration::from_secs(2),
    )
    .await?;

    let item = allocation_store::stability_pool_item(&database, &federation_id)
        .await?
        .expect("the item is still active");
    assert_eq!(
        item.step.sp_deposit_status,
        Some(SpDepositStatus::Success),
        "a drained deposit stream must commit its terminal state before the report gate"
    );
    Ok(())
}

/// A target the mint-time check refuses must fail its own item and let the
/// pass continue. Returning it as an error would abort the pass for every
/// item behind it, which is the failure the per-item budget exists to stop.
#[tokio::test]
async fn a_target_refused_at_mint_time_fails_only_its_own_item() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("stability-mint-refused")).await?;
    let setup = test_setup_config();
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let backend = FakeStabilityPoolBackend::new(regtest_address());

    // Both are refused: `set_peg_in_unusable` is global to the fake. The
    // second id says so, because the point of the test is that the pass
    // reaches a second refused item rather than aborting on the first.
    for federation_id in ["federation-a-refused", "federation-b-also-refused"] {
        AllocationSeed {
            federation_id: FederationId(federation_id.to_owned()),
            committed_amount: Sats(5_000),
            reserved_amount: Sats(6_000),
            items: vec![ItemSeed {
                source_type: SourceType::StabilityPool,
                committed_amount: Sats(5_000),
                reserved_amount: Sats(6_000),
                ..ItemSeed::default()
            }],
            ..AllocationSeed::default()
        }
        .insert(&database)
        .await?;
    }
    backend
        .set_peg_in_unusable("target client config hash is not the accepted one")
        .await;

    // Both items are refused here, which is what makes the assertion about
    // the *pass* meaningful: it has to reach the second one at all.
    let advanced =
        process_stability_pool_allocations_with(&database, &setup, &wallet, &backend).await?;
    assert_eq!(advanced, 2, "the pass must reach and decide both items");

    for federation_id in ["federation-a-refused", "federation-b-also-refused"] {
        let status = load_allocation_status_by_federation(
            &database,
            &FederationId(federation_id.to_owned()),
        )
        .await?
        .expect("allocation exists");
        assert_eq!(
            status.item_statuses[0].status,
            ItemAllocationStatus::Failed,
            "{federation_id} must be failed as a decision, not left active by an error"
        );
    }
    Ok(())
}

/// An unavailable answer is not a decision. A target check that cannot run
/// this tick must leave the item alone, or a transient failure to reach the
/// federation would permanently fail allocations that are perfectly valid.
#[tokio::test]
async fn an_unavailable_target_check_leaves_the_item_active() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("stability-target-check-down")).await?;
    let setup = test_setup_config();
    let (federation_id, _item_id) =
        seed_stability_pool_allocation(&database, &setup, Sats(25_000), None).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let backend = FakeStabilityPoolBackend::new(regtest_address());
    backend.set_target_check(None).await;

    process_stability_pool_allocations_with(&database, &setup, &wallet, &backend).await?;

    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_ne!(status.item_statuses[0].status, ItemAllocationStatus::Failed);
    Ok(())
}

/// Old interrupted rows lack the actual operation ID and fee tuple. They
/// fail closed regardless of what a bounded diagnostic scan happens to show.
#[tokio::test]
async fn legacy_incomplete_submission_requires_operator_action() -> anyhow::Result<()> {
    let opened_at = now_timestamp();
    let (database, setup, wallet, backend, federation_id) =
        interrupted_submit_fixture("legacy", opened_at).await?;
    backend
        .set_deposit_log(
            vec![deposit_log_entry(
                "diagnostic-only",
                Sats(25_000),
                opened_at,
            )],
            false,
        )
        .await;
    assert_action_required(&database, &setup, &wallet, &backend, &federation_id).await
}

async fn assert_action_required(
    database: &Database,
    setup: &SetupConfigView,
    wallet: &TestFundsWallet,
    backend: &FakeStabilityPoolBackend,
    federation_id: &FederationId,
) -> anyhow::Result<()> {
    process_stability_pool_allocations_with(database, setup, wallet, backend).await?;
    assert_eq!(backend.submitted_deposit_count().await, 0);

    // The item is not selected any more, so no later pass can resubmit it.
    process_stability_pool_allocations_with(database, setup, wallet, backend).await?;
    assert_eq!(backend.submitted_deposit_count().await, 0);

    let status = load_allocation_status_by_federation(database, federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::ActionRequired
    );
    Ok(())
}

fn deposit_log_entry(
    operation_id: &str,
    amount: Sats,
    created_at: Timestamp,
) -> crate::stability_pool::TargetDepositOperation {
    crate::stability_pool::TargetDepositOperation {
        operation_id: operation_id.to_owned(),
        amount,
        // The crash happened before anything drained the stream, so the
        // client has cached no outcome for it.
        outcome: None,
        created_at: created_at.0,
    }
}

async fn load_stability_step(
    database: &Database,
) -> anyhow::Result<allocation_store::StabilityPoolAllocationStep> {
    let step_json: String = sqlx::query_scalar(
        "SELECT step_json FROM allocation_items WHERE source_type = 'stability_pool'",
    )
    .fetch_one(database.pool())
    .await?;
    Ok(serde_json::from_str(&step_json)?)
}

/// An item stopped in the window: peg-in claimed, funding settled, the
/// A cancel committed after the worker's snapshot stops the deposit.
///
/// This is the resume path: the step already reads `submitting` with its
/// operation id recorded, so the block holding the first-pass
/// compare-and-set is skipped entirely. Without a fence immediately before
/// the call, an item cancelled after the worker loaded its item list still
/// moved provider value into the target pool — the worker iterates serially,
/// so the snapshot is as old as every preceding item's pass, and
/// `cancel_allocation` does not refuse, because it guards on the item's
/// wallet operations and a stability deposit is not one.
#[tokio::test]
async fn a_cancelled_item_does_not_submit_its_prepared_deposit() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("stability-cancel-before-deposit")).await?;
    let setup = test_setup_config();
    let item_id = ItemId("federation-1:stability_pool".to_owned());
    let (federation_id, _) = seed_stability_pool_allocation(
        &database,
        &setup,
        Sats(25_000),
        Some(StabilityPoolAllocationStep {
            target_client_opened_at: Some(Timestamp(1)),
            peg_in_operation_id: Some("peg-in-op-1".to_owned()),
            peg_in_address: Some(regtest_address()),
            wallet_operation_id: Some(
                "wallet-stability-pool-funding-federation-1-stability_pool".to_owned(),
            ),
            peg_in_status: Some(PegInProgress::Claimed),
            peg_in_amount: Some(Sats(25_000)),
            sp_deposit_status: Some(SpDepositStatus::Submitting),
            sp_deposit_operation_id: Some(
                "2222222222222222222222222222222222222222222222222222222222222222".to_owned(),
            ),
            sp_deposit_amount: Some(Sats(25_000)),
            sp_deposit_min_fee_rate_ppb: Some(0),
            observed_provided_amount: Some(Sats(0)),
        }),
    )
    .await?;
    seed_completed_wallet_operation(&database, &federation_id, &item_id).await?;
    let backend = FakeStabilityPoolBackend::new(regtest_address());
    backend
        .set_peg_in_status(PegInStatus::Claimed {
            amount: Sats(25_000),
        })
        .await;
    backend.set_observed_provided_amount(Sats(0)).await;

    // The worker's snapshot, taken when it loaded its item list.
    let item = allocation_store::stability_pool_item(&database, &federation_id)
        .await?
        .expect("the seeded item is loadable");

    // The operator cancels while the worker is still on the items ahead of
    // this one.
    let mut tx = database.begin_write().await?;
    allocation_store::cancel_item_tx(&mut tx, &federation_id, &item.item_id).await?;
    tx.commit().await?;

    let advanced = advance_stability_deposit_with(
        &database,
        &setup,
        &backend,
        item,
        Sats(25_000),
        ItemBudget::starting_now(std::time::Duration::from_secs(30)),
        StabilityDepositSubmission::generate,
    )
    .await?;

    assert!(!advanced, "a cancelled item made no durable progress");
    assert_eq!(
        backend.submitted_deposit_count().await,
        0,
        "a cancelled item must not move provider value into the target pool"
    );
    Ok(())
}

/// deposit marked `submitting` with no operation id recorded.
async fn interrupted_submit_fixture(
    name: &str,
    opened_at: Timestamp,
) -> anyhow::Result<(
    Database,
    SetupConfigView,
    TestFundsWallet,
    FakeStabilityPoolBackend,
    FederationId,
)> {
    let database =
        Database::connect(test_sqlite_path(&format!("stability-interrupted-{name}"))).await?;
    let setup = test_setup_config();
    let item_id = ItemId("federation-1:stability_pool".to_owned());
    let (federation_id, _) = seed_stability_pool_allocation(
        &database,
        &setup,
        Sats(25_000),
        Some(StabilityPoolAllocationStep {
            target_client_opened_at: Some(opened_at),
            peg_in_operation_id: Some("peg-in-op-1".to_owned()),
            peg_in_address: Some(regtest_address()),
            wallet_operation_id: Some(
                "wallet-stability-pool-funding-federation-1-stability_pool".to_owned(),
            ),
            peg_in_status: Some(PegInProgress::Claimed),
            peg_in_amount: Some(Sats(25_000)),
            sp_deposit_status: Some(SpDepositStatus::Submitting),
            sp_deposit_operation_id: None,
            sp_deposit_amount: Some(Sats(25_000)),
            sp_deposit_min_fee_rate_ppb: None,
            observed_provided_amount: Some(Sats(0)),
        }),
    )
    .await?;
    seed_completed_wallet_operation(&database, &federation_id, &item_id).await?;

    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let backend = FakeStabilityPoolBackend::new(regtest_address());
    backend
        .set_peg_in_status(PegInStatus::Claimed {
            amount: Sats(25_000),
        })
        .await;
    backend.set_observed_provided_amount(Sats(0)).await;
    Ok((database, setup, wallet, backend, federation_id))
}

/// No log line in this worker may carry a target peg-in address.
///
/// A peg-in address is private federation data, and logs are read by people the
/// federation has not authorised. An operator who needs it has
/// `inspect_target_client` on the authenticated Admin API.
///
/// This reads the source rather than capturing output, and that is
/// deliberate. A capturing subscriber installed with `set_default` is
/// thread-local, but `tracing` caches callsite interest **globally**: a
/// sibling test reaching the same callsite with no subscriber installed can
/// leave it disabled for this one, which made the capturing version pass
/// alone and fail about one run in three under the full suite. Reading the
/// source is deterministic, and it covers every invocation in the file
/// rather than only the ones a single pass happens to reach.
#[test]
fn no_log_line_in_this_worker_carries_a_peg_in_address() {
    let source = include_str!("../src/stability_allocation.rs");
    let mut offenders = Vec::new();

    for (number, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        let is_log = [
            "tracing::info!",
            "tracing::warn!",
            "tracing::error!",
            "tracing::debug!",
            "tracing::trace!",
            "info!(",
            "warn!(",
            "error!(",
            "debug!(",
            "trace!(",
        ]
        .iter()
        .any(|macro_name| trimmed.starts_with(macro_name));
        // A field that carries a value, not the word "address" in a
        // message. `"stability: allocating peg-in address"` is a constant
        // string and discloses nothing; `address = %peg_in.address` is the
        // defect.
        let carries_address_value = trimmed.contains("address =")
            || trimmed.contains("address=")
            || trimmed.contains("%address")
            || trimmed.contains("?address");
        if is_log && carries_address_value {
            offenders.push(format!("{}: {trimmed}", number + 1));
        }
    }

    assert!(
        offenders.is_empty(),
        "a log line carries an address; the target peg-in address is private \
         federation data and an operator who needs it has inspect_target_client \
         on the authenticated Admin API:\n{}",
        offenders.join("\n")
    );

    // Control: the scan must actually be looking at log lines. Without this
    // it would pass just as well against an empty file or a broken matcher.
    let log_lines = source
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("tracing::info!") || trimmed.starts_with("tracing::warn!")
        })
        .count();
    assert!(
        log_lines > 0,
        "the scan found no log lines at all, so it proves nothing"
    );
}

/// The e-cash is already claimed by the target client once the peg-in
/// completes, so a shortfall in spendable balance must not end the item.
///
/// Notes selected by an in-flight stability transaction are invisible to this
/// reading and come back if that transaction rejects; a terminal status would
/// strand them in a client no workflow touches again.
#[tokio::test]
async fn short_target_balance_after_peg_in_does_not_strand_ecash() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("stability-short-balance")).await?;
    let setup = test_setup_config();
    let item_id = ItemId("federation-1:stability_pool".to_owned());
    let (federation_id, _) = seed_stability_pool_allocation(
        &database,
        &setup,
        Sats(25_000),
        Some(StabilityPoolAllocationStep {
            target_client_opened_at: Some(now_timestamp()),
            peg_in_operation_id: Some("peg-in-op-1".to_owned()),
            peg_in_address: Some(regtest_address()),
            wallet_operation_id: Some(
                "wallet-stability-pool-funding-federation-1-stability_pool".to_owned(),
            ),
            peg_in_status: Some(PegInProgress::Claimed),
            peg_in_amount: Some(Sats(25_000)),
            sp_deposit_status: None,
            sp_deposit_operation_id: None,
            sp_deposit_amount: None,
            sp_deposit_min_fee_rate_ppb: None,
            observed_provided_amount: Some(Sats(0)),
        }),
    )
    .await?;
    seed_completed_wallet_operation(&database, &federation_id, &item_id).await?;

    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let backend = FakeStabilityPoolBackend::new(regtest_address());
    backend
        .set_peg_in_status(PegInStatus::Claimed {
            amount: Sats(25_000),
        })
        .await;
    // An in-flight transaction is holding the claimed notes.
    backend.set_wallet_balance(Sats(0)).await;

    process_stability_pool_allocations_with(&database, &setup, &wallet, &backend).await?;

    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::ActionRequired,
        "claimed e-cash must not be abandoned by a terminal status"
    );
    assert_eq!(backend.submitted_deposit_count().await, 0);
    Ok(())
}

/// A sibling deposit can lift the provider account's aggregate over this
/// item's committed amount without this item ever having deposited. The
/// aggregate is not attributable, so it must not complete the item — and it
/// plays no part in the interrupted-submit decision either, which is made
/// from the client's operation log.
#[tokio::test]
async fn unrelated_stability_balance_does_not_complete_submitting_item() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("stability-submitting-aggregate")).await?;
    let setup = test_setup_config();
    let item_id = ItemId("federation-1:stability_pool".to_owned());
    let (federation_id, _) = seed_stability_pool_allocation(
        &database,
        &setup,
        Sats(25_000),
        Some(StabilityPoolAllocationStep {
            target_client_opened_at: Some(now_timestamp()),
            peg_in_operation_id: Some("peg-in-op-1".to_owned()),
            peg_in_address: Some(regtest_address()),
            wallet_operation_id: Some(
                "wallet-stability-pool-funding-federation-1-stability_pool".to_owned(),
            ),
            peg_in_status: Some(PegInProgress::Claimed),
            peg_in_amount: Some(Sats(25_000)),
            sp_deposit_status: Some(SpDepositStatus::Submitting),
            sp_deposit_operation_id: None,
            sp_deposit_amount: Some(Sats(25_000)),
            sp_deposit_min_fee_rate_ppb: None,
            observed_provided_amount: Some(Sats(0)),
        }),
    )
    .await?;
    seed_completed_wallet_operation(&database, &federation_id, &item_id).await?;

    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let backend = FakeStabilityPoolBackend::new(regtest_address());
    backend
        .set_peg_in_status(PegInStatus::Claimed {
            amount: Sats(25_000),
        })
        .await;
    // Unrelated provider-account activity already covers the committed amount.
    backend.set_observed_provided_amount(Sats(80_000)).await;

    process_stability_pool_allocations_with(&database, &setup, &wallet, &backend).await?;

    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_ne!(
        status.item_statuses[0].status,
        ItemAllocationStatus::Completed,
        "an unattributable aggregate must not complete the item"
    );
    let step = load_stability_step(&database).await?;
    assert_eq!(
        step.observed_provided_amount,
        Some(Sats(0)),
        "the aggregate must not be recorded as this item's provided liquidity"
    );
    Ok(())
}

async fn seed_stability_pool_allocation(
    database: &Database,
    setup: &SetupConfigView,
    amount: Sats,
    step: Option<StabilityPoolAllocationStep>,
) -> anyhow::Result<(FederationId, ItemId)> {
    let federation_id = FederationId("federation-1".to_owned());
    let item_id = allocation_store::item_id(&federation_id, SourceType::StabilityPool);
    let reserved_amount = Sats(
        amount
            .0
            .checked_add(setup.funding_policy.fee_reserve.0)
            .expect("test stability reserve fits"),
    );
    AllocationSeed {
        federation_id: federation_id.clone(),
        network: setup.network.to_string(),
        committed_amount: amount,
        reserved_amount,
        items: vec![ItemSeed {
            source_type: SourceType::StabilityPool,
            committed_amount: amount,
            reserved_amount,
            step_json: step.map(|step| serde_json::to_string(&step)).transpose()?,
            ..ItemSeed::default()
        }],
        ..AllocationSeed::default()
    }
    .insert(database)
    .await?;
    Ok((federation_id, item_id))
}

async fn seed_completed_wallet_operation(
    database: &Database,
    federation_id: &FederationId,
    item_id: &ItemId,
) -> anyhow::Result<()> {
    let mut tx = database.begin_write().await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId(
                "wallet-stability-pool-funding-federation-1-stability_pool".to_owned(),
            ),
            operation_type: WalletOperationType::StabilityPoolFunding,
            status: WalletOperationStatus::Completed,
            amount: Sats(25_100),
            address: Some(regtest_address()),
            label: Some("seeded stability funding".to_owned()),
            fee_rate_sat_per_vbyte: Some(1),
            federation_id: Some(federation_id.clone()),
            item_id: Some(item_id.clone()),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

fn test_setup_config() -> SetupConfigView {
    let mut funding_policy = FundingPolicyConfig::defaults_for_network(BitcoinNetwork::Regtest);
    funding_policy.fee_reserve = Sats(100);

    SetupConfigView {
        network: BitcoinNetwork::Regtest,
        gateway: GatewayConfigView {
            gateway_id: GatewayId("gateway-1".to_owned()),
            gateway_name: GatewayName("Gateway One".to_owned()),
            admin_url: "http://127.0.0.1:8175".to_owned(),
            has_admin_credential: true,
            identity_metadata: Vec::new(),
        },
        chain_observer: ChainObserverConfigView {
            backend: ChainObserverBackendView::Esplora {
                url: Url("http://127.0.0.1:3002".to_owned()),
            },
        },
        relays: Vec::new(),
        capacity: CapacityConfig {
            mode: CapacityMode::AvailableFunds,
            explicit_cap: None,
            supported_sources: vec![SourceType::Gateway, SourceType::StabilityPool],
        },
        funding_policy,
        replenishment: ReplenishmentConfig {
            warning_threshold: Sats(10_000),
            critical_threshold: Sats(5_000),
        },
        advertised_endpoint: RpcEndpointConfig {
            endpoint_id: Some(RpcEndpointId("endpoint-1".to_owned())),
            transport: RpcTransport::Iroh,
            address: RpcEndpointAddress("iroh-node-id".to_owned()),
            discovery_hints: Vec::new(),
            rpc_protocol_name: RpcProtocolName("fedi/flip/public-liquidity/1".to_owned()),
        },
        advertisement: AdvertisementConfig {
            republish_interval: DurationSecs(600),
            ready_advertisement_enabled: true,
        },
        provider_display: None,
        policy: ProviderPolicy {
            accepted_attester_policies: Vec::new(),
            supported_networks: vec![BitcoinNetwork::Regtest],
        },
        attestation_summary: AttestationSummary::default(),
    }
}

fn regtest_address() -> String {
    let secp = Secp256k1::new();
    let secret_key = SecretKey::from_slice(&[2_u8; 32]).expect("valid test secret key");
    let public_key = CompressedPublicKey(bitcoin::secp256k1::PublicKey::from_secret_key(
        &secp,
        &secret_key,
    ));
    Address::p2wpkh(&public_key, Network::Regtest).to_string()
}

#[derive(Clone, Debug)]
struct FakeStabilityPoolBackend {
    inner: Arc<Mutex<FakeStabilityPoolState>>,
    allocation_crash_point: Arc<Mutex<Option<Arc<PegInAllocationCrashPoint>>>>,
}

#[derive(Debug)]
struct PegInAllocationCrashPoint {
    allocated: Notify,
}

#[derive(Clone, Debug)]
struct FakeStabilityPoolState {
    peg_in_address: String,
    allocated_peg_in_operations: Vec<PegInAddress>,
    peg_in_status: PegInStatus,
    deposit_status: StabilityDepositStatus,
    wallet_balance: Sats,
    observed_provided_amount: Sats,
    submitted_deposits: Vec<StabilityDepositSubmission>,
    deposit_operations: Vec<crate::stability_pool::TargetDepositOperation>,
    deposit_scan_complete: bool,
    /// `None` stands for the check being unavailable this tick.
    target_check: Option<TargetCheck>,

    /// Federations whose target check never returns, standing in for a
    /// client whose transport makes no progress.
    unresponsive: std::collections::BTreeSet<String>,

    /// Reason `allocate_peg_in_address` refuses the target, if any.
    peg_in_unusable: Option<String>,

    /// Whether the provider-account report never returns. The deposit
    /// stream can be perfectly responsive while this one is not: they are
    /// different federation calls, and only the first is what
    /// "responsive invocation" is defined over.
    report_hangs: bool,

    /// How many times the worker asked for the target check. A phase
    /// boundary is only observable as work the item did *not* start, so a
    /// test for one needs a count rather than a state.
    check_target_calls: usize,
}

impl FakeStabilityPoolBackend {
    fn new(peg_in_address: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeStabilityPoolState {
                peg_in_address,
                allocated_peg_in_operations: Vec::new(),
                peg_in_status: PegInStatus::WaitingForTransaction,
                deposit_status: StabilityDepositStatus::Initiated,
                wallet_balance: Sats(25_000),
                observed_provided_amount: Sats(0),
                submitted_deposits: Vec::new(),
                deposit_operations: Vec::new(),
                deposit_scan_complete: true,
                target_check: Some(TargetCheck::Usable),
                unresponsive: std::collections::BTreeSet::new(),
                peg_in_unusable: None,
                report_hangs: false,
                check_target_calls: 0,
            })),
            allocation_crash_point: Arc::new(Mutex::new(None)),
        }
    }

    /// What the pre-funding target check answers. `None` stands for the
    /// answer being unavailable this tick; `Unusable` for a target no later
    /// tick will make fundable.
    async fn set_target_check(&self, check: Option<TargetCheck>) {
        self.inner.lock().await.target_check = check;
    }

    /// Makes this federation's client calls hang forever, as an
    /// unresponsive peer leaves the client's version negotiation hanging.
    async fn set_unresponsive(&self, federation_id: &str) {
        self.inner
            .lock()
            .await
            .unresponsive
            .insert(federation_id.to_owned());
    }

    /// Makes the address allocation refuse the target, as it does when the
    /// handle it would mint on is not the accepted one.
    async fn set_peg_in_unusable(&self, reason: &str) {
        self.inner.lock().await.peg_in_unusable = Some(reason.to_owned());
    }

    /// Makes the provider-account report hang while the deposit stream
    /// stays responsive.
    async fn set_report_hangs(&self) {
        self.inner.lock().await.report_hangs = true;
    }

    async fn check_target_calls(&self) -> usize {
        self.inner.lock().await.check_target_calls
    }

    /// Never returns for a target `set_unresponsive` named.
    ///
    /// This existed as a field nothing read until 2026-08-11, so the
    /// starvation regression test processed two healthy items and passed
    /// whatever the worker did with its budget. Hanging here rather than
    /// returning an error is the point: the budget exists to bound an await
    /// that never resolves, and an `Err` would exercise the other arm.
    async fn block_if_unresponsive(&self, target: &FundingTargetRecord) {
        let blocked = self
            .inner
            .lock()
            .await
            .unresponsive
            .contains(&target.federation_id.0);
        if blocked {
            std::future::pending::<()>().await;
        }
    }

    /// Pauses one test allocation after the fake target has made the
    /// operation durable but before the worker can persist its SQLite step.
    async fn pause_after_next_peg_in_allocation(&self) -> Arc<PegInAllocationCrashPoint> {
        let crash_point = Arc::new(PegInAllocationCrashPoint {
            allocated: Notify::new(),
        });
        *self.allocation_crash_point.lock().await = Some(crash_point.clone());
        crash_point
    }

    /// Spendable e-cash the target client reports. A stability transaction
    /// that has selected the notes hides them from this reading until it
    /// resolves, so a low value is not necessarily a permanent shortfall.
    async fn set_wallet_balance(&self, balance: Sats) {
        self.inner.lock().await.wallet_balance = balance;
    }

    async fn set_peg_in_status(&self, status: PegInStatus) {
        self.inner.lock().await.peg_in_status = status;
    }

    async fn set_deposit_status(&self, status: StabilityDepositStatus) {
        self.inner.lock().await.deposit_status = status;
    }

    async fn set_observed_provided_amount(&self, amount: Sats) {
        self.inner.lock().await.observed_provided_amount = amount;
    }

    async fn submitted_deposit_count(&self) -> usize {
        self.inner.lock().await.submitted_deposits.len()
    }

    async fn submitted_deposit_amount(&self) -> Option<Sats> {
        self.inner
            .lock()
            .await
            .submitted_deposits
            .last()
            .copied()
            .map(StabilityDepositSubmission::amount)
    }

    async fn allocated_peg_in_operations(&self) -> Vec<PegInAddress> {
        self.inner.lock().await.allocated_peg_in_operations.clone()
    }

    /// Seeds what the target client would say it deposited, and whether its
    /// history could be read in full.
    async fn set_deposit_log(
        &self,
        operations: Vec<crate::stability_pool::TargetDepositOperation>,
        complete: bool,
    ) {
        let mut inner = self.inner.lock().await;
        inner.deposit_operations = operations;
        inner.deposit_scan_complete = complete;
    }
}

#[async_trait]
impl StabilityPoolBackend for FakeStabilityPoolBackend {
    async fn ensure_client(&self, target: &FundingTargetRecord) -> anyhow::Result<()> {
        self.block_if_unresponsive(target).await;
        Ok(())
    }

    async fn check_target(&self, target: &FundingTargetRecord) -> anyhow::Result<TargetCheck> {
        self.inner.lock().await.check_target_calls += 1;
        self.block_if_unresponsive(target).await;
        self.inner
            .lock()
            .await
            .target_check
            .clone()
            .ok_or_else(|| anyhow::anyhow!("target check unavailable"))
    }

    async fn allocate_peg_in_address(
        &self,
        _target: &FundingTargetRecord,
    ) -> anyhow::Result<PegInAllocation> {
        if let Some(reason) = self.inner.lock().await.peg_in_unusable.clone() {
            return Ok(PegInAllocation::Unusable(reason));
        }
        // Models the unused-address pool the production backend allocates
        // from, which is what makes the call idempotent for a caller that
        // loses its result. Upstream reuses an allocated address whose
        // `PegInTweakIndexData::claimed` is empty and mints fresh only when
        // none is, taking the oldest by `creation_time`; here the fake's
        // single `peg_in_status` stands for that claim state.
        //
        // A fake that minted a distinct id per call would make the
        // production reuse invisible, so a regression there would not fail
        // any test.
        let mut state = self.inner.lock().await;
        let already_claimed = matches!(state.peg_in_status, PegInStatus::Claimed { .. });
        let reusable = if already_claimed {
            None
        } else {
            state.allocated_peg_in_operations.first().cloned()
        };
        let peg_in = match reusable {
            Some(unused) => unused,
            None => {
                let peg_in = PegInAddress {
                    operation_id: format!(
                        "peg-in-op-{}",
                        state.allocated_peg_in_operations.len() + 1
                    ),
                    address: state.peg_in_address.clone(),
                };
                state.allocated_peg_in_operations.push(peg_in.clone());
                peg_in
            }
        };
        drop(state);
        if let Some(crash_point) = self.allocation_crash_point.lock().await.take() {
            crash_point.allocated.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(PegInAllocation::Allocated(peg_in))
    }

    async fn observe_peg_in(
        &self,
        _target: &FundingTargetRecord,
        _operation_id: &str,
    ) -> anyhow::Result<PegInStatus> {
        Ok(self.inner.lock().await.peg_in_status.clone())
    }

    async fn recheck_peg_in(
        &self,
        _target: &FundingTargetRecord,
        _operation_id: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn target_wallet_balance(&self, _target: &FundingTargetRecord) -> anyhow::Result<Sats> {
        Ok(self.inner.lock().await.wallet_balance)
    }

    async fn submit_deposit_to_provide(
        &self,
        _target: &FundingTargetRecord,
        submission: StabilityDepositSubmission,
        _diagnostic_item_id: &str,
    ) -> anyhow::Result<SubmissionReceipt> {
        let mut state = self.inner.lock().await;
        state.submitted_deposits.push(submission);
        Ok(SubmissionReceipt::Submitted)
    }

    async fn observe_deposit(
        &self,
        _target: &FundingTargetRecord,
        _operation_id: crate::stability_deposit::StabilityDepositOperationId,
    ) -> anyhow::Result<StabilityDepositStatus> {
        Ok(self.inner.lock().await.deposit_status.clone())
    }

    async fn report(&self, _target: &FundingTargetRecord) -> anyhow::Result<StabilityPoolReport> {
        if self.inner.lock().await.report_hangs {
            std::future::pending::<()>().await;
        }
        Ok(StabilityPoolReport {
            observed_provided_amount: self.inner.lock().await.observed_provided_amount,
            liquidity_stats_json: "{\"locked_provides_sum_msat\":0,\"staged_provides_sum_msat\":0}"
                .to_owned(),
        })
    }

    async fn list_deposit_operations(
        &self,
        _target: &FundingTargetRecord,
    ) -> anyhow::Result<crate::stability_pool::TargetDepositScan> {
        let inner = self.inner.lock().await;
        Ok(crate::stability_pool::TargetDepositScan {
            operations: inner.deposit_operations.clone(),
            complete: inner.deposit_scan_complete,
        })
    }

    async fn get_deposit_operation(
        &self,
        _target: &FundingTargetRecord,
        operation_id: crate::stability_deposit::StabilityDepositOperationId,
    ) -> anyhow::Result<Option<crate::stability_pool::TargetDepositOperation>> {
        Ok(self
            .inner
            .lock()
            .await
            .deposit_operations
            .iter()
            .find(|operation| operation.operation_id == operation_id.to_string())
            .cloned())
    }
}
