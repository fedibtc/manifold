//! The worker that funds gateway allocations.
//!
//! A gateway item advances in four steps — connect the gateway to the
//! federation, ask it for a deposit address, withdraw provider funds to that
//! address, observe the balance — each recorded durably, so a restart resumes
//! rather than repeats.
//!
//! The gatewayd boundary this drives is [`crate::gateway`]; this module is its
//! only worker-side consumer. The pairing mirrors
//! [`crate::stability_pool`]/[`crate::stability_allocation`].

use fedi_decentralized_service_liquidity_manager::{
    CompletionEvidence, GatewayCompletionEvidence, LiquidityFailureCode, Sats, ServiceResult,
    SetupConfigView, WalletOperationId, WalletOperationStatus,
};

use crate::DaemonContext;
use crate::allocation_funding;
use crate::allocation_store::{self, GatewayAllocationItem, GatewayObservation};
use crate::daemon::Worker;
use crate::database::Database;
use crate::gateway::{ConfiguredGatewayClient, GatewayClient, GatewaySnapshot};
use crate::setup_store::{self};
use crate::wallet::{FundsWallet, GatewaydFundsWallet, get_wallet_operation};
use crate::{now_timestamp, run_interval_task, unavailable, validate_deposit_address};

pub(crate) async fn run_gateway_allocation_task(context: DaemonContext) -> anyhow::Result<()> {
    run_interval_task(
        context,
        Worker::GatewayAllocation,
        std::time::Duration::from_secs(10),
        "gateway allocation processing failed",
        |context| async move { process_gateway_allocations(&context).await },
    )
    .await
}

pub(crate) async fn run_gateway_observation_task(context: DaemonContext) -> anyhow::Result<()> {
    run_interval_task(
        context,
        Worker::GatewayObservation,
        std::time::Duration::from_secs(30),
        "configured gateway observation failed",
        |context| async move { observe_configured_gateway(&context).await },
    )
    .await
}

pub(crate) async fn process_gateway_allocations(context: &DaemonContext) -> ServiceResult<usize> {
    let (setup, wallet, gateway) = configured_gateway_dependencies(context).await?;
    process_gateway_allocations_with(
        &context.database,
        &setup,
        &wallet,
        &gateway,
        crate::endpoint_policy::EndpointPolicy::from_allow_private(
            context.args.allow_private_federation_endpoints,
        ),
    )
    .await
}

async fn observe_configured_gateway(context: &DaemonContext) -> ServiceResult<()> {
    let (setup, _wallet, gateway) = configured_gateway_dependencies(context).await?;
    let snapshot = gateway.gateway_info().await.map_err(unavailable)?;
    persist_gateway_snapshot(&context.database, &setup, &snapshot).await
}

pub(crate) async fn process_gateway_allocations_with(
    database: &Database,
    setup: &SetupConfigView,
    wallet: &impl FundsWallet,
    gateway: &impl GatewayClient,
    endpoint_policy: crate::endpoint_policy::EndpointPolicy,
) -> ServiceResult<usize> {
    let items = allocation_store::active_gateway_items(database).await?;
    let mut advanced = 0;
    for item in items {
        if process_gateway_item(database, setup, wallet, gateway, endpoint_policy, item).await? {
            advanced += 1;
        }
    }
    Ok(advanced)
}

async fn process_gateway_item(
    database: &Database,
    setup: &SetupConfigView,
    wallet: &impl FundsWallet,
    gateway: &impl GatewayClient,
    endpoint_policy: crate::endpoint_policy::EndpointPolicy,
    mut item: GatewayAllocationItem,
) -> ServiceResult<bool> {
    if !allocation_store::mark_item_running(database, &item.federation_id, &item.item_id).await? {
        return Ok(false);
    }

    let snapshot = gateway.gateway_info().await.map_err(unavailable)?;
    if snapshot.network != setup.network {
        allocation_store::require_item_action(
            database,
            &item.federation_id,
            &item.item_id,
            LiquidityFailureCode::GatewayAttachFailed,
            format!(
                "gateway network {} does not match configured network {}",
                snapshot.network, setup.network
            ),
        )
        .await?;
        return Ok(true);
    }
    if !snapshot.synced_to_chain {
        return Ok(false);
    }
    persist_gateway_snapshot(database, setup, &snapshot).await?;

    let mut advanced = false;
    let observed_federation = snapshot
        .federations
        .iter()
        .find(|federation| federation.federation_id == item.target.federation_id.0);

    if observed_federation.is_none() && !item.step.gateway_connected {
        // The same endpoint policy the target-client join takes, applied before
        // the operator's gateway process dials on FLIP's behalf.
        //
        // `connect_federation` posts the FI-supplied invite verbatim to that
        // process, which joins the federation from its API URLs. Unguarded that
        // is a dial on FLIP's production path to a host a requester chose, with
        // no policy anywhere along it. Being another process's dial does not
        // make it someone else's exposure: FLIP chose the invite and asked for
        // the connection.
        let approved_invite = match crate::endpoint_policy::check_invite_endpoints(
            endpoint_policy,
            &item.target.invite_code.0,
        )
        .await
        {
            Ok(invite) => invite,
            Err(error) => {
                tracing::warn!(
                    federation_id = %item.federation_id.0,
                    %error,
                    "target federation endpoint refused by policy"
                );
                allocation_store::require_item_action(
                    database,
                    &item.federation_id,
                    &item.item_id,
                    LiquidityFailureCode::GatewayAttachFailed,
                    "the target federation endpoint is not permitted".to_owned(),
                )
                .await?;
                return Ok(true);
            }
        };

        match gateway
            .connect_federation(&approved_invite.to_string())
            .await
        {
            Ok(federation) => {
                if federation.federation_id != item.target.federation_id.0 {
                    allocation_store::require_item_action(
                        database,
                        &item.federation_id,
                        &item.item_id,
                        LiquidityFailureCode::GatewayAttachFailed,
                        format!(
                            "gateway connected to federation {}, expected {}",
                            federation.federation_id, item.target.federation_id.0
                        ),
                    )
                    .await?;
                    return Ok(true);
                }
                item.step.gateway_connected = true;
                allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
                tracing::info!(
                    federation_id = %item.federation_id.0,
                    item_id = %item.item_id.0,
                    "the configured gateway joined the target federation"
                );
                advanced = true;
            }
            Err(error) => {
                // The requester reads this reason back through
                // `get_allocation_status`, so it does not carry the dial result.
                //
                // Not `error.to_string()`: that would distinguish refused from
                // timed-out from TLS failure for whatever host the invite
                // named, which is what makes a service a *port prober* rather
                // than merely a connection initiator. An address-class filter
                // cannot stop a dial to a third-party global host, but nothing
                // requires FLIP to report what it found there.
                //
                // The detail stays operator-side, where the adversary is not.
                tracing::warn!(
                    federation_id = %item.federation_id.0,
                    ?error,
                    "gateway could not attach the target federation"
                );
                allocation_store::require_item_action(
                    database,
                    &item.federation_id,
                    &item.item_id,
                    LiquidityFailureCode::GatewayAttachFailed,
                    "the configured gateway could not attach this federation".to_owned(),
                )
                .await?;
                return Ok(true);
            }
        }
    } else if observed_federation.is_some() && !item.step.gateway_connected {
        item.step.gateway_connected = true;
        allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
        advanced = true;
    }

    if item.step.deposit_address.is_none() {
        match gateway
            .deposit_address(&item.target.federation_id.0, setup.network)
            .await
        {
            Ok(address) => {
                if let Err(error) = validate_deposit_address(&address, setup.network) {
                    allocation_store::require_item_action(
                        database,
                        &item.federation_id,
                        &item.item_id,
                        LiquidityFailureCode::GatewayAttachFailed,
                        error,
                    )
                    .await?;
                    return Ok(true);
                }
                item.step.deposit_address = Some(address);
                allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
                advanced = true;
            }
            Err(error) => {
                allocation_store::require_item_action(
                    database,
                    &item.federation_id,
                    &item.item_id,
                    LiquidityFailureCode::GatewayAttachFailed,
                    error.to_string(),
                )
                .await?;
                return Ok(true);
            }
        }
    }

    let operation =
        allocation_funding::ensure_wallet_operation(database, setup, &gateway_funding_step(&item))
            .await?;
    let Some(operation) = operation else {
        return Ok(false);
    };
    if item.step.wallet_operation_id.as_deref() != Some(operation.operation_id.0.as_str()) {
        item.step.wallet_operation_id = Some(operation.operation_id.0.clone());
        allocation_store::update_item_step(database, &item.item_id, &item.step).await?;
    }
    match operation.status {
        WalletOperationStatus::Pending => {
            allocation_funding::submit_funding_withdrawal(
                database,
                setup,
                wallet,
                &gateway_funding_step(&item),
                &operation.operation_id,
            )
            .await?;
            Ok(true)
        }
        WalletOperationStatus::Broadcast | WalletOperationStatus::Confirmed => Ok(advanced),
        WalletOperationStatus::InDoubt | WalletOperationStatus::ManualReviewRequired => {
            Ok(advanced)
        }
        WalletOperationStatus::Failed => {
            allocation_store::require_item_action(
                database,
                &item.federation_id,
                &item.item_id,
                LiquidityFailureCode::WithdrawFailed,
                "gateway funding wallet operation failed",
            )
            .await?;
            Ok(true)
        }
        WalletOperationStatus::Cancelled => {
            allocation_store::fail_item(
                database,
                &item.federation_id,
                &item.item_id,
                LiquidityFailureCode::WithdrawFailed,
                "gateway funding wallet operation was cancelled while item was active",
            )
            .await?;
            Ok(true)
        }
        WalletOperationStatus::Completed => {
            complete_if_gateway_funded(database, setup, gateway, item, operation.operation_id).await
        }
    }
}

fn gateway_funding_step(item: &GatewayAllocationItem) -> allocation_funding::FundingStep<'_> {
    allocation_funding::FundingStep {
        kind: allocation_funding::FundingKind::Gateway,
        federation_id: &item.federation_id,
        item_id: &item.item_id,
        address: item.step.deposit_address.as_deref(),
        amount: gateway_withdrawal_amount(item),
    }
}

fn gateway_withdrawal_amount(item: &GatewayAllocationItem) -> Sats {
    Sats(item.reserved_amount.0.max(item.committed_amount.0))
}

async fn complete_if_gateway_funded(
    database: &Database,
    setup: &SetupConfigView,
    gateway: &impl GatewayClient,
    item: GatewayAllocationItem,
    operation_id: WalletOperationId,
) -> ServiceResult<bool> {
    let operation = get_wallet_operation(database, &operation_id).await?;
    // The item's own funding output is the only target-side credit that can
    // complete it. The gateway's `deposit-confirmed` log names the txid,
    // output index, and amount of every deposit its Fedimint client claimed,
    // so matching those against the funding operation is attribution; a
    // federation-wide balance inequality is not, because a concurrent item or
    // an independent deposit raises the same aggregate.
    let Some(funding_txid) = operation.txid.as_deref() else {
        recheck_gateway_deposit(setup, gateway, &item).await?;
        return Ok(false);
    };
    let claims = match gateway.deposit_claims(&item.target.federation_id.0).await {
        Ok(claims) => claims,
        // A gateway that cannot answer for this federation leaves the item
        // running until it can. Every other item of the pass is independent
        // of this one, so one unanswered read must not end their turn.
        Err(error) => {
            tracing::warn!(
                federation_id = %item.target.federation_id.0,
                item_id = %item.item_id.0,
                %error,
                "gateway could not report its claimed deposits"
            );
            return Ok(false);
        }
    };
    // One transaction can pay two items' deposit addresses in separate
    // outputs, so a txid alone does not name the output this item funded.
    // Chain observation settles allocation funding sends and records the
    // output index it verified there; a manual review resolved by an operator
    // leaves `tx_vout` unset, and the asserted txid is then the whole of the
    // attribution that exists.
    let claimed = claims.iter().any(|claim| {
        claim.txid == funding_txid
            && operation.tx_vout.is_none_or(|vout| claim.out_idx == vout)
            && claim.amount.0 >= item.committed_amount.0
    });
    if !claimed {
        recheck_gateway_deposit(setup, gateway, &item).await?;
        return Ok(false);
    }
    // Completion evidence records what the gateway reported for the funded
    // federation, so a gateway that reports no such federation has nothing to
    // record and the item waits for one that does.
    let Some(observed_balance) = gateway
        .observe_federation_balance(&item.target.federation_id.0)
        .await
        .map_err(unavailable)?
    else {
        recheck_gateway_deposit(setup, gateway, &item).await?;
        return Ok(false);
    };
    allocation_store::upsert_gateway_observation(
        database,
        &GatewayObservation {
            gateway_id: setup.gateway.gateway_id.clone(),
            federation_id: Some(item.target.federation_id.0.clone()),
            status: "federation_observed".to_owned(),
            observed_balance: Some(observed_balance),
            observed_at: now_timestamp(),
        },
    )
    .await?;
    let gateway_api = gateway
        .gateway_info()
        .await
        .map_err(unavailable)?
        .gateway_api;
    allocation_store::complete_item(
        database,
        &item.federation_id,
        &item.item_id,
        item.committed_amount,
        CompletionEvidence::Gateway(GatewayCompletionEvidence {
            gateway_id: setup.gateway.gateway_id.clone(),
            gateway_api,
            fulfilled_amount: item.committed_amount,
            observed_gateway_balance: observed_balance,
            observed_at: now_timestamp(),
            withdrawal_txid: operation.txid,
            wallet_operation_id: Some(operation_id),
        }),
    )
    .await?;
    Ok(true)
}

async fn recheck_gateway_deposit(
    setup: &SetupConfigView,
    gateway: &impl GatewayClient,
    item: &GatewayAllocationItem,
) -> ServiceResult<()> {
    let Some(address) = item.step.deposit_address.as_deref() else {
        return Ok(());
    };
    gateway
        .recheck_deposit_address(&item.target.federation_id.0, address, setup.network)
        .await
        .map_err(unavailable)
}

async fn persist_gateway_snapshot(
    database: &Database,
    setup: &SetupConfigView,
    snapshot: &GatewaySnapshot,
) -> ServiceResult<()> {
    let observed_at = now_timestamp();
    allocation_store::upsert_gateway_observation(
        database,
        &GatewayObservation {
            gateway_id: setup.gateway.gateway_id.clone(),
            federation_id: None,
            status: snapshot.state.clone(),
            observed_balance: None,
            observed_at,
        },
    )
    .await?;
    for federation in &snapshot.federations {
        allocation_store::upsert_gateway_observation(
            database,
            &GatewayObservation {
                gateway_id: setup.gateway.gateway_id.clone(),
                federation_id: Some(federation.federation_id.clone()),
                status: "federation_observed".to_owned(),
                observed_balance: Some(federation.balance),
                observed_at,
            },
        )
        .await?;
    }
    Ok(())
}

async fn configured_gateway_dependencies(
    context: &DaemonContext,
) -> ServiceResult<(
    SetupConfigView,
    GatewaydFundsWallet,
    ConfiguredGatewayClient,
)> {
    let (config, credential) =
        setup_store::ready_gateway_config(&context.database, &context.secret_store).await?;
    let wallet = GatewaydFundsWallet::new(config.clone(), credential.clone())
        .await
        .map_err(unavailable)?;
    let gateway = ConfiguredGatewayClient::new(config.clone(), credential)
        .await
        .map_err(unavailable)?;
    Ok((config, wallet, gateway))
}

#[cfg(test)]
#[path = "../tests/gateway_allocation.rs"]
mod tests;
