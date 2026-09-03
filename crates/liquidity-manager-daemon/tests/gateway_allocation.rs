use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use bitcoin::{Address, CompressedPublicKey, Network};
use fedi_decentralized_service_liquidity_manager::{
    AdvertisementConfig, AllocationItemTarget, AttestationSummary, BitcoinNetwork,
    CancelAllocationRequest, CapacityConfig, CapacityMode, ChainObserverBackendView,
    ChainObserverConfigView, DurationSecs, FederationId, FundingPolicyConfig, GatewayApiUrl,
    GatewayConfigView, GatewayId, GatewayName, ItemAllocationStatus, ItemId, LiquidityFailureCode,
    ManualOperationStatus, ManualReviewResolution, ProviderPolicy, ReplenishmentConfig,
    ResolveManualReviewRequest, RpcEndpointAddress, RpcEndpointConfig, RpcEndpointId,
    RpcProtocolName, RpcTransport, SourceType, Url, WalletOperationType,
};
use tokio::sync::Mutex;

use super::*;
use crate::allocation_store::load_allocation_status_by_federation;
use crate::gateway::{GatewayDepositClaim, GatewayFederationSnapshot, GatewaySnapshot};
use crate::manual_ops::{
    cancel_allocation_with_database, resolve_manual_review_with_database_for_test,
};
use crate::test_support::{AllocationSeed, ItemSeed, test_sqlite_path};
use crate::wallet::{SyncedWalletStatus, TestFundsWallet, WalletOperationSync};
use crate::wallet::{
    apply_sync_update, mark_operation_failed, mark_operation_in_doubt, mark_withdrawal_broadcast,
    wallet_operation_for_item,
};

#[tokio::test]
async fn gateway_allocation_in_doubt_is_not_resubmitted() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("gateway-in-doubt")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) = seed_gateway_allocation(&database, &setup, Sats(25_000)).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    wallet.set_submit_in_doubt("lost gatewayd response").await;
    let gateway = FakeGateway::new(setup.network, regtest_address());

    let advanced = process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    assert_eq!(advanced, 1);
    assert_eq!(wallet.submitted_count().await, 1);

    let operation =
        wallet_operation_for_item(&database, WalletOperationType::GatewayFunding, &item_id)
            .await?
            .expect("wallet operation exists");
    assert_eq!(operation.status, WalletOperationStatus::InDoubt);

    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    assert_eq!(wallet.submitted_count().await, 1);
    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::Running
    );
    Ok(())
}

/// This deliberately records the unsafe manual-recovery boundary: the test
/// wallet accepts the first send but loses its response, then an incorrect
/// `SafeToRetry` assertion makes the resumed worker submit again.
#[tokio::test]
async fn manual_safe_to_retry_resubmits_an_accepted_unknown_gateway_send() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("gateway-manual-retry-duplicate")).await?;
    let setup = test_setup_config();
    let (_federation_id, item_id) =
        seed_gateway_allocation(&database, &setup, Sats(25_000)).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    // `TestFundsWallet` records the prepared send before returning this
    // transport-style error, modelling an externally accepted send whose
    // response/txid FLIP did not receive.
    wallet
        .set_submit_in_doubt("accepted send; response lost")
        .await;
    let gateway = FakeGateway::new(setup.network, regtest_address());

    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    assert_eq!(wallet.submitted_count().await, 1);
    let operation =
        wallet_operation_for_item(&database, WalletOperationType::GatewayFunding, &item_id)
            .await?
            .expect("wallet operation exists");
    assert_eq!(operation.status, WalletOperationStatus::InDoubt);
    assert_eq!(operation.txid, None, "the first response was lost");

    assert!(
        crate::wallet::escalate_in_doubt_to_manual_review(
            &database,
            &operation.operation_id,
            0,
            "no chain or target-side evidence is available",
        )
        .await?
    );
    let operation = crate::wallet::get_wallet_operation(&database, &operation.operation_id).await?;
    assert_eq!(
        operation.status,
        WalletOperationStatus::ManualReviewRequired
    );

    let response = resolve_manual_review_with_database_for_test(
        &database,
        ResolveManualReviewRequest {
            operation_id: operation.operation_id.clone(),
            resolution: ManualReviewResolution::SafeToRetry,
            txid: None,
            reason: Some("mistakenly concluded the first send did not happen".to_owned()),
        },
    )
    .await?;
    assert_eq!(response.status, ManualOperationStatus::Accepted);
    assert_eq!(
        crate::wallet::get_wallet_operation(&database, &operation.operation_id)
            .await?
            .status,
        WalletOperationStatus::Pending
    );

    wallet.set_submit_success("second-send-txid").await;
    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    assert_eq!(
        wallet.submitted_count().await,
        2,
        "the resumed worker sent the persisted operation a second time"
    );
    Ok(())
}

#[tokio::test]
async fn cancellation_winning_submission_race_prevents_send_and_stale_writes() -> anyhow::Result<()>
{
    let database = Database::connect(test_sqlite_path("gateway-cancel-wins-race")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) = seed_gateway_allocation(&database, &setup, Sats(25_000)).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let mut item = allocation_store::active_gateway_items(&database)
        .await?
        .pop()
        .expect("active item exists");
    item.step.deposit_address = Some(regtest_address());
    allocation_store::update_item_step(&database, &item_id, &item.step).await?;
    let operation = allocation_funding::ensure_wallet_operation(
        &database,
        &setup,
        &gateway_funding_step(&item),
    )
    .await?
    .expect("pending operation created");

    let cancelled = cancel_allocation_with_database(
        &database,
        CancelAllocationRequest {
            federation_id: federation_id.clone(),
            reason: Some("race winner".to_owned()),
        },
    )
    .await?;
    assert_eq!(cancelled.status, ManualOperationStatus::Accepted);

    allocation_funding::submit_funding_withdrawal(
        &database,
        &setup,
        &wallet,
        &gateway_funding_step(&item),
        &operation.operation_id,
    )
    .await?;
    assert_eq!(wallet.submitted_count().await, 0);

    allocation_store::fail_item(
        &database,
        &federation_id,
        &item_id,
        LiquidityFailureCode::WithdrawFailed,
        "stale worker failure",
    )
    .await?;
    item.step.gateway_connected = true;
    allocation_store::update_item_step(&database, &item_id, &item.step).await?;
    apply_sync_update(
        &database,
        &WalletOperationSync {
            operation_id: operation.operation_id.clone(),
            status: SyncedWalletStatus::Completed,
            txid: Some("stale-txid".to_owned()),
            confirmation_count: Some(1),
            amount: None,
            detail: None,
        },
    )
    .await?;
    mark_withdrawal_broadcast(&database, &operation.operation_id, "delayed-response-txid").await?;
    mark_operation_in_doubt(&database, &operation.operation_id, "delayed ambiguity").await?;
    mark_operation_failed(&database, &operation.operation_id, "delayed failure").await?;

    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::Cancelled
    );
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::Cancelled
    );
    let operation =
        wallet_operation_for_item(&database, WalletOperationType::GatewayFunding, &item_id)
            .await?
            .expect("operation remains");
    assert_eq!(operation.status, WalletOperationStatus::Cancelled);
    assert_eq!(operation.txid, None);
    Ok(())
}

#[tokio::test]
async fn submission_fence_winning_race_makes_cancellation_reject() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("gateway-submit-wins-race")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) = seed_gateway_allocation(&database, &setup, Sats(25_000)).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let (submit_started, submit_release) = wallet.pause_submission().await;
    let gateway = FakeGateway::new(setup.network, regtest_address());
    let worker = tokio::spawn({
        let database = database.clone();
        let setup = setup.clone();
        let wallet = wallet.clone();
        let gateway = gateway.clone();
        async move {
            process_gateway_allocations_with(
                &database,
                &setup,
                &wallet,
                &gateway,
                crate::endpoint_policy::EndpointPolicy::AllowPrivate,
            )
            .await
        }
    });

    submit_started.notified().await;
    let cancellation = cancel_allocation_with_database(
        &database,
        CancelAllocationRequest {
            federation_id,
            reason: Some("too late".to_owned()),
        },
    )
    .await?;
    assert_eq!(cancellation.status, ManualOperationStatus::Rejected);
    let operation =
        wallet_operation_for_item(&database, WalletOperationType::GatewayFunding, &item_id)
            .await?
            .expect("operation is fenced");
    assert_eq!(operation.status, WalletOperationStatus::InDoubt);

    submit_release.notify_one();
    worker.await??;
    assert_eq!(wallet.submitted_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn completed_wallet_operation_persists_gateway_completion_evidence() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("gateway-completion")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) = seed_gateway_allocation(&database, &setup, Sats(25_000)).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let gateway = FakeGateway::new(setup.network, regtest_address());

    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    let operation =
        wallet_operation_for_item(&database, WalletOperationType::GatewayFunding, &item_id)
            .await?
            .expect("wallet operation exists");
    assert_eq!(operation.status, WalletOperationStatus::Broadcast);

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
    // The gateway's Fedimint client claims the output the funding operation
    // paid. This is the only credit that completes the item.
    gateway.claim_deposit("txid-1", 0, Sats(25_000)).await;
    gateway.set_balance("federation-1", Sats(25_000)).await;

    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::Completed
    );
    assert_eq!(status.item_statuses[0].fulfilled_amount, Some(Sats(25_000)));
    let evidence = status.item_statuses[0]
        .completion_evidence
        .as_ref()
        .expect("completion evidence exists");
    match evidence {
        fedi_decentralized_service_liquidity_manager::CompletionEvidence::Gateway(evidence) => {
            assert_eq!(evidence.gateway_id, setup.gateway.gateway_id);
            assert_eq!(evidence.fulfilled_amount, Sats(25_000));
            assert_eq!(evidence.observed_gateway_balance, Sats(25_000));
            assert_eq!(evidence.withdrawal_txid.as_deref(), Some("txid-1"));
            assert_eq!(evidence.wallet_operation_id, Some(operation.operation_id));
        }
        other => panic!("expected gateway evidence, got {other:?}"),
    }

    let list = allocation_store::list_allocations(
        &database,
        allocation_store::ListAllocationsStoreRequest {
            page: fedi_decentralized_service_liquidity_manager::PageRequest {
                cursor: None,
                limit: 10,
            },
            time_range: None,
        },
    )
    .await?;
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].federation_id, federation_id);

    let admin_detail =
        allocation_store::get_admin_allocation(&database, &list.items[0].federation_id).await?;
    assert_eq!(admin_detail.allocation.wallet_operations.len(), 1);
    assert!(admin_detail.allocation.failures.is_empty());
    Ok(())
}

/// A federation balance this item did not cause — e-cash held before the
/// gateway connected, or a concurrent deposit the gateway claimed — must not
/// complete it. Only a `deposit-confirmed` claim naming this item's own
/// funding txid is attribution; every aggregate-balance increase is not.
#[tokio::test]
async fn unrelated_target_credit_does_not_complete_gateway_item() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("gateway-unrelated-credit")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) = seed_gateway_allocation(&database, &setup, Sats(25_000)).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let gateway = FakeGateway::new(setup.network, regtest_address());
    // The federation holds far more than this item commits, and the gateway
    // claims another deposit entirely — the exact pairing the aggregate
    // inequality accepted.
    gateway.set_connect_balance(Sats(40_000)).await;

    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    let operation =
        wallet_operation_for_item(&database, WalletOperationType::GatewayFunding, &item_id)
            .await?
            .expect("wallet operation exists");
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

    gateway.set_balance("federation-1", Sats(65_000)).await;
    gateway.claim_deposit("other-txid", 1, Sats(25_000)).await;
    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::Running,
        "a credit that names another txid is not evidence for this item"
    );

    // The item completes once the gateway claims its own funding output.
    gateway.claim_deposit("txid-1", 0, Sats(25_000)).await;
    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::Completed
    );
    Ok(())
}

/// One transaction can pay two items' deposit addresses in separate outputs,
/// so a shared txid is not attribution. Once chain observation has verified
/// which output this item's send paid, only a claim of that output completes
/// it.
#[tokio::test]
async fn claimed_output_index_gates_gateway_completion() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("gateway-claimed-vout")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) = seed_gateway_allocation(&database, &setup, Sats(25_000)).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let gateway = FakeGateway::new(setup.network, regtest_address());

    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    let operation =
        wallet_operation_for_item(&database, WalletOperationType::GatewayFunding, &item_id)
            .await?
            .expect("wallet operation exists");

    let funding_txid = operation.txid.clone().expect("the send was broadcast");

    // Chain evidence is the settlement writer for a funding send, and it
    // records the output index it verified.
    let claim = crate::wallet::claim_chain_evidence(
        &database,
        &operation.operation_id,
        &[crate::chain_observer::ChainOutputEvidence {
            txid: funding_txid.clone(),
            vout: 1,
            address: Some(regtest_address()),
            script_pubkey: String::new(),
            amount_sats: 25_000,
            confirmations: 6,
        }],
        1,
    )
    .await?;
    assert!(
        matches!(claim, crate::wallet::ChainEvidenceClaim::Applied(_)),
        "{claim:?}"
    );

    gateway.set_balance("federation-1", Sats(25_000)).await;
    gateway.claim_deposit(&funding_txid, 0, Sats(25_000)).await;
    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::Running,
        "another output of the same transaction is not evidence for this item"
    );

    gateway.claim_deposit(&funding_txid, 1, Sats(25_000)).await;
    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::Completed
    );
    Ok(())
}

#[tokio::test]
async fn cancelled_wallet_operation_marks_gateway_item_failed() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("gateway-cancelled-wallet")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) = seed_gateway_allocation(&database, &setup, Sats(25_000)).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let gateway = FakeGateway::new(setup.network, regtest_address());

    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    let operation =
        wallet_operation_for_item(&database, WalletOperationType::GatewayFunding, &item_id)
            .await?
            .expect("wallet operation exists");
    sqlx::query("UPDATE wallet_operations SET status = 'cancelled' WHERE operation_id = ?")
        .bind(&operation.operation_id.0)
        .execute(database.pool())
        .await?;

    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
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

/// A withdrawal refused at prepare sends nothing and fails the item cleanly.
///
/// `submit_funding_withdrawal` distinguishes three endings, and this is the
/// only one that is certain: the backend refused before anything irreversible,
/// so the item can be failed outright rather than left in doubt. The other two
/// — in doubt, and broadcast — are pinned elsewhere.
///
/// Reaching it needs `TestFundsWallet::set_prepare_failed`: rejecting the
/// address does not work, because `validate_deposit_address` refuses a bad one
/// earlier in the pass, so the backend has to refuse a well-formed address.
#[tokio::test]
async fn a_withdrawal_refused_at_prepare_sends_nothing_and_fails_the_item() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("gateway-prepare-refused")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) = seed_gateway_allocation(&database, &setup, Sats(25_000)).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let gateway = FakeGateway::new(setup.network, regtest_address());
    wallet
        .set_prepare_failed("gatewayd refused to prepare")
        .await;

    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;

    assert_eq!(
        wallet.submitted_count().await,
        0,
        "a refusal at prepare must not reach the backend"
    );

    let operation =
        wallet_operation_for_item(&database, WalletOperationType::GatewayFunding, &item_id)
            .await?
            .expect("the funding operation was created before the prepare attempt");
    assert_eq!(
        operation.status,
        WalletOperationStatus::Failed,
        "a refusal before the irreversible call is certain, so the operation \
         is failed rather than left in doubt"
    );

    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::ActionRequired
    );
    assert_eq!(
        status.item_statuses[0]
            .failure
            .as_ref()
            .map(|failure| failure.code),
        Some(LiquidityFailureCode::WithdrawFailed)
    );
    Ok(())
}

#[tokio::test]
async fn wrong_network_gateway_deposit_address_fails_before_withdrawal() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("gateway-wrong-network")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) = seed_gateway_allocation(&database, &setup, Sats(25_000)).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let gateway = FakeGateway::new(setup.network, address_for_network(Network::Bitcoin));

    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;
    assert_eq!(wallet.submitted_count().await, 0);
    assert!(
        wallet_operation_for_item(&database, WalletOperationType::GatewayFunding, &item_id)
            .await?
            .is_none()
    );

    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::ActionRequired
    );
    assert_eq!(
        status.item_statuses[0]
            .failure
            .as_ref()
            .map(|failure| failure.code),
        Some(LiquidityFailureCode::GatewayAttachFailed)
    );
    Ok(())
}

/// The production policy has to reach the gateway attach path, not only
/// the target-client join: `connect_federation` hands the requester's
/// invite to the operator's gateway process, which dials the API URLs it
/// names. Being another process's socket does not make it another
/// process's exposure.
#[tokio::test]
async fn a_non_global_invite_endpoint_is_refused_before_the_gateway_dials() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("gateway-endpoint-policy")).await?;
    let setup = test_setup_config();
    let (federation_id, item_id) = seed_gateway_allocation(&database, &setup, Sats(25_000)).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let gateway = FakeGateway::new(setup.network, regtest_address());

    // The seeded invite names a loopback guardian, which is exactly the
    // substitution the finding describes once the policy is production.
    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::GlobalOnly,
    )
    .await?;

    assert_eq!(
        gateway.connect_attempts().await,
        0,
        "the refusal must land before the gateway is asked to attach"
    );
    assert_eq!(wallet.submitted_count().await, 0);
    assert!(
        wallet_operation_for_item(&database, WalletOperationType::GatewayFunding, &item_id)
            .await?
            .is_none()
    );

    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::ActionRequired
    );
    assert_eq!(
        status.item_statuses[0]
            .failure
            .as_ref()
            .map(|failure| failure.code),
        Some(LiquidityFailureCode::GatewayAttachFailed)
    );
    Ok(())
}

/// An address-class filter cannot stop a dial to a third-party global
/// host, so what is left to deny the requester is the *result*. The reason
/// travels back through `get_allocation_status`, so it must not
/// distinguish refused from timed out from wrong certificate.
#[tokio::test]
async fn an_attach_failure_does_not_report_the_dial_outcome() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("gateway-attach-oracle")).await?;
    let setup = test_setup_config();
    let (federation_id, _item_id) =
        seed_gateway_allocation(&database, &setup, Sats(25_000)).await?;
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let gateway = FakeGateway::new(setup.network, regtest_address());
    gateway
        .set_connect_error("tcp connect to 203.0.113.7:8443: connection refused")
        .await;

    process_gateway_allocations_with(
        &database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::AllowPrivate,
    )
    .await?;

    assert_eq!(gateway.connect_attempts().await, 1);
    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation status exists");
    assert_eq!(
        status.item_statuses[0].status,
        ItemAllocationStatus::ActionRequired
    );
    let failure = status.item_statuses[0]
        .failure
        .as_ref()
        .expect("the item records a failure");
    assert_eq!(failure.code, LiquidityFailureCode::GatewayAttachFailed);
    let reason = failure.reason.clone().unwrap_or_default();
    for leak in [
        "203.0.113.7",
        "8443",
        "refused",
        "connect",
        "tcp",
        "certificate",
        "timed out",
    ] {
        assert!(
            !reason.to_lowercase().contains(leak),
            "the requester-visible reason discloses the dial outcome: {reason}"
        );
    }
    Ok(())
}

async fn seed_gateway_allocation(
    database: &Database,
    setup: &SetupConfigView,
    amount: Sats,
) -> anyhow::Result<(FederationId, ItemId)> {
    let federation_id = FederationId("federation-1".to_owned());
    let item_id = allocation_store::item_id(&federation_id, SourceType::Gateway);
    AllocationSeed {
        federation_id: federation_id.clone(),
        network: setup.network.to_string(),
        committed_amount: amount,
        reserved_amount: amount,
        items: vec![ItemSeed {
            source_type: SourceType::Gateway,
            committed_amount: amount,
            reserved_amount: amount,
            item_target: Some(AllocationItemTarget::Gateway {
                item_id: item_id.clone(),
                gateway_id: setup.gateway.gateway_id.clone(),
                gateway_name: setup.gateway.gateway_name.clone(),
                amount,
            }),
            ..ItemSeed::default()
        }],
        ..AllocationSeed::default()
    }
    .insert(database)
    .await?;
    Ok((federation_id, item_id))
}

fn test_setup_config() -> SetupConfigView {
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
            supported_sources: vec![SourceType::Gateway],
        },
        funding_policy: FundingPolicyConfig::defaults_for_network(BitcoinNetwork::Regtest),
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
    address_for_network(Network::Regtest)
}

fn address_for_network(network: Network) -> String {
    let secp = Secp256k1::new();
    let secret_key = SecretKey::from_slice(&[1_u8; 32]).expect("valid test secret key");
    let public_key = CompressedPublicKey(bitcoin::secp256k1::PublicKey::from_secret_key(
        &secp,
        &secret_key,
    ));
    Address::p2wpkh(&public_key, network).to_string()
}

#[derive(Clone, Debug)]
struct FakeGateway {
    inner: Arc<Mutex<FakeGatewayState>>,
}

#[derive(Clone, Debug)]
struct FakeGatewayState {
    network: BitcoinNetwork,
    synced_to_chain: bool,
    deposit_address: String,
    federations: BTreeMap<String, Sats>,
    connect_balance: Sats,
    connect_error: Option<String>,
    connect_attempts: usize,
    deposit_claims: Vec<GatewayDepositClaim>,
}

impl FakeGateway {
    fn new(network: BitcoinNetwork, deposit_address: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeGatewayState {
                network,
                synced_to_chain: true,
                deposit_address,
                federations: BTreeMap::new(),
                connect_balance: Sats(0),
                connect_error: None,
                connect_attempts: 0,
                deposit_claims: Vec::new(),
            })),
        }
    }

    async fn set_balance(&self, federation_id: &str, balance: Sats) {
        self.inner
            .lock()
            .await
            .federations
            .insert(federation_id.to_owned(), balance);
    }

    /// Balance the federation already holds when the gateway connects to it.
    /// `connect_federation` reports `balance_msat` for a federation the
    /// gateway may have joined or recovered earlier, so this is not
    /// necessarily zero.
    async fn set_connect_balance(&self, balance: Sats) {
        self.inner.lock().await.connect_balance = balance;
    }

    /// Makes the attach fail with the kind of detail a real dial produces.
    async fn set_connect_error(&self, error: &str) {
        self.inner.lock().await.connect_error = Some(error.to_owned());
    }

    async fn connect_attempts(&self) -> usize {
        self.inner.lock().await.connect_attempts
    }

    /// Reports a deposit the gateway's Fedimint client has claimed.
    async fn claim_deposit(&self, txid: &str, out_idx: u32, amount: Sats) {
        self.inner
            .lock()
            .await
            .deposit_claims
            .push(GatewayDepositClaim {
                txid: txid.to_owned(),
                out_idx,
                amount,
            });
    }
}

#[async_trait]
impl GatewayClient for FakeGateway {
    async fn gateway_info(&self) -> anyhow::Result<GatewaySnapshot> {
        let state = self.inner.lock().await;
        Ok(GatewaySnapshot {
            state: "running".to_owned(),
            network: state.network,
            synced_to_chain: state.synced_to_chain,
            gateway_api: GatewayApiUrl::try_from("https://gateway.example").unwrap(),
            federations: state
                .federations
                .iter()
                .map(|(federation_id, balance)| GatewayFederationSnapshot {
                    federation_id: federation_id.clone(),
                    balance: *balance,
                })
                .collect(),
        })
    }

    async fn connect_federation(
        &self,
        _invite_code: &str,
    ) -> anyhow::Result<GatewayFederationSnapshot> {
        let mut state = self.inner.lock().await;
        state.connect_attempts += 1;
        if let Some(error) = state.connect_error.clone() {
            anyhow::bail!(error);
        }
        let connect_balance = state.connect_balance;
        let balance = *state
            .federations
            .entry("federation-1".to_owned())
            .or_insert(connect_balance);
        Ok(GatewayFederationSnapshot {
            federation_id: "federation-1".to_owned(),
            balance,
        })
    }

    async fn deposit_address(
        &self,
        _federation_id: &str,
        _expected_network: BitcoinNetwork,
    ) -> anyhow::Result<String> {
        Ok(self.inner.lock().await.deposit_address.clone())
    }

    async fn recheck_deposit_address(
        &self,
        _federation_id: &str,
        _address: &str,
        _expected_network: BitcoinNetwork,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn deposit_claims(
        &self,
        _federation_id: &str,
    ) -> anyhow::Result<Vec<GatewayDepositClaim>> {
        Ok(self.inner.lock().await.deposit_claims.clone())
    }
}
