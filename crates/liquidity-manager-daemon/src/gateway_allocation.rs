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
    CompletionEvidence, GatewayCompletionEvidence, LiquidityFailureCode, Sats, ServiceErrorCode,
    ServiceResult, SetupConfigView, WalletOperation, WalletOperationId, WalletOperationStatus,
};

use crate::DaemonContext;
use crate::allocation_funding;
use crate::allocation_store::{self, GatewayAllocationItem, GatewayObservation};
use crate::daemon::Worker;
use crate::database::Database;
use crate::gateway::{ConfiguredGatewayClient, DepositClaimQuery, GatewayClient, GatewaySnapshot};
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
    observe_configured_gateway_with(&context.database, &setup, &gateway).await
}

pub(crate) async fn observe_configured_gateway_with(
    database: &Database,
    setup: &SetupConfigView,
    gateway: &impl GatewayClient,
) -> ServiceResult<()> {
    // Both workers read the gateway the same way, so neither can overwrite an
    // outage the other recorded. Persisting an unsynced snapshot here would
    // replace the outage row and its start time with a live-looking state, and
    // the next allocation pass would open a second outage for the same one.
    let Some(snapshot) = read_usable_gateway_snapshot(database, setup, gateway).await? else {
        return Ok(());
    };
    persist_gateway_snapshot(database, setup, &snapshot).await
}

pub(crate) async fn process_gateway_allocations_with(
    database: &Database,
    setup: &SetupConfigView,
    wallet: &impl FundsWallet,
    gateway: &impl GatewayClient,
    endpoint_policy: crate::endpoint_policy::EndpointPolicy,
) -> ServiceResult<usize> {
    let items = allocation_store::active_gateway_items(database).await?;
    if items.is_empty() {
        return Ok(0);
    }
    // One gateway serves every item, so it is asked once a pass and its answer
    // decides the whole pass. A gateway that cannot answer, or that has not
    // caught up with the chain, has said nothing about any item: it reports no
    // claim it has not yet read, so acting on that silence would read an outage
    // as evidence.
    let Some(snapshot) = read_usable_gateway_snapshot(database, setup, gateway).await? else {
        return Ok(0);
    };
    persist_gateway_snapshot(database, setup, &snapshot).await?;

    let mut advanced = 0;
    for item in items {
        let item_id = item.item_id.clone();
        match process_gateway_item(
            database,
            setup,
            wallet,
            gateway,
            endpoint_policy,
            &snapshot,
            item,
        )
        .await
        {
            Ok(true) => advanced += 1,
            Ok(false) => {}
            // Items are independent, and a dependency that failed one of them
            // may well answer the next. Ending the pass here would make one
            // item's unlucky moment everybody else's outage.
            Err(error) if error.code() == ServiceErrorCode::Unavailable => {
                tracing::warn!(
                    item_id = %item_id.0,
                    %error,
                    "a dependency could not answer for this gateway item"
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(advanced)
}

/// Reads the gateway and reports it only when it can actually answer for an
/// item, recording an outage rather than failing the caller.
///
/// `None` covers both ways the gateway has nothing to say: it did not answer,
/// or it answered without having caught up with the chain, so it has not read
/// the deposits it would be asked about. Treating those alike is what keeps one
/// outage one outage — an unsynced gateway is not a recovery, and reporting it
/// as one would restart the clock every pass.
///
/// The outage is durable and dated, so its length is readable rather than
/// inferred from how long the log has been repeating itself.
async fn read_usable_gateway_snapshot(
    database: &Database,
    setup: &SetupConfigView,
    gateway: &impl GatewayClient,
) -> ServiceResult<Option<GatewaySnapshot>> {
    match gateway.gateway_info().await {
        Ok(snapshot) if snapshot.synced_to_chain => {
            note_gateway_answers(database, setup).await?;
            Ok(Some(snapshot))
        }
        Ok(_) => {
            note_gateway_cannot_answer(
                database,
                setup,
                "gatewayd has not caught up with the chain",
            )
            .await?;
            Ok(None)
        }
        Err(error) => {
            note_gateway_cannot_answer(database, setup, &error.to_string()).await?;
            Ok(None)
        }
    }
}

async fn process_gateway_item(
    database: &Database,
    setup: &SetupConfigView,
    wallet: &impl FundsWallet,
    gateway: &impl GatewayClient,
    endpoint_policy: crate::endpoint_policy::EndpointPolicy,
    snapshot: &GatewaySnapshot,
    mut item: GatewayAllocationItem,
) -> ServiceResult<bool> {
    if !allocation_store::mark_item_running(database, &item.federation_id, &item.item_id).await? {
        return Ok(false);
    }

    // A configured gateway on the wrong network is a settled fact about the
    // deployment, not a dependency having a bad moment, so it stops the item
    // rather than making it wait.
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
        recheck_and_note_delay(
            database,
            setup,
            gateway,
            &item,
            &operation,
            "the settled funding send records no transaction id, so no gateway claim \
             can name the output it paid",
        )
        .await?;
        return Ok(false);
    };
    // Chain observation settles allocation funding sends and records the
    // output index it verified there, which is what separates two items paid
    // by one transaction.
    let query = DepositClaimQuery {
        txid: funding_txid,
        out_idx: operation.tx_vout,
        min_amount: item.committed_amount,
    };
    let claim = match gateway
        .find_deposit_claim(&item.target.federation_id.0, &query)
        .await
    {
        Ok(claim) => claim,
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
    if claim.is_none() {
        recheck_and_note_delay(
            database,
            setup,
            gateway,
            &item,
            &operation,
            "the gateway does not report claiming the deposit this item funded",
        )
        .await?;
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
        recheck_and_note_delay(
            database,
            setup,
            gateway,
            &item,
            &operation,
            "the gateway does not report the funded federation, so there is no balance \
             to record as completion evidence",
        )
        .await?;
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
    .await
}

/// Rechecks the deposit address, and raises a long wait for an operator.
///
/// Both halves apply on every pass where the gateway has not yet attributed
/// the item's funding output. The recheck asks the gateway to look at the
/// address again; the notice tells an operator that the looking has gone on
/// long enough to be worth their attention.
async fn recheck_and_note_delay(
    database: &Database,
    setup: &SetupConfigView,
    gateway: &impl GatewayClient,
    item: &GatewayAllocationItem,
    operation: &WalletOperation,
    detail: &str,
) -> ServiceResult<()> {
    note_attribution_delay(database, setup, item, operation, detail).await?;
    recheck_gateway_deposit(setup, gateway, item).await
}

/// Records that the gateway's attribution for this item is overdue.
///
/// The age is measured from the funding operation's last update, which for a
/// settled send is when it reached `completed`. Terminal wallet states are
/// monotonic, so that timestamp stops moving and the age only grows.
///
/// Passing the threshold says a human should look, not that the money is gone.
/// Missing confirmation does not establish whether the gateway received the
/// deposit: the gateway may be offline, resyncing, or behind on its log. So the
/// marker rides in the item step and the item keeps its active status, which is
/// what lets the next pass read the gateway's log again and complete the item
/// from evidence that arrives afterwards, with no operator action at all.
///
/// The marker is written once. A worker that keeps finding the same wait calls
/// this every pass, so an unguarded event would repeat one fact for as long as
/// the condition lasted.
async fn note_attribution_delay(
    database: &Database,
    setup: &SetupConfigView,
    item: &GatewayAllocationItem,
    operation: &WalletOperation,
    detail: &str,
) -> ServiceResult<()> {
    if item.step.attribution_overdue_since.is_some() {
        return Ok(());
    }
    let threshold = setup.funding_policy.gateway_claim_review_after_secs;
    if threshold == 0 {
        return Ok(());
    }
    let now = now_timestamp();
    let unattributed_for = now.0.saturating_sub(operation.updated_at.0);
    if unattributed_for < threshold {
        return Ok(());
    }

    let mut step = item.step.clone();
    step.attribution_overdue_since = Some(now);
    allocation_store::update_item_step(database, &item.item_id, &step).await?;
    tracing::warn!(
        federation_id = %item.target.federation_id.0,
        item_id = %item.item_id.0,
        unattributed_for_secs = unattributed_for,
        detail,
        "gateway funding deposit is still unattributed; the item stays active and \
         keeps reconciling"
    );
    Ok(())
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

/// The status the gateway row carries while the gateway cannot answer.
///
/// Every successful observation overwrites the row with gatewayd's own state
/// string, so this value is never one of those and its presence means the last
/// thing FLIP learned was that the gateway was not answering.
const GATEWAY_UNAVAILABLE_STATUS: &str = "unavailable";

/// Records that the gateway cannot answer, dating the outage from its start.
///
/// Written once per outage. While the gateway answers, the row's `observed_at`
/// means "last seen"; while it does not, the row is left alone so the same
/// field means "unavailable since". Repeating the write each pass would keep
/// resetting that to now and hide exactly the thing worth knowing, which is
/// how long this has been going on.
async fn note_gateway_cannot_answer(
    database: &Database,
    setup: &SetupConfigView,
    detail: &str,
) -> ServiceResult<()> {
    let recorded = allocation_store::gateway_observation(database, &setup.gateway.gateway_id)
        .await?
        .is_some_and(|observation| observation.status == GATEWAY_UNAVAILABLE_STATUS);
    if recorded {
        return Ok(());
    }
    allocation_store::upsert_gateway_observation(
        database,
        &GatewayObservation {
            gateway_id: setup.gateway.gateway_id.clone(),
            federation_id: None,
            status: GATEWAY_UNAVAILABLE_STATUS.to_owned(),
            observed_balance: None,
            observed_at: now_timestamp(),
        },
    )
    .await?;
    tracing::warn!(
        gateway_id = %setup.gateway.gateway_id.0,
        detail,
        "gateway cannot answer; its allocation items wait rather than fail"
    );
    Ok(())
}

/// Reports the end of an outage, once, with how long it lasted.
async fn note_gateway_answers(database: &Database, setup: &SetupConfigView) -> ServiceResult<()> {
    let Some(previous) =
        allocation_store::gateway_observation(database, &setup.gateway.gateway_id).await?
    else {
        return Ok(());
    };
    if previous.status != GATEWAY_UNAVAILABLE_STATUS {
        return Ok(());
    }
    tracing::info!(
        gateway_id = %setup.gateway.gateway_id.0,
        unavailable_for_secs = now_timestamp().0.saturating_sub(previous.observed_at.0),
        "gateway answers again"
    );
    Ok(())
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
