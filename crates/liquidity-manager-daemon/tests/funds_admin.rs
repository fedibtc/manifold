use bitcoin::secp256k1::{Secp256k1, SecretKey};
use bitcoin::{Address, CompressedPublicKey, Network};
use fedi_decentralized_service_liquidity_manager::{
    AdvertisementConfig, CapacityConfig, CapacityMode, ChainObserverBackendView,
    ChainObserverConfigView, DurationSecs, GatewayConfigView, GatewayId, GatewayName, PageRequest,
    ProviderPolicy, ReplenishmentConfig, RpcEndpointAddress, RpcEndpointConfig, RpcEndpointId,
    RpcProtocolName, RpcTransport, ServiceErrorCode, Url,
};

use super::*;
use crate::Database;
use crate::chain_observer::{
    AddressEvidence, ChainObserverHealth, ChainOutputEvidence, TxEvidence,
};
use crate::test_support::test_sqlite_path;
use crate::wallet::TestFundsWallet;

/// Concurrent balance reads get distinct, strictly increasing read points.
///
/// The whole read-order mechanism rests on this. `begin_balance_read` bumps
/// the tick on the pool rather than inside its caller's transaction, so two
/// reads that start at the same instant are separated by SQLite serialising
/// their `UPDATE ... RETURNING` statements and by nothing stronger. If two
/// callers could ever take the same tick, the later-arriving reply would
/// pass the `>=` monotonic guard and overwrite the fresher balance with the
/// staler one, which is the overcommit direction.
#[tokio::test]
async fn concurrent_balance_reads_take_distinct_read_points() -> anyhow::Result<()> {
    const READERS: usize = 16;
    let database = Database::connect(test_sqlite_path("concurrent-read-points")).await?;

    let mut readers = Vec::with_capacity(READERS);
    for _ in 0..READERS {
        let database = database.clone();
        readers.push(tokio::spawn(async move {
            crate::wallet::begin_balance_read(&database).await
        }));
    }

    let mut ticks = Vec::with_capacity(READERS);
    for reader in readers {
        ticks.push(reader.await??);
    }

    let mut seen: Vec<_> = ticks.clone();
    seen.sort_unstable_by_key(|point| format!("{point:?}"));
    seen.dedup_by_key(|point| format!("{point:?}"));
    assert_eq!(
        seen.len(),
        READERS,
        "two concurrent balance reads took the same read point: {ticks:?}"
    );
    Ok(())
}

/// The last durable write before an irreversible send must re-assert the
/// state it is acting on.
///
/// Each irreversible call is preceded by a compare-and-set on the state it is
/// acting on. An unchecked update immediately before the call is not that:
/// matching on `operation_id` alone, as `bind_operator_withdrawal_intent_tx`
/// could, would send on a row whose status has since moved.
///
/// **Read the fence for what it is.** On today's only caller the row is
/// inserted `in_doubt` inside this same `BEGIN IMMEDIATE` transaction, so the
/// predicate cannot fail in production. It is carried anyway, so that all three
/// irreversible call sites have the same shape and a reader can check them by
/// grep rather than by argument. This test drives the refusal directly, because
/// no production path can.
#[tokio::test]
async fn binding_a_withdrawal_intent_refuses_an_operation_that_left_in_doubt() -> anyhow::Result<()>
{
    let database = Database::connect(test_sqlite_path("withdrawal-intent-fence")).await?;

    // The control first: an `in_doubt` row binds, which is the production
    // path. Without this half, the assertions below would pass against a
    // predicate that refused everything.
    let mut tx = database.begin_write().await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId("withdrawal-fence-ok".to_owned()),
            operation_type: WalletOperationType::Withdrawal,
            status: WalletOperationStatus::InDoubt,
            amount: Sats(10_000),
            address: Some(regtest_address()),
            label: None,
            fee_rate_sat_per_vbyte: Some(1),
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    bind_operator_withdrawal_intent_tx(
        &mut tx,
        &WalletOperationId("withdrawal-fence-ok".to_owned()),
        "intent-ok",
    )
    .await?;
    tx.commit().await?;

    // The refusal: the same call against a row that is no longer `in_doubt`.
    // A row already `completed` is the state the fence exists to refuse
    // acting on.
    let mut tx = database.begin_write().await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId("withdrawal-fence-moved".to_owned()),
            operation_type: WalletOperationType::Withdrawal,
            status: WalletOperationStatus::Completed,
            amount: Sats(10_000),
            address: Some(regtest_address()),
            label: None,
            fee_rate_sat_per_vbyte: Some(1),
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    let error = bind_operator_withdrawal_intent_tx(
        &mut tx,
        &WalletOperationId("withdrawal-fence-moved".to_owned()),
        "intent-moved",
    )
    .await
    .expect_err("binding must refuse an operation that is no longer in_doubt");
    assert_eq!(error.code(), ServiceErrorCode::FailedPrecondition);
    drop(tx);

    // And an operation that does not exist at all is refused rather than
    // silently doing nothing, which is what `rows_affected() != 1` buys
    // beyond the status predicate.
    let mut tx = database.begin_write().await?;
    let error = bind_operator_withdrawal_intent_tx(
        &mut tx,
        &WalletOperationId("withdrawal-fence-absent".to_owned()),
        "intent-absent",
    )
    .await
    .expect_err("binding must refuse an operation that does not exist");
    assert_eq!(error.code(), ServiceErrorCode::FailedPrecondition);
    drop(tx);
    Ok(())
}

#[tokio::test]
async fn get_funds_excludes_pending_outgoing_and_fee_reserve() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("funds-accounting")).await?;
    let setup = test_setup_config();
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());

    let mut tx = database.begin_write().await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId("withdrawal-1".to_owned()),
            operation_type: WalletOperationType::Withdrawal,
            status: WalletOperationStatus::Pending,
            amount: Sats(10_000),
            address: Some(regtest_address()),
            label: None,
            fee_rate_sat_per_vbyte: Some(1),
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    tx.commit().await?;

    let response = get_funds_with_wallet(&database, setup, wallet).await?;
    assert_eq!(response.balance.spendable, Sats(100_000));
    assert_eq!(response.balance.pending_outgoing, Sats(10_000));
    assert_eq!(response.balance.fee_reserve, Sats(1_000));
    assert_eq!(response.balance.available_balance, Sats(89_000));
    assert_eq!(response.replenishment, ReplenishmentStatus::Ok);
    Ok(())
}

/// A settled withdrawal must keep reducing admissible capacity until a
/// balance observation exists that was read after the settlement was seen.
/// The sync pass persists the balance before it applies settlements, so
/// releasing on settlement alone hands the same sats back to admission while
/// the observed balance still predates the debit.
#[tokio::test]
async fn settled_withdrawal_holds_capacity_until_a_later_observation() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("wallet-observation-watermark")).await?;
    let setup = test_setup_config();
    let observation = |spendable| crate::wallet::WalletBackendBalance {
        network: setup.network,
        spendable,
        observed_at: crate::now_timestamp(),
    };

    // The observation that will still be current when the send settles.
    crate::wallet::observe_balance_serially(&database, &observation(Sats(100_000))).await?;

    let mut tx = database.begin_write().await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId("withdrawal-1".to_owned()),
            operation_type: WalletOperationType::Withdrawal,
            status: WalletOperationStatus::Broadcast,
            amount: Sats(40_000),
            address: Some(regtest_address()),
            label: None,
            fee_rate_sat_per_vbyte: Some(1),
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    let in_flight = available_balance_for_request(&mut tx, &setup, Sats(100_000)).await?;
    tx.commit().await?;
    assert_eq!(in_flight, Sats(59_000), "unsettled send is subtracted");

    crate::wallet::apply_sync_update(
        &database,
        &crate::wallet::WalletOperationSync {
            operation_id: WalletOperationId("withdrawal-1".to_owned()),
            status: crate::wallet::SyncedWalletStatus::Completed,
            txid: Some("txid-1".to_owned()),
            confirmation_count: Some(1),
            amount: None,
            detail: None,
        },
    )
    .await?;

    let mut tx = database.begin_write().await?;
    let after_settlement = available_balance_for_request(&mut tx, &setup, Sats(100_000)).await?;
    tx.commit().await?;
    assert_eq!(
        after_settlement,
        Sats(59_000),
        "settling does not prove the debit reached the observed balance"
    );

    // A later observation was read after the settlement was applied, so the
    // debit is now accounted for by the balance itself.
    crate::wallet::observe_balance_serially(&database, &observation(Sats(60_000))).await?;
    let mut tx = database.begin_write().await?;
    let after_observation = available_balance_for_request(&mut tx, &setup, Sats(60_000)).await?;
    tx.commit().await?;
    assert_eq!(
        after_observation,
        Sats(59_000),
        "the send is released once a later observation exists, not counted twice"
    );
    Ok(())
}

/// The same rule, for an allocation funding send.
///
/// Funding sends debit the same wallet as an operator withdrawal, but they
/// were excluded from this subtraction entirely: the query filtered on the
/// withdrawal type, and the production settlement writer for a funding send
/// is chain evidence, which recorded no watermark. While the item was
/// active its reservation covered the send, so the gap opened at exactly
/// the moment the item went terminal — the reservation vanished, the debit
/// was not yet in any observed balance, and nothing was left subtracting
/// it. A second request then reused sats the first had already spent.
#[tokio::test]
async fn a_settled_funding_send_holds_capacity_until_a_later_observation() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("funding-send-watermark")).await?;
    let setup = test_setup_config();
    let observation = |spendable| crate::wallet::WalletBackendBalance {
        network: setup.network,
        spendable,
        observed_at: crate::now_timestamp(),
    };
    let available = |balance| {
        let database = database.clone();
        let setup = setup.clone();
        async move {
            let mut tx = database.begin_write().await?;
            let available = available_balance_for_request(&mut tx, &setup, balance).await?;
            tx.commit().await?;
            anyhow::Ok(available)
        }
    };

    // The observation that is still current when the send settles: read
    // before the debit, persisted after it.
    crate::wallet::observe_balance_serially(&database, &observation(Sats(100_000))).await?;

    // An accepted allocation with one active gateway item reserving 40k,
    // and the funding send that item made.
    sqlx::query(
        "INSERT INTO allocations (federation_id, requester_pubkey, provider_pubkey, network, \
         details_payload_hash, request_json, verification_json, target_json, \
         committed_amount_sats, reserved_amount_sats) \
         VALUES ('fed-1', 'requester', 'provider', 'regtest', X'00', '{}', '{}', '{}', \
         40000, 40000)",
    )
    .execute(database.pool())
    .await?;
    sqlx::query(
        "INSERT INTO allocation_items \
         (item_id, federation_id, source_type, status, committed_amount_sats, \
          reserved_amount_sats) \
         VALUES ('fed-1:gateway', 'fed-1', 'gateway', 'pending', 40000, 40000)",
    )
    .execute(database.pool())
    .await?;

    let mut tx = database.begin_write().await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId("gateway-funding-1".to_owned()),
            operation_type: WalletOperationType::GatewayFunding,
            status: WalletOperationStatus::Broadcast,
            amount: Sats(40_000),
            address: Some(regtest_address()),
            label: None,
            fee_rate_sat_per_vbyte: Some(1),
            federation_id: Some(fedi_decentralized_service_liquidity_manager::FederationId(
                "fed-1".to_owned(),
            )),
            item_id: Some(fedi_decentralized_service_liquidity_manager::ItemId(
                "fed-1:gateway".to_owned(),
            )),
        },
    )
    .await?;
    tx.commit().await?;

    assert_eq!(
        available(Sats(100_000)).await?,
        Sats(59_000),
        "an active item's reservation covers its own send exactly once"
    );

    // Chain evidence settles the send. This is the production settlement
    // writer for a funding operation, and the balance on record is still
    // the pre-debit one.
    let claim = crate::wallet::claim_chain_evidence(
        &database,
        &WalletOperationId("gateway-funding-1".to_owned()),
        &[crate::chain_observer::ChainOutputEvidence {
            txid: "funding-txid".to_owned(),
            vout: 0,
            address: Some(regtest_address()),
            script_pubkey: String::new(),
            amount_sats: 40_000,
            confirmations: 6,
        }],
        1,
    )
    .await?;
    assert!(
        matches!(claim, crate::wallet::ChainEvidenceClaim::Applied(_)),
        "{claim:?}"
    );

    // The item completes, so its reservation is released. This is the moment
    // the gap would open.
    sqlx::query("UPDATE allocation_items SET status = 'completed' WHERE item_id = ?")
        .bind("fed-1:gateway")
        .execute(database.pool())
        .await?;

    assert_eq!(
        available(Sats(100_000)).await?,
        Sats(59_000),
        "a settled funding send stays subtracted until an observation is \
         known to include its debit"
    );

    // An observation read after the settlement accounts for the debit
    // itself, so the send is released rather than counted twice.
    //
    // Two observations, not one, because the assertion above took the
    // release watermark: an item-linked send is charged from the count
    // current when its exclusion lifted, plus one. The `plus one` is there
    // because that stamp is written in the capacity transaction, which runs
    // concurrently with the observation task rather than inside it, so the
    // first observation may have read the backend before the stamp. The
    // send is still released exactly once and never counted twice; what
    // changed is that it is released against an observation that provably
    // read after it became chargeable.
    // One observation is enough, and that is the point of read-order ticks.
    //
    // The assertion above took the release stamp at the tick then current.
    // `observe_balance_serially` takes a *later* tick before it reads, so
    // this observation provably began after that stamp and can account for
    // the debit. Under the previous write-order counter this needed two
    // observations, because a stamp and a read falling between the same
    // pair of writes were indistinguishable and the code compensated with a
    // `+ 1` that over-charged by a whole cycle.
    crate::wallet::observe_balance_serially(&database, &observation(Sats(60_000))).await?;
    assert_eq!(
        available(Sats(60_000)).await?,
        Sats(59_000),
        "released once, not counted twice"
    );
    Ok(())
}

/// Money arriving must not be charged as money leaving.
///
/// Deposits share the `wallet_operations` table with sends, and a pending
/// deposit sits in the same statuses. Broadening the capacity subtraction
/// from withdrawals to "every operation" therefore also caught deposits and
/// silently halved a replenishing provider's admissible capacity. The live
/// suite found it; no unit test did, which is why this one exists.
#[tokio::test]
async fn a_pending_deposit_does_not_reduce_capacity() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("pending-deposit-capacity")).await?;
    let setup = test_setup_config();
    crate::wallet::observe_balance_serially(
        &database,
        &crate::wallet::WalletBackendBalance {
            network: setup.network,
            spendable: Sats(100_000),
            observed_at: crate::now_timestamp(),
        },
    )
    .await?;

    let mut tx = database.begin_write().await?;
    let before = available_balance_for_request(&mut tx, &setup, Sats(100_000)).await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId("deposit-1".to_owned()),
            operation_type: WalletOperationType::Deposit,
            status: WalletOperationStatus::Pending,
            amount: Sats(40_000),
            address: Some(regtest_address()),
            label: None,
            fee_rate_sat_per_vbyte: None,
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    let after = available_balance_for_request(&mut tx, &setup, Sats(100_000)).await?;
    tx.commit().await?;

    assert_eq!(
        before, after,
        "an incoming deposit must not be subtracted from available capacity"
    );
    Ok(())
}

#[tokio::test]
async fn create_deposit_address_persists_operation() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("deposit-address")).await?;
    let setup = test_setup_config();
    let address = regtest_address();
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), address.clone());

    let response = create_deposit_address_with_wallet(
        &database,
        &setup,
        wallet,
        CreateDepositAddressRequest {
            label: Some("top-up".to_owned()),
        },
    )
    .await?;
    assert_eq!(response.address, address);
    assert_eq!(response.network, BitcoinNetwork::Regtest);

    let listed = list_wallet_operations(
        &database,
        WalletOperationPageRequest {
            page: PageRequest {
                cursor: None,
                limit: 10,
            },
            status_filter: Some(WalletOperationStatus::Pending),
            time_range: None,
        },
    )
    .await?;
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].operation_type, WalletOperationType::Deposit);
    Ok(())
}

#[tokio::test]
async fn lost_withdrawal_response_moves_to_in_doubt_once() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("withdrawal-in-doubt")).await?;
    let setup = test_setup_config();
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    wallet.set_submit_in_doubt("lost gatewayd response").await;

    let response = request_withdrawal_with_wallet(
        &database,
        &setup,
        wallet.clone(),
        RequestWithdrawalRequest {
            withdrawal_intent_id: "withdrawal-in-doubt".to_owned(),
            address: regtest_address(),
            amount: Sats(25_000),
            fee_rate_sat_per_vbyte: Some(1),
        },
    )
    .await?;

    assert_eq!(response.operation.status, WalletOperationStatus::InDoubt);
    assert_eq!(response.operation.amount, Sats(25_000));
    assert_eq!(wallet.submitted_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn withdrawal_replay_returns_existing_operation_without_resubmission() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("withdrawal-replay")).await?;
    let setup = test_setup_config();
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let request = withdrawal_request("same-intent");

    let first =
        request_withdrawal_with_wallet(&database, &setup, wallet.clone(), request.clone()).await?;
    let replay = request_withdrawal_with_wallet(&database, &setup, wallet.clone(), request).await?;

    assert_eq!(replay.operation, first.operation);
    assert_eq!(wallet.submitted_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn concurrent_withdrawal_replay_submits_once() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("withdrawal-concurrent-replay")).await?;
    let setup = test_setup_config();
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let request = withdrawal_request("concurrent-intent");

    let (first, second) = tokio::join!(
        request_withdrawal_with_wallet(&database, &setup, wallet.clone(), request.clone()),
        request_withdrawal_with_wallet(&database, &setup, wallet.clone(), request)
    );
    let first = first?;
    let second = second?;

    assert_eq!(first.operation.operation_id, second.operation.operation_id);
    assert_eq!(wallet.submitted_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn withdrawal_intent_rejects_parameter_conflict() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("withdrawal-intent-conflict")).await?;
    let setup = test_setup_config();
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let request = withdrawal_request("conflicting-intent");
    request_withdrawal_with_wallet(&database, &setup, wallet.clone(), request.clone()).await?;

    let error = request_withdrawal_with_wallet(
        &database,
        &setup,
        wallet.clone(),
        RequestWithdrawalRequest {
            amount: Sats(request.amount.0 + 1),
            ..request
        },
    )
    .await
    .expect_err("changed parameters must conflict");

    assert_eq!(error.code(), ServiceErrorCode::FailedPrecondition);
    assert_eq!(wallet.submitted_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn distinct_intents_allow_identical_withdrawals() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("withdrawal-distinct-intents")).await?;
    let setup = test_setup_config();
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());

    let first = request_withdrawal_with_wallet(
        &database,
        &setup,
        wallet.clone(),
        withdrawal_request("intent-one"),
    )
    .await?;
    let second = request_withdrawal_with_wallet(
        &database,
        &setup,
        wallet.clone(),
        withdrawal_request("intent-two"),
    )
    .await?;

    assert_ne!(first.operation.operation_id, second.operation.operation_id);
    assert_eq!(wallet.submitted_count().await, 2);
    Ok(())
}

#[tokio::test]
async fn withdrawal_is_durably_fenced_before_send_and_not_resubmitted_after_restart()
-> anyhow::Result<()> {
    let path = test_sqlite_path("withdrawal-pre-send-fence");
    let database = Database::connect(&path).await?;
    let setup = test_setup_config();
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let (submit_started, _submit_release) = wallet.pause_submission().await;
    let request = withdrawal_request("restart-safe-intent");
    let task = tokio::spawn({
        let database = database.clone();
        let setup = setup.clone();
        let wallet = wallet.clone();
        let request = request.clone();
        async move { request_withdrawal_with_wallet(&database, &setup, wallet, request).await }
    });

    submit_started.notified().await;
    let stored = operator_withdrawal_for_intent(&database, &request.withdrawal_intent_id)
        .await?
        .expect("intent is durable before submission begins");
    let operation = get_wallet_operation(&database, &stored.operation_id).await?;
    assert_eq!(operation.status, WalletOperationStatus::InDoubt);

    task.abort();
    let _ = task.await;
    drop(database);

    let reopened = Database::connect(path).await?;
    let retry_wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    let replay =
        request_withdrawal_with_wallet(&reopened, &setup, retry_wallet.clone(), request).await?;
    assert_eq!(replay.operation.operation_id, stored.operation_id);
    assert_eq!(replay.operation.status, WalletOperationStatus::InDoubt);
    assert_eq!(retry_wallet.submitted_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn withdrawal_exceeding_available_balance_is_rejected() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("withdrawal-insufficient")).await?;
    let setup = test_setup_config();
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());

    let mut tx = database.begin_write().await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId("withdrawal-1".to_owned()),
            operation_type: WalletOperationType::Withdrawal,
            status: WalletOperationStatus::Pending,
            amount: Sats(10_000),
            address: Some(regtest_address()),
            label: None,
            fee_rate_sat_per_vbyte: Some(1),
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    tx.commit().await?;

    // Spendable 100_000 minus pending 10_000 and fee reserve 1_000
    // leaves 89_000 available.
    let result = request_withdrawal_with_wallet(
        &database,
        &setup,
        wallet.clone(),
        RequestWithdrawalRequest {
            withdrawal_intent_id: "withdrawal-insufficient".to_owned(),
            address: regtest_address(),
            amount: Sats(90_000),
            fee_rate_sat_per_vbyte: Some(1),
        },
    )
    .await;

    assert!(result.is_err());
    assert_eq!(wallet.submitted_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn clean_submit_failure_marks_operation_failed() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("withdrawal-failed")).await?;
    let setup = test_setup_config();
    let wallet = TestFundsWallet::new(setup.network, Sats(100_000), regtest_address());
    wallet.set_submit_failed("invalid destination").await;

    let response = request_withdrawal_with_wallet(
        &database,
        &setup,
        wallet.clone(),
        RequestWithdrawalRequest {
            withdrawal_intent_id: "withdrawal-failed".to_owned(),
            address: regtest_address(),
            amount: Sats(25_000),
            fee_rate_sat_per_vbyte: Some(1),
        },
    )
    .await?;

    assert_eq!(response.operation.status, WalletOperationStatus::Failed);
    let failure = response.operation.failure.expect("failure detail recorded");
    assert_eq!(failure.message, "invalid destination");
    Ok(())
}

#[tokio::test]
async fn sync_chain_evidence_completes_pending_deposit() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("deposit-sync")).await?;
    let setup = test_setup_config();
    let address = regtest_address();
    let mut tx = database.begin_write().await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId("deposit-1".to_owned()),
            operation_type: WalletOperationType::Deposit,
            status: WalletOperationStatus::Pending,
            amount: Sats(0),
            address: Some(address.clone()),
            label: None,
            fee_rate_sat_per_vbyte: None,
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    tx.commit().await?;

    let observer = StaticChainObserver {
        outputs: vec![test_output("txid-1", 0, &address, 25_000, 1)],
    };
    let applied = sync_chain_evidence(&database, &setup, &observer).await?;
    let operation =
        crate::wallet::get_wallet_operation(&database, &WalletOperationId("deposit-1".to_owned()))
            .await?;

    assert_eq!(applied, 1);
    assert_eq!(operation.status, WalletOperationStatus::Completed);
    assert_eq!(operation.txid.as_deref(), Some("txid-1"));
    assert_eq!(operation.tx_vout, Some(0));
    assert_eq!(operation.amount, Sats(25_000));
    Ok(())
}

#[tokio::test]
async fn chain_evidence_requires_the_exact_expected_amount() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("wrong-chain-amount")).await?;
    let setup = test_setup_config();
    let address = regtest_address();
    insert_test_operation(
        &database,
        "withdrawal-wrong-amount",
        WalletOperationType::Withdrawal,
        WalletOperationStatus::InDoubt,
        Sats(25_000),
        &address,
    )
    .await?;
    let observer = StaticChainObserver {
        outputs: vec![
            test_output("dust", 0, &address, 1, 1),
            test_output("wrong", 1, &address, 24_999, 1),
        ],
    };

    assert_eq!(sync_chain_evidence(&database, &setup, &observer).await?, 0);
    let operation = get_wallet_operation(
        &database,
        &WalletOperationId("withdrawal-wrong-amount".to_owned()),
    )
    .await?;
    assert_eq!(operation.status, WalletOperationStatus::InDoubt);
    assert_eq!(operation.txid, None);
    assert_eq!(operation.tx_vout, None);
    Ok(())
}

#[tokio::test]
async fn multiple_exact_chain_outputs_remain_nonterminal() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("ambiguous-chain-output")).await?;
    let setup = test_setup_config();
    let address = regtest_address();
    insert_test_operation(
        &database,
        "ambiguous-withdrawal",
        WalletOperationType::Withdrawal,
        WalletOperationStatus::InDoubt,
        Sats(25_000),
        &address,
    )
    .await?;
    let observer = StaticChainObserver {
        outputs: vec![
            test_output("candidate-a", 0, &address, 25_000, 1),
            test_output("candidate-b", 2, &address, 25_000, 1),
        ],
    };

    assert_eq!(sync_chain_evidence(&database, &setup, &observer).await?, 0);
    let operation = get_wallet_operation(
        &database,
        &WalletOperationId("ambiguous-withdrawal".to_owned()),
    )
    .await?;
    assert_eq!(operation.status, WalletOperationStatus::InDoubt);
    assert_eq!(operation.txid, None);
    assert_eq!(operation.tx_vout, None);
    Ok(())
}

/// An `in_doubt` send whose evidence never arrives is escalated once the
/// operator's threshold passes. Without this it stays `in_doubt` forever,
/// and `in_doubt` rejects both guarded retry and cancellation, so the
/// allocation has no route to any terminal state.
#[tokio::test]
async fn unresolved_in_doubt_escalates_to_manual_review_past_the_threshold() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("escalate-in-doubt")).await?;
    let mut setup = test_setup_config();
    setup.funding_policy.in_doubt_review_after_secs = 3_600;
    let address = regtest_address();
    insert_test_operation(
        &database,
        "stuck-withdrawal",
        WalletOperationType::Withdrawal,
        WalletOperationStatus::InDoubt,
        Sats(25_000),
        &address,
    )
    .await?;
    backdate_submission(&database, "stuck-withdrawal", 7_200).await?;
    let observer = StaticChainObserver { outputs: vec![] };

    assert_eq!(sync_chain_evidence(&database, &setup, &observer).await?, 1);

    let operation =
        get_wallet_operation(&database, &WalletOperationId("stuck-withdrawal".to_owned())).await?;
    assert_eq!(
        operation.status,
        WalletOperationStatus::ManualReviewRequired
    );
    assert_eq!(
        operation
            .failure
            .as_ref()
            .map(|failure| failure.code.clone()),
        Some("manual_review_required".to_owned())
    );
    Ok(())
}

/// Ambiguity is escalated on the same threshold as absence. Both mean FLIP
/// cannot say what happened, and waiting longer does not change that.
#[tokio::test]
async fn ambiguous_evidence_escalates_on_the_same_threshold() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("escalate-ambiguous")).await?;
    let mut setup = test_setup_config();
    setup.funding_policy.in_doubt_review_after_secs = 3_600;
    let address = regtest_address();
    insert_test_operation(
        &database,
        "ambiguous-stuck",
        WalletOperationType::Withdrawal,
        WalletOperationStatus::InDoubt,
        Sats(25_000),
        &address,
    )
    .await?;
    backdate_submission(&database, "ambiguous-stuck", 7_200).await?;
    let observer = StaticChainObserver {
        outputs: vec![
            test_output("candidate-a", 0, &address, 25_000, 1),
            test_output("candidate-b", 2, &address, 25_000, 1),
        ],
    };

    assert_eq!(sync_chain_evidence(&database, &setup, &observer).await?, 1);

    let operation =
        get_wallet_operation(&database, &WalletOperationId("ambiguous-stuck".to_owned())).await?;
    assert_eq!(
        operation.status,
        WalletOperationStatus::ManualReviewRequired
    );
    assert_eq!(operation.txid, None, "no output was claimed");
    Ok(())
}

/// Before the threshold, an unresolved send is left alone. Escalation
/// blocks automatic settlement, so escalating early would strand sends that
/// were about to resolve on their own.
#[tokio::test]
async fn in_doubt_within_the_threshold_is_left_alone() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("threshold-not-reached")).await?;
    let mut setup = test_setup_config();
    setup.funding_policy.in_doubt_review_after_secs = 3_600;
    let address = regtest_address();
    insert_test_operation(
        &database,
        "recent-withdrawal",
        WalletOperationType::Withdrawal,
        WalletOperationStatus::InDoubt,
        Sats(25_000),
        &address,
    )
    .await?;
    backdate_submission(&database, "recent-withdrawal", 60).await?;
    let observer = StaticChainObserver { outputs: vec![] };

    assert_eq!(sync_chain_evidence(&database, &setup, &observer).await?, 0);

    let operation = get_wallet_operation(
        &database,
        &WalletOperationId("recent-withdrawal".to_owned()),
    )
    .await?;
    assert_eq!(operation.status, WalletOperationStatus::InDoubt);
    Ok(())
}

/// Evidence that settles the send wins even when the operation is old
/// enough to escalate. The threshold decides when to give up looking, not
/// what to do with what was found.
#[tokio::test]
async fn settling_evidence_wins_over_an_expired_threshold() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("evidence-beats-threshold")).await?;
    let mut setup = test_setup_config();
    setup.funding_policy.in_doubt_review_after_secs = 1;
    let address = regtest_address();
    insert_test_operation(
        &database,
        "settled-withdrawal",
        WalletOperationType::Withdrawal,
        WalletOperationStatus::InDoubt,
        Sats(25_000),
        &address,
    )
    .await?;
    backdate_submission(&database, "settled-withdrawal", 7_200).await?;
    let observer = StaticChainObserver {
        outputs: vec![test_output("settling-tx", 1, &address, 25_000, 1)],
    };

    assert_eq!(sync_chain_evidence(&database, &setup, &observer).await?, 1);

    let operation = get_wallet_operation(
        &database,
        &WalletOperationId("settled-withdrawal".to_owned()),
    )
    .await?;
    assert_eq!(operation.status, WalletOperationStatus::Completed);
    assert_eq!(operation.txid.as_deref(), Some("settling-tx"));
    Ok(())
}

#[tokio::test]
async fn known_txid_still_requires_its_exact_output() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("known-tx-output")).await?;
    let setup = test_setup_config();
    let address = regtest_address();
    let operation_id = WalletOperationId("known-tx-withdrawal".to_owned());
    insert_test_operation(
        &database,
        &operation_id.0,
        WalletOperationType::Withdrawal,
        WalletOperationStatus::Pending,
        Sats(25_000),
        &address,
    )
    .await?;
    mark_withdrawal_broadcast(&database, &operation_id, "known-tx").await?;

    let wrong = StaticChainObserver {
        outputs: vec![test_output("known-tx", 0, &address, 1, 6)],
    };
    assert_eq!(sync_chain_evidence(&database, &setup, &wrong).await?, 0);
    let operation = get_wallet_operation(&database, &operation_id).await?;
    assert_eq!(operation.status, WalletOperationStatus::Broadcast);
    assert_eq!(operation.tx_vout, None);

    let exact = StaticChainObserver {
        outputs: vec![test_output("known-tx", 3, &address, 25_000, 0)],
    };
    assert_eq!(sync_chain_evidence(&database, &setup, &exact).await?, 1);
    let operation = get_wallet_operation(&database, &operation_id).await?;
    assert_eq!(operation.status, WalletOperationStatus::Broadcast);
    assert_eq!(operation.txid.as_deref(), Some("known-tx"));
    assert_eq!(operation.tx_vout, Some(3));

    mark_withdrawal_broadcast(&database, &operation_id, "delayed-other-tx").await?;
    let operation = get_wallet_operation(&database, &operation_id).await?;
    assert_eq!(operation.txid.as_deref(), Some("known-tx"));
    assert_eq!(operation.tx_vout, Some(3));

    let confirmed = StaticChainObserver {
        outputs: vec![test_output("known-tx", 3, &address, 25_000, 6)],
    };
    assert_eq!(sync_chain_evidence(&database, &setup, &confirmed).await?, 1);
    let operation = get_wallet_operation(&database, &operation_id).await?;
    assert_eq!(operation.status, WalletOperationStatus::Completed);
    assert_eq!(operation.txid.as_deref(), Some("known-tx"));
    assert_eq!(operation.tx_vout, Some(3));
    Ok(())
}

#[tokio::test]
async fn one_outpoint_cannot_settle_two_operations_concurrently() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("exclusive-chain-output")).await?;
    let setup = test_setup_config();
    let address = regtest_address();
    for operation_id in ["competing-a", "competing-b"] {
        insert_test_operation(
            &database,
            operation_id,
            WalletOperationType::Withdrawal,
            WalletOperationStatus::InDoubt,
            Sats(25_000),
            &address,
        )
        .await?;
    }
    let observer = StaticChainObserver {
        outputs: vec![test_output("single-output", 4, &address, 25_000, 1)],
    };

    let (first, second) = tokio::join!(
        sync_chain_evidence(&database, &setup, &observer),
        sync_chain_evidence(&database, &setup, &observer),
    );
    first?;
    second?;
    let operations = active_wallet_operations(&database).await?;
    assert_eq!(operations.len(), 1);
    let settled = [
        get_wallet_operation(&database, &WalletOperationId("competing-a".to_owned())).await?,
        get_wallet_operation(&database, &WalletOperationId("competing-b".to_owned())).await?,
    ];
    assert_eq!(
        settled
            .iter()
            .filter(|operation| operation.status == WalletOperationStatus::Completed)
            .count(),
        1
    );
    assert_eq!(
        settled
            .iter()
            .filter(|operation| operation.tx_vout == Some(4))
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn distinct_outputs_settle_distinct_operations() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("distinct-chain-outputs")).await?;
    let setup = test_setup_config();
    let address_a = regtest_address();
    let address_b = {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[2_u8; 32])?;
        let public_key = CompressedPublicKey(bitcoin::secp256k1::PublicKey::from_secret_key(
            &secp,
            &secret_key,
        ));
        Address::p2wpkh(&public_key, Network::Regtest).to_string()
    };
    insert_test_operation(
        &database,
        "distinct-a",
        WalletOperationType::Withdrawal,
        WalletOperationStatus::InDoubt,
        Sats(25_000),
        &address_a,
    )
    .await?;
    insert_test_operation(
        &database,
        "distinct-b",
        WalletOperationType::Withdrawal,
        WalletOperationStatus::InDoubt,
        Sats(25_000),
        &address_b,
    )
    .await?;
    let observer = StaticChainObserver {
        outputs: vec![
            test_output("shared-tx", 0, &address_a, 25_000, 1),
            test_output("shared-tx", 1, &address_b, 25_000, 1),
        ],
    };

    assert_eq!(sync_chain_evidence(&database, &setup, &observer).await?, 2);
    assert_eq!(
        get_wallet_operation(&database, &WalletOperationId("distinct-a".to_owned()))
            .await?
            .tx_vout,
        Some(0)
    );
    assert_eq!(
        get_wallet_operation(&database, &WalletOperationId("distinct-b".to_owned()))
            .await?
            .tx_vout,
        Some(1)
    );
    Ok(())
}

#[tokio::test]
async fn admin_list_wallet_operations_uses_status_filter() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("wallet-list")).await?;
    let mut tx = database.begin_write().await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId("deposit-1".to_owned()),
            operation_type: WalletOperationType::Deposit,
            status: WalletOperationStatus::Pending,
            amount: Sats(0),
            address: Some(regtest_address()),
            label: None,
            fee_rate_sat_per_vbyte: None,
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId("withdrawal-1".to_owned()),
            operation_type: WalletOperationType::Withdrawal,
            status: WalletOperationStatus::Broadcast,
            amount: Sats(1_000),
            address: Some(regtest_address()),
            label: None,
            fee_rate_sat_per_vbyte: Some(1),
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    tx.commit().await?;

    let response = list_wallet_operations(
        &database,
        WalletOperationPageRequest {
            page: PageRequest {
                cursor: None,
                limit: 10,
            },
            status_filter: Some(WalletOperationStatus::Broadcast),
            time_range: None,
        },
    )
    .await?;
    assert_eq!(response.items.len(), 1);
    assert_eq!(response.items[0].operation_id.0, "withdrawal-1");
    Ok(())
}

/// The detail read the resolution screen needs, and the list shape's gap.
///
/// `list_wallet_operations` returns a summary: id, type, amount, status,
/// federation and two timestamps. An operator resolving a send held for
/// manual review has to see where it was going and what chain evidence
/// exists, and neither is in that shape — so the whole operation is read
/// through its own verb.
#[tokio::test]
async fn a_single_wallet_operation_reads_back_the_detail_the_list_omits() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("wallet-detail")).await?;
    let address = regtest_address();
    let mut tx = database.begin_write().await?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId("withdrawal-detail".to_owned()),
            operation_type: WalletOperationType::Withdrawal,
            status: WalletOperationStatus::Pending,
            amount: Sats(250_000),
            address: Some(address.clone()),
            label: None,
            fee_rate_sat_per_vbyte: Some(1),
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    tx.commit().await?;

    let listed = list_wallet_operations(
        &database,
        WalletOperationPageRequest {
            page: PageRequest {
                cursor: None,
                limit: 10,
            },
            status_filter: None,
            time_range: None,
        },
    )
    .await?;
    assert_eq!(listed.items.len(), 1);

    let operation = crate::wallet::get_wallet_operation(
        &database,
        &WalletOperationId("withdrawal-detail".to_owned()),
    )
    .await?;
    assert_eq!(operation.amount, Sats(250_000));
    assert_eq!(operation.address.as_deref(), Some(address.as_str()));
    assert_eq!(operation.txid, None);
    assert_eq!(operation.confirmation_count, None);

    // An id that is not there is a not-found, not an empty operation: the
    // screen must not render a blank resolution form for nothing.
    let missing = crate::wallet::get_wallet_operation(
        &database,
        &WalletOperationId("no-such-operation".to_owned()),
    )
    .await;
    assert_eq!(
        missing.expect_err("absent operation is an error").code(),
        ServiceErrorCode::NotFound
    );
    Ok(())
}

fn test_setup_config() -> SetupConfigView {
    let mut funding_policy =
        fedi_decentralized_service_liquidity_manager::FundingPolicyConfig::defaults_for_network(
            BitcoinNetwork::Regtest,
        );
    funding_policy.fee_reserve = Sats(1_000);

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
        attestation_summary: Default::default(),
    }
}

fn regtest_address() -> String {
    let secp = Secp256k1::new();
    let secret_key = SecretKey::from_slice(&[1_u8; 32]).expect("valid test secret key");
    let public_key = CompressedPublicKey(bitcoin::secp256k1::PublicKey::from_secret_key(
        &secp,
        &secret_key,
    ));
    Address::p2wpkh(&public_key, Network::Regtest).to_string()
}

fn withdrawal_request(withdrawal_intent_id: &str) -> RequestWithdrawalRequest {
    RequestWithdrawalRequest {
        withdrawal_intent_id: withdrawal_intent_id.to_owned(),
        address: regtest_address(),
        amount: Sats(25_000),
        fee_rate_sat_per_vbyte: Some(1),
    }
}

/// Ages an operation's submission so a review threshold can be crossed
/// without the test sleeping.
async fn backdate_submission(
    database: &Database,
    operation_id: &str,
    seconds: u64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE wallet_operations SET submitted_at = unixepoch() - ? WHERE operation_id = ?",
    )
    .bind(i64::try_from(seconds)?)
    .bind(operation_id)
    .execute(database.pool())
    .await?;
    Ok(())
}

async fn insert_test_operation(
    database: &Database,
    operation_id: &str,
    operation_type: WalletOperationType,
    status: WalletOperationStatus,
    amount: Sats,
    address: &str,
) -> ServiceResult<()> {
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: WalletOperationId(operation_id.to_owned()),
            operation_type,
            status,
            amount,
            address: Some(address.to_owned()),
            label: None,
            fee_rate_sat_per_vbyte: None,
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    tx.commit().await.map_err(internal_error)
}

struct StaticChainObserver {
    outputs: Vec<ChainOutputEvidence>,
}

#[async_trait::async_trait]
impl ChainObserver for StaticChainObserver {
    async fn health(&self) -> anyhow::Result<ChainObserverHealth> {
        Ok(ChainObserverHealth {
            reachable: true,
            detail: None,
        })
    }

    async fn tx_evidence(&self, txid: &str) -> anyhow::Result<Option<TxEvidence>> {
        let outputs = self
            .outputs
            .iter()
            .filter(|output| output.txid == txid)
            .cloned()
            .collect::<Vec<_>>();
        Ok((!outputs.is_empty()).then(|| TxEvidence {
            txid: txid.to_owned(),
            confirmations: outputs
                .iter()
                .map(|output| output.confirmations)
                .max()
                .unwrap_or_default(),
            outputs,
        }))
    }

    async fn address_evidence(&self, address: &str) -> anyhow::Result<AddressEvidence> {
        Ok(AddressEvidence {
            address: address.to_owned(),
            outputs: self
                .outputs
                .iter()
                .filter(|output| output.address.as_deref() == Some(address))
                .cloned()
                .collect(),
        })
    }
}

fn test_output(
    txid: &str,
    vout: u32,
    address: &str,
    amount_sats: u64,
    confirmations: u32,
) -> ChainOutputEvidence {
    ChainOutputEvidence {
        txid: txid.to_owned(),
        vout,
        address: Some(address.to_owned()),
        script_pubkey: "0014test".to_owned(),
        amount_sats,
        confirmations,
    }
}
