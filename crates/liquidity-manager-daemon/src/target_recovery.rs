//! Operator reconciliation for value that reached a target federation client.
//!
//! Every post-peg-in exit that FLIP cannot resolve on its own leaves the item
//! `action_required` with the provider's e-cash already inside the target
//! client. That is deliberate — guessing costs money either way (see
//! [SPEC-flip-funding-safety](../specs/SPEC-flip-funding-safety.md)) — but it
//! is only half an answer, because until an operator can see what the client
//! actually did and tell FLIP about it, the item has no route onward.
//!
//! These operations are that route, and none of them moves money. Two concern a
//! deposit that may exist: one reads the target client, the other records which
//! of its deposits an operator concluded is this item's. The third is for when
//! there is nothing to bind — a pool that will never accept the deposit — and
//! releases the capacity the item holds while recording that the value stays
//! where it is.

use fedi_decentralized_service_liquidity_manager::{
    AbandonTargetClientValueRequest, AbandonTargetClientValueResponse, BindTargetDepositRequest,
    BindTargetDepositResponse, InspectTargetClientRequest, InspectTargetClientResponse,
    ItemAllocationStatus, LiquidityFailureCode, ManualOperationStatus, ServiceResult,
    TargetDepositOperationView, Timestamp,
};

use crate::allocation_store::{self, PegInProgress, SpDepositStatus, StabilityPoolAllocationItem};
use crate::database::Database;
use crate::internal_error;
use crate::stability_pool::{StabilityDepositStatus, StabilityPoolBackend, TargetDepositOperation};

pub(crate) async fn inspect_target_client(
    database: &Database,
    backend: &impl StabilityPoolBackend,
    request: InspectTargetClientRequest,
) -> ServiceResult<InspectTargetClientResponse> {
    let item = allocation_store::stability_pool_item(database, &request.federation_id)
        .await?
        .ok_or_else(|| {
            crate::not_found(format!(
                "no stability-pool allocation item for federation {}",
                request.federation_id.0
            ))
        })?;

    let report = backend
        .report(&item.target)
        .await
        .map_err(crate::unavailable)?;
    let spendable_balance = backend
        .target_wallet_balance(&item.target)
        .await
        .map_err(crate::unavailable)?;
    let deposit_operations = backend
        .list_deposit_operations(&item.target)
        .await
        .map_err(crate::unavailable)?;

    Ok(InspectTargetClientResponse {
        spendable_balance,
        observed_provided_amount: report.observed_provided_amount,
        liquidity_stats_json: report.liquidity_stats_json,
        recorded_deposit_operation_id: item.step.sp_deposit_operation_id.clone(),
        scan_complete: deposit_operations.complete,
        deposit_operations: deposit_operations
            .operations
            .iter()
            .map(deposit_view)
            .collect(),
    })
}

pub(crate) async fn bind_target_deposit(
    database: &Database,
    backend: &impl StabilityPoolBackend,
    request: BindTargetDepositRequest,
) -> ServiceResult<BindTargetDepositResponse> {
    let Some(item) =
        allocation_store::stability_pool_item(database, &request.federation_id).await?
    else {
        return audited(
            database,
            &request,
            ManualOperationStatus::NotFound,
            "no stability-pool allocation item for this federation",
        )
        .await;
    };

    // Only an item FLIP has already given up on may be bound. A running item is
    // still FLIP's to advance, and a terminal one has had its reservation
    // settled — reopening either from outside the worker would race it.
    if item.status != ItemAllocationStatus::ActionRequired {
        return audited(
            database,
            &request,
            ManualOperationStatus::Rejected,
            format!(
                "allocation item is {} and is not awaiting operator action",
                item.status
            ),
        )
        .await;
    }
    if item.corrupt_step_json.is_some() {
        return audited(
            database,
            &request,
            ManualOperationStatus::Rejected,
            "allocation step is unreadable; bind is unsafe, use audited abandon after inspecting the retained raw state",
        )
        .await;
    }

    // Binding over an existing id would silently redirect the item's completion
    // evidence to a different deposit.
    if let Some(existing) = &item.step.sp_deposit_operation_id {
        let status = if existing == &request.operation_id {
            ManualOperationStatus::AlreadyApplied
        } else {
            ManualOperationStatus::Rejected
        };
        return audited(
            database,
            &request,
            status,
            format!("allocation item already records deposit operation {existing}"),
        )
        .await;
    }

    // The operator names an operation; the target client is what decides
    // whether it exists. Accepting an unverified id would let a typo attach the
    // item to nothing and complete against a sibling's deposit later.
    let operation_id =
        crate::stability_deposit::StabilityDepositOperationId::parse(&request.operation_id)
            .map_err(crate::invalid_argument)?;
    let operation = backend
        .get_deposit_operation(&item.target, operation_id)
        .await
        .map_err(crate::unavailable)?;
    let Some(operation) = operation else {
        return audited(
            database,
            &request,
            ManualOperationStatus::NotFound,
            "the target client records no stability-pool deposit with that operation id",
        )
        .await;
    };

    // A deposit smaller than the item committed cannot be the one that
    // discharges it, and binding it would complete the item for less than it
    // promised.
    if operation.amount < item.committed_amount {
        return audited(
            database,
            &request,
            ManualOperationStatus::Rejected,
            format!(
                "deposit of {} sats is below this item's committed {} sats",
                operation.amount.0, item.committed_amount.0
            ),
        )
        .await;
    }

    bind_and_resume(database, &item, operation_id).await?;
    audited(
        database,
        &request,
        ManualOperationStatus::Accepted,
        "deposit bound; the stability worker will resume observing it",
    )
    .await
}

/// Records the operation on the item and returns it to the worker.
///
/// The status is set to `initiated` rather than to whatever the client has
/// cached: observation is the worker's job, and starting from the earliest
/// state means the normal path re-derives everything, including the provider
/// report gate that completion depends on.
async fn bind_and_resume(
    database: &Database,
    item: &StabilityPoolAllocationItem,
    operation_id: crate::stability_deposit::StabilityDepositOperationId,
) -> ServiceResult<()> {
    let mut step = item.step.clone();
    step.sp_deposit_amount = Some(item.committed_amount);
    step.sp_deposit_min_fee_rate_ppb.get_or_insert(0);
    // Through the monotone setter, which also writes the operation id. Binding a
    // *new* id starts a new deposit at `initiated`, which is the point of this
    // verb. Re-binding the id already recorded is different: if that deposit
    // reached `success`, `initiated` would walk a terminal state backwards, and
    // the setter refuses. The operator is told rather than silently given a
    // resumption that discards a terminal observation.
    if !step.advance_sp_deposit_status(&operation_id.to_string(), SpDepositStatus::Initiated) {
        return Err(crate::failed_precondition(
            "this operation id already reached a terminal deposit state for this item; \
             resuming it would discard that observation",
        ));
    }

    let mut tx = database.begin_write().await.map_err(internal_error)?;
    // Guarded on the status the caller checked, so a worker or operator that
    // moved the item in between wins rather than being overwritten.
    let result = sqlx::query(
        "UPDATE allocation_items \
         SET status = ?, step_json = ?, failure_json = NULL, updated_at = unixepoch() \
         WHERE item_id = ? AND status = ?",
    )
    .bind(ItemAllocationStatus::Running.to_string())
    .bind(serde_json::to_string(&step).map_err(internal_error)?)
    .bind(&item.item_id.0)
    .bind(ItemAllocationStatus::ActionRequired.to_string())
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;
    if result.rows_affected() != 1 {
        return Err(crate::failed_precondition(
            "allocation item changed status while the deposit was being bound",
        ));
    }
    tx.commit().await.map_err(internal_error)?;
    Ok(())
}

/// Gives up on an item whose target-client value FLIP cannot recover, releasing
/// the capacity it holds.
///
/// This exists because such an item has no terminal state otherwise. After the
/// peg-in is claimed the funding send has settled, so `cancel_allocation` and
/// `retry_funding_step` both refuse; if the pool will never accept the deposit
/// the item can never complete either, and `action_required` reserves provider
/// capacity, so one federation that rejects provision would consume it forever.
///
/// It separates two problems that are otherwise fused: the capacity is released
/// here and now, while the value stays where it is and is recorded as needing
/// recovery outside FLIP. It moves no money and recovers none.
pub(crate) async fn abandon_target_client_value(
    database: &Database,
    request: AbandonTargetClientValueRequest,
) -> ServiceResult<AbandonTargetClientValueResponse> {
    if request.reason.trim().is_empty() {
        return abandon_audited(
            database,
            &request,
            ManualOperationStatus::Rejected,
            None,
            "abandoning target-client value requires an operator reason",
        )
        .await;
    }

    let Some(item) =
        allocation_store::stability_pool_item(database, &request.federation_id).await?
    else {
        return abandon_audited(
            database,
            &request,
            ManualOperationStatus::NotFound,
            None,
            "no stability-pool allocation item for this federation",
        )
        .await;
    };

    if item.status != ItemAllocationStatus::ActionRequired {
        return abandon_audited(
            database,
            &request,
            ManualOperationStatus::Rejected,
            None,
            format!(
                "allocation item is {} and is not awaiting operator action",
                item.status
            ),
        )
        .await;
    }

    // Only the post-peg-in state needs this. Before the peg-in is claimed the
    // provider's funds have not reached the target client, and `retry` or
    // `cancel` can still resolve the item without writing anything off.
    if item.corrupt_step_json.is_none() && item.step.peg_in_status != Some(PegInProgress::Claimed) {
        return abandon_audited(
            database,
            &request,
            ManualOperationStatus::Rejected,
            None,
            "no value has reached the target client for this item; resolve it with              retry_funding_step or cancel_allocation instead",
        )
        .await;
    }

    let abandoned_amount = item.step.peg_in_amount;
    let detail = match abandoned_amount {
        Some(amount) => format!(
            "operator abandoned {} sats held by the target federation client; the value              remains there and needs recovery outside FLIP. Reason: {}",
            amount.0,
            request.reason.trim()
        ),
        None => format!(
            "operator abandoned value held by the target federation client; the value              remains there and needs recovery outside FLIP. Reason: {}",
            request.reason.trim()
        ),
    };

    let mut tx = database.begin_write().await.map_err(internal_error)?;
    // Guarded on the status the caller checked. `set_item_failure` cannot be
    // reused: it deliberately refuses to move an `action_required` item, which
    // is the whole state this operation exists to resolve.
    let result = sqlx::query(
        "UPDATE allocation_items \
         SET status = ?, failure_json = ?, updated_at = unixepoch() \
         WHERE item_id = ? AND status = ?",
    )
    .bind(ItemAllocationStatus::Failed.to_string())
    .bind(
        serde_json::to_string(
            &fedi_decentralized_service_liquidity_manager::LiquidityFailure {
                code: LiquidityFailureCode::StabilityPoolFailed,
                reason: Some(detail.clone()),
            },
        )
        .map_err(internal_error)?,
    )
    .bind(&item.item_id.0)
    .bind(ItemAllocationStatus::ActionRequired.to_string())
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;
    if result.rows_affected() != 1 {
        return Err(crate::failed_precondition(
            "allocation item changed status while it was being abandoned",
        ));
    }
    let detail_json = serde_json::json!({
        "federation_id": request.federation_id,
        "reason": request.reason,
        "abandoned_amount": abandoned_amount,
        "outcome": ManualOperationStatus::Accepted.to_string(),
        "detail": detail,
    });
    sqlx::query(
        "INSERT INTO audit_log (action, detail_json, created_at) VALUES (?, ?, unixepoch())",
    )
    .bind("abandon_target_client_value")
    .bind(detail_json.to_string())
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;
    tx.commit().await.map_err(internal_error)?;
    // Warned rather than noted: this is the one operator action that ends
    // FLIP's management of value that is really there, and no later pass
    // revisits it.
    tracing::warn!(
        federation_id = %request.federation_id.0,
        abandoned_sats = abandoned_amount.map(|amount| amount.0).unwrap_or(0),
        reason = %request.reason,
        "operator abandoned target-client value; it is no longer managed by FLIP"
    );

    Ok(AbandonTargetClientValueResponse {
        status: ManualOperationStatus::Accepted,
        abandoned_amount,
        detail: Some(detail),
    })
}

async fn abandon_audited(
    database: &Database,
    request: &AbandonTargetClientValueRequest,
    outcome: ManualOperationStatus,
    abandoned_amount: Option<fedi_decentralized_service_liquidity_manager::Sats>,
    detail: impl Into<String>,
) -> ServiceResult<AbandonTargetClientValueResponse> {
    let detail = detail.into();
    let detail_json = serde_json::json!({
        "federation_id": request.federation_id,
        "reason": request.reason,
        "abandoned_amount": abandoned_amount,
        "outcome": outcome.to_string(),
        "detail": detail,
    });
    sqlx::query(
        "INSERT INTO audit_log (action, detail_json, created_at) VALUES (?, ?, unixepoch())",
    )
    .bind("abandon_target_client_value")
    .bind(detail_json.to_string())
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    tracing::info!(
        federation_id = %request.federation_id.0,
        outcome = %outcome,
        %detail,
        "abandon_target_client_value recorded"
    );
    Ok(AbandonTargetClientValueResponse {
        status: outcome,
        abandoned_amount,
        detail: Some(detail),
    })
}

fn deposit_view(operation: &TargetDepositOperation) -> TargetDepositOperationView {
    let (outcome, failure_detail) = match &operation.outcome {
        Some(StabilityDepositStatus::Success) => (Some("success"), None),
        Some(StabilityDepositStatus::TxAccepted) => (Some("tx_accepted"), None),
        Some(StabilityDepositStatus::Initiated) => (Some("initiated"), None),
        Some(StabilityDepositStatus::Failed(detail)) => (Some("failed"), Some(detail.clone())),
        None => (None, None),
    };
    TargetDepositOperationView {
        operation_id: operation.operation_id.clone(),
        amount: operation.amount,
        outcome: outcome.map(str::to_owned),
        failure_detail,
        created_at: Timestamp(operation.created_at),
    }
}

async fn audited(
    database: &Database,
    request: &BindTargetDepositRequest,
    outcome: ManualOperationStatus,
    detail: impl Into<String>,
) -> ServiceResult<BindTargetDepositResponse> {
    let detail = detail.into();
    let detail_json = serde_json::json!({
        "federation_id": request.federation_id,
        "operation_id": request.operation_id,
        "reason": request.reason,
        "outcome": outcome.to_string(),
        "detail": detail,
    });
    sqlx::query(
        "INSERT INTO audit_log (action, detail_json, created_at) VALUES (?, ?, unixepoch())",
    )
    .bind("bind_target_deposit")
    .bind(detail_json.to_string())
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    tracing::info!(
        federation_id = %request.federation_id.0,
        operation_id = %request.operation_id,
        outcome = %outcome,
        %detail,
        "bind_target_deposit recorded"
    );
    Ok(BindTargetDepositResponse {
        status: outcome,
        detail: Some(detail),
    })
}

#[cfg(test)]
#[path = "../tests/target_recovery.rs"]
mod tests;
