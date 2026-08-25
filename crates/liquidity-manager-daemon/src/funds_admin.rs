//! The operator-facing side of the provider wallet: balances, deposit
//! addresses, withdrawals, and the operation sync task.
//!
//! Capacity is what the wallet holds minus what accepted allocations have
//! already reserved, so every admission reads through here. See
//! [SPEC-flip-funding-safety](../specs/SPEC-flip-funding-safety.md).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use fedi_decentralized_service_liquidity_manager::{
    BitcoinNetwork, CreateDepositAddressRequest, CreateDepositAddressResponse,
    EffectiveLiquidityItem, GatewayInventoryState, GetFundsRequest, GetFundsResponse,
    InventoryStatus, ListWalletOperationsRequest, ListWalletOperationsResponse,
    ReplenishmentStatus, RequestWithdrawalRequest, RequestWithdrawalResponse, Sats, ServiceResult,
    SetupConfigView, SourceType, StabilityPoolInventoryState, WalletBalanceSummary,
    WalletOperation, WalletOperationId, WalletOperationStatus, WalletOperationType,
};

use crate::DaemonContext;
use crate::chain_observer::{ChainObserver, ChainOutputEvidence, ConfiguredChainObserver};
use crate::daemon::Worker;
use crate::database::Database;
use crate::setup_store::{self};
use crate::wallet::{
    ChainEvidenceClaim, WalletAccountingSums, WalletOperationInput, WalletOperationPageRequest,
    active_wallet_operations, active_wallet_withdrawal_amount_tx,
    bind_operator_withdrawal_intent_tx, claim_chain_evidence, get_wallet_operation,
    insert_wallet_operation_tx, list_wallet_operations, mark_operation_failed,
    mark_operation_in_doubt, mark_withdrawal_broadcast, operator_withdrawal_for_intent,
    operator_withdrawal_for_intent_tx, upsert_wallet_balance_observation, wallet_accounting_sums,
};
use crate::wallet::{
    FundsWallet, GatewaydFundsWallet, SubmitWithdrawalError, WalletBackendBalance,
};
use crate::{
    failed_precondition, internal_error, invalid_argument, run_interval_task, unavailable,
};

static WALLET_OPERATION_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(crate) async fn get_funds(
    context: &DaemonContext,
    _request: GetFundsRequest,
) -> ServiceResult<GetFundsResponse> {
    let (setup, wallet) = configured_wallet(context).await?;
    get_funds_with_wallet(&context.database, setup, wallet).await
}

pub(crate) async fn create_deposit_address(
    context: &DaemonContext,
    request: CreateDepositAddressRequest,
) -> ServiceResult<CreateDepositAddressResponse> {
    let (setup, wallet) = configured_wallet(context).await?;
    create_deposit_address_with_wallet(&context.database, &setup, wallet, request).await
}

pub(crate) async fn request_withdrawal(
    context: &DaemonContext,
    request: RequestWithdrawalRequest,
) -> ServiceResult<RequestWithdrawalResponse> {
    let (setup, wallet) = configured_wallet(context).await?;
    request_withdrawal_with_wallet(&context.database, &setup, wallet, request).await
}

pub(crate) async fn list_operations(
    context: &DaemonContext,
    request: ListWalletOperationsRequest,
) -> ServiceResult<ListWalletOperationsResponse> {
    let operations = list_wallet_operations(
        &context.database,
        WalletOperationPageRequest {
            page: request.page,
            status_filter: request.status_filter,
            time_range: request.time_range,
        },
    )
    .await?;
    Ok(ListWalletOperationsResponse { operations })
}

pub(crate) async fn sync_wallet_operations(context: &DaemonContext) -> ServiceResult<usize> {
    let (setup, wallet) = configured_wallet(context).await?;
    // This worker is serial with its own reads, so read order and write order
    // agree for it. It records the read point anyway: the comparison is against
    // read points now, and a writer that recorded a write point would release
    // rows the other writers had not yet covered.
    let read_at = crate::wallet::begin_balance_read(&context.database).await?;
    let balance = wallet.balance_summary().await.map_err(unavailable)?;
    upsert_wallet_balance_observation(&context.database, &balance, read_at).await?;
    let updates = wallet.sync_operations().await.map_err(unavailable)?;
    let mut applied = 0;
    for update in updates {
        crate::wallet::apply_sync_update(&context.database, &update).await?;
        applied += 1;
    }
    let password =
        setup_store::load_bitcoind_password(&context.database, &context.secret_store).await?;
    let observer = ConfiguredChainObserver::from_config(&setup.chain_observer, password);
    applied += sync_chain_evidence(&context.database, &setup, &observer).await?;
    Ok(applied)
}

pub(crate) async fn run_operation_sync_task(context: DaemonContext) -> anyhow::Result<()> {
    run_interval_task(
        context,
        Worker::WalletOperationSync,
        std::time::Duration::from_secs(30),
        "wallet operation sync failed",
        |context| async move { sync_wallet_operations(&context).await },
    )
    .await
}

async fn sync_chain_evidence(
    database: &Database,
    setup: &SetupConfigView,
    observer: &impl ChainObserver,
) -> ServiceResult<usize> {
    let operations = active_wallet_operations(database).await?;
    let mut applied = 0;
    for operation in operations {
        if operation.status == WalletOperationStatus::ManualReviewRequired {
            continue;
        }
        let required_confirmations = setup.funding_policy.confirmations;
        let evidence = evidence_for_operation(observer, &operation)
            .await
            .map_err(unavailable)?;
        let resolved = match evidence {
            Some(outputs) => {
                match claim_chain_evidence(
                    database,
                    &operation.operation_id,
                    &outputs,
                    required_confirmations,
                )
                .await?
                {
                    ChainEvidenceClaim::Applied(settled) => {
                        // A claim applies on every pass that moves the
                        // operation, and most of those passes only raise the
                        // confirmation count. Settling it is the event; the
                        // climb to the required depth is the heartbeat.
                        if settled.status == WalletOperationStatus::Completed {
                            tracing::info!(
                                operation_id = %settled.operation_id.0,
                                operation_type = %settled.operation_type,
                                txid = settled.txid.as_deref().unwrap_or(""),
                                amount_sats = settled.amount.0,
                                confirmations = settled.confirmation_count.unwrap_or(0),
                                "wallet operation settled on chain"
                            );
                        } else {
                            tracing::debug!(
                                operation_id = %settled.operation_id.0,
                                status = %settled.status,
                                confirmations = settled.confirmation_count.unwrap_or(0),
                                required_confirmations,
                                "claimed chain evidence for a wallet operation"
                            );
                        }
                        applied += 1;
                        true
                    }
                    ChainEvidenceClaim::NoMatch => false,
                    ChainEvidenceClaim::Ambiguous { candidate_count } => {
                        tracing::warn!(
                            operation_id = %operation.operation_id.0,
                            candidate_count,
                            "ambiguous exact chain outputs; wallet operation remains nonterminal"
                        );
                        false
                    }
                }
            }
            None => false,
        };

        // Missing and ambiguous evidence are the same situation once enough
        // time has passed: nothing is going to arrive, and `in_doubt` blocks
        // operator retry and cancellation, so something has to hand the
        // operation to a human rather than leave it stuck forever.
        if !resolved && operation.status == WalletOperationStatus::InDoubt {
            let review_after = setup.funding_policy.in_doubt_review_after_secs;
            if crate::wallet::escalate_in_doubt_to_manual_review(
                database,
                &operation.operation_id,
                review_after,
                "no chain or target-side evidence resolved this send within the \
                 configured review threshold",
            )
            .await?
            {
                tracing::warn!(
                    operation_id = %operation.operation_id.0,
                    review_after_secs = review_after,
                    "wallet send escalated to manual review"
                );
                applied += 1;
            }
        }
    }
    Ok(applied)
}

async fn evidence_for_operation(
    observer: &impl ChainObserver,
    operation: &WalletOperation,
) -> anyhow::Result<Option<Vec<ChainOutputEvidence>>> {
    if let Some(txid) = &operation.txid {
        return Ok(observer
            .tx_evidence(txid)
            .await?
            .map(|evidence| evidence.outputs));
    }

    if matches!(
        operation.status,
        WalletOperationStatus::Pending | WalletOperationStatus::InDoubt
    ) && let Some(address) = &operation.address
    {
        let evidence = observer.address_evidence(address).await?;
        return Ok(Some(evidence.outputs));
    }

    Ok(None)
}

pub(crate) async fn get_funds_with_wallet(
    database: &Database,
    setup: SetupConfigView,
    wallet: impl FundsWallet,
) -> ServiceResult<GetFundsResponse> {
    // Before the read, not after: the point of the pair is that this
    // observation records when its backend read *began*.
    let read_at = crate::wallet::begin_balance_read(database).await?;
    let balance = wallet.balance_summary().await.map_err(unavailable)?;
    if balance.network != setup.network {
        return Err(failed_precondition(format!(
            "gatewayd wallet network {} does not match configured network {}",
            balance.network, setup.network
        )));
    }
    upsert_wallet_balance_observation(database, &balance, read_at).await?;
    let accounting = wallet_accounting_sums(database).await?;
    Ok(funds_response(&setup, balance, accounting))
}

pub(crate) async fn create_deposit_address_with_wallet(
    database: &Database,
    setup: &SetupConfigView,
    wallet: impl FundsWallet,
    request: CreateDepositAddressRequest,
) -> ServiceResult<CreateDepositAddressResponse> {
    // Allocate before recording: an allocated-but-unrecorded gatewayd address
    // is harmless (nobody was told to fund it), while a recorded Pending
    // operation without an address is invisible to chain-evidence sync forever.
    let operation_id = next_wallet_operation_id("deposit");
    let address = wallet
        .allocate_deposit_address(&operation_id, request.label.as_deref())
        .await
        .map_err(unavailable)?;

    let mut tx = database.begin_write().await.map_err(internal_error)?;
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: operation_id.clone(),
            operation_type: WalletOperationType::Deposit,
            status: WalletOperationStatus::Pending,
            amount: Sats(0),
            address: Some(address.clone()),
            label: request.label.clone(),
            fee_rate_sat_per_vbyte: None,
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    tx.commit().await.map_err(internal_error)?;
    tracing::info!(
        operation_id = %operation_id.0,
        %address,
        label = request.label.as_deref().unwrap_or(""),
        "created a provider wallet deposit address"
    );

    Ok(CreateDepositAddressResponse {
        address,
        network: setup.network,
        operation_id: Some(operation_id),
    })
}

pub(crate) async fn request_withdrawal_with_wallet(
    database: &Database,
    setup: &SetupConfigView,
    wallet: impl FundsWallet,
    request: RequestWithdrawalRequest,
) -> ServiceResult<RequestWithdrawalResponse> {
    if request.withdrawal_intent_id.is_empty() {
        return Err(invalid_argument("withdrawal_intent_id must not be empty"));
    }
    if request.amount.0 == 0 {
        return Err(invalid_argument(
            "withdrawal amount must be greater than zero",
        ));
    }
    let fee_rate = request
        .fee_rate_sat_per_vbyte
        .unwrap_or_else(|| default_fee_rate_sat_per_vbyte(setup.network));
    if fee_rate == 0 {
        return Err(invalid_argument(
            "fee_rate_sat_per_vbyte must be greater than zero",
        ));
    }

    if let Some(existing) =
        operator_withdrawal_for_intent(database, &request.withdrawal_intent_id).await?
    {
        return existing_withdrawal_response(database, &request, fee_rate, existing).await;
    }

    // Before the read, for the reason `begin_balance_read` documents.
    let read_at = crate::wallet::begin_balance_read(database).await?;
    let balance = wallet.balance_summary().await.map_err(unavailable)?;
    upsert_wallet_balance_observation(database, &balance, read_at).await?;

    let operation_id = next_wallet_operation_id("withdrawal");
    let prepared = wallet
        .prepare_withdrawal(&operation_id, &request.address, request.amount, fee_rate)
        .await
        .map_err(invalid_argument)?;

    // The availability check runs inside the insert transaction so concurrent
    // withdrawals cannot both pass it against the same uncommitted balance,
    // mirroring the public request-acceptance path.
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    if let Some(existing) =
        operator_withdrawal_for_intent_tx(&mut tx, &request.withdrawal_intent_id).await?
    {
        tx.commit().await.map_err(internal_error)?;
        return existing_withdrawal_response(database, &request, fee_rate, existing).await;
    }
    let available = available_balance_for_request(&mut tx, setup, balance.spendable).await?;
    if request.amount.0 > available.0 {
        return Err(failed_precondition(
            "available wallet balance is insufficient",
        ));
    }
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: operation_id.clone(),
            operation_type: WalletOperationType::Withdrawal,
            // gatewayd has no idempotency key or reliable submission query.
            // Fence the operation durably before the irreversible call.
            status: WalletOperationStatus::InDoubt,
            amount: request.amount,
            address: Some(request.address.clone()),
            label: None,
            fee_rate_sat_per_vbyte: Some(fee_rate),
            federation_id: None,
            item_id: None,
        },
    )
    .await?;
    bind_operator_withdrawal_intent_tx(&mut tx, &operation_id, &request.withdrawal_intent_id)
        .await?;
    tx.commit().await.map_err(internal_error)?;

    tracing::info!(
        operation_id = %operation_id.0,
        withdrawal_intent_id = %request.withdrawal_intent_id,
        amount_sats = request.amount.0,
        fee_rate_sat_per_vbyte = fee_rate,
        destination = %request.address,
        "submitting an operator withdrawal to gatewayd"
    );
    let operation = match wallet.submit_prepared_withdrawal(prepared).await {
        Ok(submitted) => {
            tracing::info!(
                operation_id = %operation_id.0,
                txid = %submitted.txid,
                "operator withdrawal broadcast"
            );
            match mark_withdrawal_broadcast(database, &operation_id, &submitted.txid).await {
                Ok(operation) => operation,
                Err(error) => {
                    // The payment is gone and its txid is only in the line
                    // above. The operation stays `in_doubt`, so chain evidence
                    // can still settle it by address, but this is a reconcile.
                    tracing::error!(
                        operation_id = %operation_id.0,
                        txid = %submitted.txid,
                        %error,
                        "the operator withdrawal was broadcast but its txid could not be recorded"
                    );
                    return Err(error);
                }
            }
        }
        Err(SubmitWithdrawalError::InDoubt(detail)) => {
            tracing::warn!(
                operation_id = %operation_id.0,
                %detail,
                "operator withdrawal outcome is in doubt; the send may have happened and is \
                 never resubmitted automatically"
            );
            mark_operation_in_doubt(database, &operation_id, &detail).await?
        }
        Err(SubmitWithdrawalError::Failed(detail)) => {
            tracing::warn!(
                operation_id = %operation_id.0,
                %detail,
                "operator withdrawal was refused before submission"
            );
            mark_operation_failed(database, &operation_id, &detail).await?
        }
    };

    Ok(RequestWithdrawalResponse { operation })
}

async fn existing_withdrawal_response(
    database: &Database,
    request: &RequestWithdrawalRequest,
    fee_rate_sat_per_vbyte: u64,
    existing: crate::wallet::OperatorWithdrawalIntent,
) -> ServiceResult<RequestWithdrawalResponse> {
    if existing.address != request.address
        || existing.amount != request.amount
        || existing.fee_rate_sat_per_vbyte != fee_rate_sat_per_vbyte
    {
        tracing::warn!(
            operation_id = %existing.operation_id.0,
            withdrawal_intent_id = %request.withdrawal_intent_id,
            "an existing withdrawal intent was replayed with different terms; refusing"
        );
        return Err(failed_precondition(
            "withdrawal_intent_id is already bound to different request parameters",
        ));
    }
    Ok(RequestWithdrawalResponse {
        operation: get_wallet_operation(database, &existing.operation_id).await?,
    })
}

pub(crate) async fn available_balance_for_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    config: &SetupConfigView,
    wallet_balance: Sats,
) -> ServiceResult<Sats> {
    let in_flight_allocations = crate::wallet::active_reserved_amount_tx(tx).await?;
    let pending_outgoing = active_wallet_withdrawal_amount_tx(tx).await?;
    let fee_reserve = config.funding_policy.fee_reserve;
    Ok(available_balance(
        wallet_balance,
        pending_outgoing,
        in_flight_allocations,
        fee_reserve,
    ))
}

/// The one definition of spendable-minus-committed balance, shared by the
/// admin funds view and the public request-acceptance capacity check so the
/// two can never disagree on what is still allocatable.
fn available_balance(
    spendable: Sats,
    pending_outgoing: Sats,
    in_flight_allocations: Sats,
    fee_reserve: Sats,
) -> Sats {
    Sats(
        spendable
            .0
            .saturating_sub(pending_outgoing.0)
            .saturating_sub(in_flight_allocations.0)
            .saturating_sub(fee_reserve.0),
    )
}

fn funds_response(
    setup: &SetupConfigView,
    balance: WalletBackendBalance,
    accounting: WalletAccountingSums,
) -> GetFundsResponse {
    let summary = balance_summary(
        setup.funding_policy.fee_reserve,
        balance.spendable,
        accounting,
    );
    let gateway_supported = setup
        .capacity
        .supported_sources
        .contains(&SourceType::Gateway);
    let stability_supported = setup
        .capacity
        .supported_sources
        .contains(&SourceType::StabilityPool);
    let replenishment = replenishment_status(
        summary.available_balance,
        setup.replenishment.warning_threshold,
        setup.replenishment.critical_threshold,
    );
    let available_balance = summary.available_balance;
    let mut effective_liquidity = Vec::new();
    if gateway_supported {
        effective_liquidity.push(EffectiveLiquidityItem {
            source_type: SourceType::Gateway,
            gateway_id: Some(setup.gateway.gateway_id.clone()),
            amount: available_balance,
        });
    }
    if stability_supported {
        effective_liquidity.push(EffectiveLiquidityItem {
            source_type: SourceType::StabilityPool,
            gateway_id: None,
            amount: available_balance,
        });
    }

    GetFundsResponse {
        balance: summary,
        replenishment,
        gateway: GatewayInventoryState {
            gateway_id: setup.gateway.gateway_id.clone(),
            gateway_name: setup.gateway.gateway_name.clone(),
            status: if gateway_supported {
                InventoryStatus::Available
            } else {
                InventoryStatus::Disabled
            },
            available_amount: if gateway_supported {
                available_balance
            } else {
                Sats(0)
            },
            observed_at: Some(balance.observed_at),
        },
        stability_pool: StabilityPoolInventoryState {
            status: if stability_supported {
                InventoryStatus::Available
            } else {
                InventoryStatus::Disabled
            },
            available_amount: if stability_supported {
                available_balance
            } else {
                Sats(0)
            },
            observed_at: Some(balance.observed_at),
        },
        effective_liquidity,
    }
}

fn balance_summary(
    fee_reserve: Sats,
    spendable: Sats,
    accounting: WalletAccountingSums,
) -> WalletBalanceSummary {
    WalletBalanceSummary {
        spendable,
        pending_incoming: accounting.pending_incoming,
        pending_outgoing: accounting.pending_outgoing,
        in_flight_allocations: accounting.in_flight_allocations,
        fee_reserve,
        available_balance: available_balance(
            spendable,
            accounting.pending_outgoing,
            accounting.in_flight_allocations,
            fee_reserve,
        ),
    }
}

fn replenishment_status(
    available_balance: Sats,
    warning_threshold: Sats,
    critical_threshold: Sats,
) -> ReplenishmentStatus {
    if available_balance.0 <= critical_threshold.0 {
        ReplenishmentStatus::Critical
    } else if available_balance.0 <= warning_threshold.0 {
        ReplenishmentStatus::Warning
    } else {
        ReplenishmentStatus::Ok
    }
}

pub(crate) async fn configured_wallet(
    context: &DaemonContext,
) -> ServiceResult<(SetupConfigView, GatewaydFundsWallet)> {
    let (config, credential) =
        setup_store::ready_gateway_config(&context.database, &context.secret_store).await?;
    let wallet = GatewaydFundsWallet::new(config.clone(), credential)
        .await
        .map_err(unavailable)?;
    Ok((config, wallet))
}

pub(crate) fn default_fee_rate_sat_per_vbyte(network: BitcoinNetwork) -> u64 {
    match network {
        BitcoinNetwork::Bitcoin => 5,
        BitcoinNetwork::Testnet | BitcoinNetwork::Signet | BitcoinNetwork::Regtest => 1,
    }
}

fn next_wallet_operation_id(kind: &str) -> WalletOperationId {
    let counter = WALLET_OPERATION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    WalletOperationId(format!("wallet-{kind}-{}-{counter}", nanos))
}

#[cfg(test)]
#[path = "../tests/funds_admin.rs"]
mod tests;
