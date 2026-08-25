//! Guarded operator remediation for allocations FLIP cannot resolve on its own.
//!
//! Each verb is deliberately narrow, and each irreversible one is fenced on the
//! state it expects, so a stale operator view cannot act on a row that has
//! moved. `resolve_manual_review` requires exact chain evidence;
//! `complete_review_without_evidence` is the recorded route through when FLIP
//! cannot obtain it.

use fedi_decentralized_service_liquidity_manager::{
    AllocationStatus, CancelAllocationRequest, CancelAllocationResponse,
    CompleteReviewWithoutEvidenceRequest, CompleteReviewWithoutEvidenceResponse, FederationId,
    ItemAllocationStatus, ItemId, ManualOperationStatus, ManualReviewResolution, Pubkey,
    ReleaseFederationAllocationRequest, ReleaseFederationAllocationResponse,
    ResolveManualReviewRequest, ResolveManualReviewResponse, RetryFundingStepRequest,
    RetryFundingStepResponse, ServiceResult, WalletOperationId, WalletOperationStatus,
};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::DaemonContext;
use crate::allocation_store;
use crate::database::Database;
use crate::internal_error;
use crate::wallet;
use crate::wallet::ManualReviewOutcome;

pub(crate) async fn retry_funding_step(
    context: &DaemonContext,
    request: RetryFundingStepRequest,
) -> ServiceResult<RetryFundingStepResponse> {
    retry_funding_step_with_database(&context.database, request).await
}

pub(crate) async fn cancel_allocation(
    context: &DaemonContext,
    request: CancelAllocationRequest,
) -> ServiceResult<CancelAllocationResponse> {
    cancel_allocation_with_database(&context.database, request).await
}

/// Releases a federation's allocation binding when it is idle but wedged.
///
/// `SPEC-flip-rpc`'s second mechanism. The first —
/// takeover by a verified requester inside the admission path — handles the
/// ordinary case on its own and needs no operator. This one exists for when
/// that does not happen: nobody else is asking for the federation, or the
/// operator needs it free now.
///
/// **It overrides who holds a federation, not whether the allocation is idle.**
/// The predicate is `allocation_holding_tx`, exactly the one the automatic
/// takeover uses: nothing reserving, nothing awaiting settlement, no delivered
/// value. An operator cannot release an allocation that still holds work, and
/// the refusal names what it holds. Cancelling that work first is
/// `cancel_allocation`'s job, and it stays a separate decision because it is a
/// separate one: giving up on funding in flight is not the same as handing the
/// federation to the next requester.
///
/// Modelled on `abandon_target_client_value`, including its guard order — the
/// reason first, then existence, then state — and its refusal to proceed on a
/// row count it did not expect.
pub(crate) async fn release_federation_allocation(
    context: &DaemonContext,
    request: ReleaseFederationAllocationRequest,
) -> ServiceResult<ReleaseFederationAllocationResponse> {
    release_federation_allocation_with_database(&context.database, request).await
}

pub(crate) async fn release_federation_allocation_with_database(
    database: &Database,
    request: ReleaseFederationAllocationRequest,
) -> ServiceResult<ReleaseFederationAllocationResponse> {
    let reason = request.reason.trim();
    if reason.is_empty() {
        return release_audited(
            database,
            &request,
            ManualOperationStatus::Rejected,
            None,
            "releasing a federation allocation requires an operator reason",
        )
        .await;
    }

    let mut tx = database.begin_write().await.map_err(internal_error)?;
    let outcome = allocation_store::release_allocation_tx(&mut tx, &request.federation_id).await?;
    match outcome {
        allocation_store::AllocationReleaseOutcome::NotFound => {
            tx.rollback().await.map_err(internal_error)?;
            release_audited(
                database,
                &request,
                ManualOperationStatus::NotFound,
                None,
                "no allocation for this federation",
            )
            .await
        }
        allocation_store::AllocationReleaseOutcome::Held(holding) => {
            tx.rollback().await.map_err(internal_error)?;
            release_audited(
                database,
                &request,
                ManualOperationStatus::Rejected,
                None,
                &format!(
                    "allocation still holds work or value and cannot be released: \
                     {} reserving item(s), {} wallet operation(s) awaiting settlement, \
                     {} sat(s) delivered. Resolve or cancel that work first",
                    holding.reserving_items, holding.pending_operations, holding.fulfilled_sats
                ),
            )
            .await
        }
        allocation_store::AllocationReleaseOutcome::Released { previous_requester } => {
            let detail = format!(
                "released the allocation binding for federation {}; the federation can be \
                 requested again by any verified requester. Reason: {}",
                request.federation_id.0, reason
            );
            insert_audit_tx(
                &mut tx,
                "release_federation_allocation",
                &request.federation_id,
                ManualOperationStatus::Accepted,
                None,
                None,
                Some(&detail),
            )
            .await?;
            tx.commit().await.map_err(internal_error)?;

            tracing::info!(
                federation_id = %request.federation_id.0,
                previous_requester = %previous_requester.0,
                reason,
                "operator released a federation allocation binding",
            );
            Ok(ReleaseFederationAllocationResponse {
                status: ManualOperationStatus::Accepted,
                previous_requester: Some(previous_requester),
                detail: Some(detail),
            })
        }
    }
}

/// Records a refused release and returns it, so a rejection is as auditable as
/// an acceptance.
async fn release_audited(
    database: &Database,
    request: &ReleaseFederationAllocationRequest,
    status: ManualOperationStatus,
    previous_requester: Option<Pubkey>,
    detail: &str,
) -> ServiceResult<ReleaseFederationAllocationResponse> {
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    insert_audit_tx(
        &mut tx,
        "release_federation_allocation",
        &request.federation_id,
        status,
        None,
        None,
        Some(detail),
    )
    .await?;
    tx.commit().await.map_err(internal_error)?;
    Ok(ReleaseFederationAllocationResponse {
        status,
        previous_requester,
        detail: Some(detail.to_owned()),
    })
}

pub(crate) async fn resolve_manual_review(
    context: &DaemonContext,
    request: ResolveManualReviewRequest,
) -> ServiceResult<ResolveManualReviewResponse> {
    if let Some(detail) = completed_resolution_lacks_chain_evidence(context, &request).await {
        return Ok(manual_review_response(
            ManualOperationStatus::Rejected,
            None,
            detail,
        ));
    }
    resolve_manual_review_with_database(&context.database, request).await
}

/// Requires exact-output chain evidence for a `completed` resolution, and
/// refuses every case where FLIP cannot obtain it.
///
/// A reviewed operation must not reach `completed` unless FLIP has evidence of
/// its exact destination and amount, so evidence is required and its absence
/// refuses. Refusing only a *visible contradiction* would be weaker: an
/// unreachable observer, a missing persisted address, or a txid the observer
/// does not know would each complete the operation with no evidence at all.
///
/// **The operator is not left without a route.** Those three cases are the ones
/// an operator legitimately meets, and a chain-observer outage is close to the
/// situation that produces reviewed operations. `complete_review_without_evidence`
/// is the deliberate way through: it completes on the operator's assertion and
/// records that no evidence existed. The point of splitting them is that an
/// unverified completion cannot arrive through the verb that looks verified.
///
/// Looking the txid up is additive rather than a repeat of the automatic path:
/// `sync_chain_evidence` skips operations in `manual_review_required`, so FLIP
/// stops gathering evidence for a row once it enters review, and the operator's
/// txid is information it does not otherwise have.
async fn completed_resolution_lacks_chain_evidence(
    context: &DaemonContext,
    request: &ResolveManualReviewRequest,
) -> Option<String> {
    if request.resolution != ManualReviewResolution::Completed {
        return None;
    }
    // An absent or blank txid is refused downstream with a message about what a
    // `completed` resolution must carry, which is the more useful error.
    let txid = request.txid.as_deref()?.trim();
    if txid.is_empty() {
        return None;
    }

    let operation =
        match wallet::get_wallet_operation(&context.database, &request.operation_id).await {
            Ok(operation) => operation,
            // A missing operation is refused downstream as not found.
            Err(_) => return None,
        };
    let Some(address) = operation.address.as_deref() else {
        return Some(no_evidence_detail(
            "this operation records no destination address, so no transaction can be \
             checked against it",
        ));
    };

    let observer = match chain_observer_for(context).await {
        Some(observer) => observer,
        None => {
            return Some(no_evidence_detail(
                "the chain observer is not configured or its credential could not be read, \
                 so the transaction cannot be checked",
            ));
        }
    };

    chain_evidence_gap(&observer, txid, address, operation.amount).await
}

/// Builds the configured chain observer, or reports that it cannot be built.
async fn chain_observer_for(
    context: &DaemonContext,
) -> Option<crate::chain_observer::ConfiguredChainObserver> {
    let setup = crate::setup_store::load_setup_state(&context.database)
        .await
        .ok()?;
    let config = setup.config?;
    let password =
        crate::setup_store::load_bitcoind_password(&context.database, &context.secret_store)
            .await
            .ok()?;
    Some(crate::chain_observer::ConfiguredChainObserver::from_config(
        &config.chain_observer,
        password,
    ))
}

/// Names the override in every refusal, so an operator meeting one is told what
/// to do rather than only what failed.
fn no_evidence_detail(reason: &str) -> String {
    format!(
        "{reason}. A completed resolution requires chain evidence of this operation's exact \
         destination and amount. If you have established the outcome out of band, use \
         complete_review_without_evidence, which records that no evidence existed."
    )
}

/// The predicate itself, separated from loading the operation and building the
/// observer so a test can drive it against a chain that answers in each way.
///
/// Returns `None` only when the observer returned the transaction *and* one of
/// its outputs pays this operation's address for its amount. Every other answer —
/// an unreachable observer, an unknown transaction, or a transaction that pays
/// this operation nowhere — is a refusal, because what is required is evidence
/// rather than the absence of a contradiction.
async fn chain_evidence_gap(
    observer: &impl crate::chain_observer::ChainObserver,
    txid: &str,
    address: &str,
    amount: fedi_decentralized_service_liquidity_manager::Sats,
) -> Option<String> {
    let evidence = match observer.tx_evidence(txid).await {
        Ok(Some(evidence)) => evidence,
        Ok(None) => {
            return Some(no_evidence_detail(&format!(
                "the chain observer does not know transaction {txid}"
            )));
        }
        Err(error) => {
            return Some(no_evidence_detail(&format!(
                "the chain observer could not be reached to check transaction {txid}: {error}"
            )));
        }
    };

    let pays_this_operation = evidence
        .outputs
        .iter()
        .any(|output| output.address.as_deref() == Some(address) && output.amount_sats == amount.0);
    if pays_this_operation {
        return None;
    }

    Some(format!(
        "transaction {txid} is on chain and pays no output of {} sats to this operation's \
         address; resolving it as completed would record a settlement this transaction \
         does not contain",
        amount.0
    ))
}

/// Completes a reviewed wallet send on the operator's assertion alone, recording
/// that FLIP had no evidence for it.
///
/// **This exists because `resolve_manual_review` refuses what it cannot
/// verify.** The split avoids either extreme: requiring evidence with no way
/// through would make reviewed operations unresolvable during a chain-observer
/// outage, which is close to the situation that produces them; and letting
/// unverified assertions pass through the normal verb leaves nothing marking
/// them as unverified.
///
/// It is the same shape as `abandon_target_client_value`
/// (`target_recovery.rs`): FLIP does not deny a state it cannot prevent. It names
/// the state, requires a deliberate second call to reach it, and writes the
/// choice down in the same transaction that reaches it.
///
/// The txid is stored as the operator asserted it. `tx_vout` stays unset,
/// exactly as on the verified path, because chain observation owns exact output
/// attribution and this route explicitly has none.
pub(crate) async fn complete_review_without_evidence(
    database: &Database,
    request: CompleteReviewWithoutEvidenceRequest,
) -> ServiceResult<CompleteReviewWithoutEvidenceResponse> {
    let reason = request.reason.trim();
    if reason.is_empty() {
        return Ok(CompleteReviewWithoutEvidenceResponse {
            status: ManualOperationStatus::Rejected,
            detail: Some(
                "completing a reviewed send without evidence requires an operator reason"
                    .to_owned(),
            ),
        });
    }
    let txid = request.txid.trim();
    if txid.is_empty() {
        return Ok(CompleteReviewWithoutEvidenceResponse {
            status: ManualOperationStatus::Rejected,
            detail: Some(
                "completing a reviewed send requires the transaction the operator asserts \
                 settled it"
                    .to_owned(),
            ),
        });
    }

    let detail = format!(
        "operator completed this send without chain evidence, asserting transaction {txid}; \
         FLIP did not verify that it pays this operation. Reason: {reason}"
    );

    let mut tx = database.begin_write().await.map_err(internal_error)?;
    // Same shape as `resolve_manual_review`: an operation that does not exist is
    // `not_found`, and one that exists but is not under review is `rejected`.
    // Collapsing the two would tell an operator chasing a typo the same thing it
    // tells one whose operation somebody else already resolved.
    let Some(operation) = load_operation_tx(&mut tx, &request.operation_id).await? else {
        let detail = "wallet operation not found".to_owned();
        insert_audit_tx(
            &mut tx,
            "complete_review_without_evidence",
            &FederationId(String::new()),
            ManualOperationStatus::NotFound,
            None,
            Some(&request.operation_id),
            Some(&detail),
        )
        .await?;
        tx.commit().await.map_err(internal_error)?;
        return Ok(CompleteReviewWithoutEvidenceResponse {
            status: ManualOperationStatus::NotFound,
            detail: Some(detail),
        });
    };
    if operation.status != WalletOperationStatus::ManualReviewRequired {
        let detail = format!(
            "wallet operation {} is in state {} and is not under manual review",
            operation.operation_id, operation.status
        );
        insert_audit_tx(
            &mut tx,
            "complete_review_without_evidence",
            &FederationId(String::new()),
            ManualOperationStatus::Rejected,
            None,
            Some(&request.operation_id),
            Some(&detail),
        )
        .await?;
        tx.commit().await.map_err(internal_error)?;
        return Ok(CompleteReviewWithoutEvidenceResponse {
            status: ManualOperationStatus::Rejected,
            detail: Some(detail),
        });
    }

    // The same compare-and-set the verified path uses, on the same status. The
    // status read above is inside this `BEGIN IMMEDIATE` transaction, so this is
    // a fence rather than a second racy read: a concurrent resolution loses.
    let applied = wallet::resolve_manual_review_tx(
        &mut tx,
        &request.operation_id,
        &ManualReviewOutcome::Completed {
            txid: txid.to_owned(),
        },
    )
    .await?;
    if !applied {
        let detail =
            "wallet operation left manual review before the completion was applied".to_owned();
        insert_audit_tx(
            &mut tx,
            "complete_review_without_evidence",
            &FederationId(String::new()),
            ManualOperationStatus::Rejected,
            None,
            Some(&request.operation_id),
            Some(&detail),
        )
        .await?;
        tx.commit().await.map_err(internal_error)?;
        return Ok(CompleteReviewWithoutEvidenceResponse {
            status: ManualOperationStatus::Rejected,
            detail: Some(detail),
        });
    }
    // In the same transaction as the completion, so an unverified settlement
    // cannot be recorded without the record of why it was unverified.
    insert_audit_tx(
        &mut tx,
        "complete_review_without_evidence",
        &FederationId(String::new()),
        ManualOperationStatus::Accepted,
        None,
        Some(&request.operation_id),
        Some(&detail),
    )
    .await?;
    tx.commit().await.map_err(internal_error)?;

    tracing::warn!(
        operation_id = %request.operation_id.0,
        %txid,
        "manual review completed without chain evidence"
    );

    Ok(CompleteReviewWithoutEvidenceResponse {
        status: ManualOperationStatus::Accepted,
        detail: Some(detail),
    })
}

async fn resolve_manual_review_with_database(
    database: &Database,
    request: ResolveManualReviewRequest,
) -> ServiceResult<ResolveManualReviewResponse> {
    // A txid is the whole content of a `completed` resolution and is
    // meaningless on the two that assert no send happened, so a request
    // carrying the wrong combination is a mistake about what is being claimed,
    // not something to interpret.
    let outcome = match (request.resolution, request.txid.as_deref()) {
        (ManualReviewResolution::Completed, Some(txid)) if !txid.trim().is_empty() => {
            ManualReviewOutcome::Completed {
                txid: txid.to_owned(),
            }
        }
        (ManualReviewResolution::Completed, _) => {
            return Ok(manual_review_response(
                ManualOperationStatus::Rejected,
                None,
                "resolving an operation as completed requires the settling txid",
            ));
        }
        (_, Some(_)) => {
            return Ok(manual_review_response(
                ManualOperationStatus::Rejected,
                None,
                "a txid may only accompany the completed resolution",
            ));
        }
        (ManualReviewResolution::Failed, None) => ManualReviewOutcome::Failed {
            reason: request
                .reason
                .clone()
                .unwrap_or_else(|| "operator resolved manual review as failed".to_owned()),
        },
        (ManualReviewResolution::SafeToRetry, None) => ManualReviewOutcome::SafeToRetry,
    };

    let mut tx = database.begin_write().await.map_err(internal_error)?;
    let Some(operation) = load_operation_tx(&mut tx, &request.operation_id).await? else {
        insert_manual_review_audit_tx(&mut tx, &request, ManualOperationStatus::NotFound, None)
            .await?;
        tx.commit().await.map_err(internal_error)?;
        return Ok(manual_review_response(
            ManualOperationStatus::NotFound,
            None,
            "wallet operation not found",
        ));
    };

    if operation.status != WalletOperationStatus::ManualReviewRequired {
        let detail = format!(
            "wallet operation {} is in state {} and is not under manual review",
            operation.operation_id, operation.status
        );
        let status = if operation.status == WalletOperationStatus::InDoubt {
            ManualOperationStatus::Rejected
        } else {
            // Already resolved, by an operator or by evidence that arrived
            // first. Reporting this apart from a rejection lets a retried
            // request tell "someone got there first" from "you may not do this".
            ManualOperationStatus::AlreadyApplied
        };
        insert_manual_review_audit_tx(&mut tx, &request, status, Some(&detail)).await?;
        tx.commit().await.map_err(internal_error)?;
        return Ok(manual_review_response(status, None, detail));
    }

    let applied =
        wallet::resolve_manual_review_tx(&mut tx, &request.operation_id, &outcome).await?;
    if !applied {
        insert_manual_review_audit_tx(
            &mut tx,
            &request,
            ManualOperationStatus::AlreadyApplied,
            Some("wallet operation left manual review before the resolution was applied"),
        )
        .await?;
        tx.commit().await.map_err(internal_error)?;
        return Ok(manual_review_response(
            ManualOperationStatus::AlreadyApplied,
            None,
            "wallet operation left manual review before the resolution was applied",
        ));
    }

    insert_manual_review_audit_tx(&mut tx, &request, ManualOperationStatus::Accepted, None).await?;
    tx.commit().await.map_err(internal_error)?;

    let operation = wallet::get_wallet_operation(database, &request.operation_id).await?;
    Ok(ResolveManualReviewResponse {
        status: ManualOperationStatus::Accepted,
        operation: Some(operation),
        detail: Some(format!("manual review resolved as {}", request.resolution)),
    })
}

/// Exposes the real manual-resolution transaction to an in-crate focused test.
#[cfg(test)]
pub(crate) async fn resolve_manual_review_with_database_for_test(
    database: &Database,
    request: ResolveManualReviewRequest,
) -> ServiceResult<ResolveManualReviewResponse> {
    resolve_manual_review_with_database(database, request).await
}

async fn retry_funding_step_with_database(
    database: &Database,
    request: RetryFundingStepRequest,
) -> ServiceResult<RetryFundingStepResponse> {
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    if !allocation_exists_tx(&mut tx, &request.federation_id).await? {
        insert_audit_tx(
            &mut tx,
            "retry_funding_step",
            &request.federation_id,
            ManualOperationStatus::NotFound,
            request.item_id.as_ref(),
            request.operation_id.as_ref(),
            None,
        )
        .await?;
        tx.commit().await.map_err(internal_error)?;
        return Ok(manual_retry_response(
            ManualOperationStatus::NotFound,
            "allocation not found",
        ));
    }

    let operation = match &request.operation_id {
        Some(operation_id) => match load_operation_tx(&mut tx, operation_id).await? {
            Some(operation) => Some(operation),
            None => {
                insert_audit_tx(
                    &mut tx,
                    "retry_funding_step",
                    &request.federation_id,
                    ManualOperationStatus::NotFound,
                    request.item_id.as_ref(),
                    request.operation_id.as_ref(),
                    Some("wallet operation not found"),
                )
                .await?;
                tx.commit().await.map_err(internal_error)?;
                return Ok(manual_retry_response(
                    ManualOperationStatus::NotFound,
                    "wallet operation not found",
                ));
            }
        },
        None => None,
    };
    if let Some(operation) = &operation {
        if operation.federation_id.as_ref() != Some(&request.federation_id.0) {
            insert_audit_tx(
                &mut tx,
                "retry_funding_step",
                &request.federation_id,
                ManualOperationStatus::NotFound,
                request.item_id.as_ref(),
                request.operation_id.as_ref(),
                Some("wallet operation does not belong to the allocation"),
            )
            .await?;
            tx.commit().await.map_err(internal_error)?;
            return Ok(manual_retry_response(
                ManualOperationStatus::NotFound,
                "wallet operation does not belong to the allocation",
            ));
        }
        if operation.item_id.is_none() {
            insert_audit_tx(
                &mut tx,
                "retry_funding_step",
                &request.federation_id,
                ManualOperationStatus::Rejected,
                request.item_id.as_ref(),
                request.operation_id.as_ref(),
                Some("wallet operation is not attached to an allocation item"),
            )
            .await?;
            tx.commit().await.map_err(internal_error)?;
            return Ok(manual_retry_response(
                ManualOperationStatus::Rejected,
                "wallet operation is not attached to an allocation item",
            ));
        }
        if let Some(request_item_id) = &request.item_id
            && operation.item_id.as_ref() != Some(&request_item_id.0)
        {
            insert_audit_tx(
                &mut tx,
                "retry_funding_step",
                &request.federation_id,
                ManualOperationStatus::NotFound,
                request.item_id.as_ref(),
                request.operation_id.as_ref(),
                Some("wallet operation does not belong to requested item"),
            )
            .await?;
            tx.commit().await.map_err(internal_error)?;
            return Ok(manual_retry_response(
                ManualOperationStatus::NotFound,
                "wallet operation does not belong to requested item",
            ));
        }
    }

    let item_selector = operation
        .as_ref()
        .and_then(|operation| operation.item_id.clone().map(ItemId))
        .or(request.item_id.clone());
    let items = load_items_tx(&mut tx, &request.federation_id, item_selector.as_ref()).await?;
    if items.is_empty() {
        insert_audit_tx(
            &mut tx,
            "retry_funding_step",
            &request.federation_id,
            ManualOperationStatus::NotFound,
            request.item_id.as_ref(),
            request.operation_id.as_ref(),
            Some("allocation item not found"),
        )
        .await?;
        tx.commit().await.map_err(internal_error)?;
        return Ok(manual_retry_response(
            ManualOperationStatus::NotFound,
            "allocation item not found",
        ));
    }

    let mut item_ids_to_retry = Vec::new();
    let mut wallet_ids_to_retry = Vec::new();
    for item in &items {
        if item.status == ItemAllocationStatus::Failed {
            let detail = "failed allocation items are terminal and cannot be retried";
            insert_audit_tx(
                &mut tx,
                "retry_funding_step",
                &request.federation_id,
                ManualOperationStatus::Rejected,
                Some(&item.item_id),
                request.operation_id.as_ref(),
                Some(detail),
            )
            .await?;
            tx.commit().await.map_err(internal_error)?;
            return Ok(manual_retry_response(
                ManualOperationStatus::Rejected,
                detail,
            ));
        }
        if is_terminal_item(item.status) {
            continue;
        }

        let operations = if let Some(operation) = &operation {
            vec![operation.clone()]
        } else {
            load_operations_for_item_tx(&mut tx, &item.item_id).await?
        };

        let item_retryable = item.status == ItemAllocationStatus::ActionRequired;
        let failed_wallets = operations
            .iter()
            .filter(|operation| operation.status == WalletOperationStatus::Failed)
            .collect::<Vec<_>>();
        if !item_retryable {
            continue;
        }

        for operation in &operations {
            if !retry_safe_wallet_status(operation) {
                let detail = format!(
                    "wallet operation {} is in state {} and cannot be retried",
                    operation.operation_id, operation.status
                );
                insert_audit_tx(
                    &mut tx,
                    "retry_funding_step",
                    &request.federation_id,
                    ManualOperationStatus::Rejected,
                    Some(&item.item_id),
                    request.operation_id.as_ref(),
                    Some(&detail),
                )
                .await?;
                tx.commit().await.map_err(internal_error)?;
                return Ok(manual_retry_response(
                    ManualOperationStatus::Rejected,
                    detail,
                ));
            }
        }

        item_ids_to_retry.push(item.item_id.clone());
        wallet_ids_to_retry.extend(
            failed_wallets
                .into_iter()
                .map(|operation| WalletOperationId(operation.operation_id.clone())),
        );
    }

    if item_ids_to_retry.is_empty() && wallet_ids_to_retry.is_empty() {
        insert_audit_tx(
            &mut tx,
            "retry_funding_step",
            &request.federation_id,
            ManualOperationStatus::AlreadyApplied,
            request.item_id.as_ref(),
            request.operation_id.as_ref(),
            Some("no action-required safe funding step matched the request"),
        )
        .await?;
        tx.commit().await.map_err(internal_error)?;
        return Ok(manual_retry_response(
            ManualOperationStatus::AlreadyApplied,
            "no action-required safe funding step matched the request",
        ));
    }

    for operation_id in &wallet_ids_to_retry {
        wallet::reset_wallet_operation_tx(&mut tx, operation_id).await?;
    }
    for item_id in &item_ids_to_retry {
        allocation_store::reset_item_tx(&mut tx, &request.federation_id, item_id).await?;
    }
    insert_audit_tx(
        &mut tx,
        "retry_funding_step",
        &request.federation_id,
        ManualOperationStatus::Accepted,
        request.item_id.as_ref(),
        request.operation_id.as_ref(),
        None,
    )
    .await?;
    tx.commit().await.map_err(internal_error)?;

    Ok(manual_retry_response(
        ManualOperationStatus::Accepted,
        "safe funding step was requeued",
    ))
}

pub(crate) async fn cancel_allocation_with_database(
    database: &Database,
    request: CancelAllocationRequest,
) -> ServiceResult<CancelAllocationResponse> {
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    if !allocation_exists_tx(&mut tx, &request.federation_id).await? {
        insert_audit_tx(
            &mut tx,
            "cancel_allocation",
            &request.federation_id,
            ManualOperationStatus::NotFound,
            None,
            None,
            request.reason.as_deref(),
        )
        .await?;
        tx.commit().await.map_err(internal_error)?;
        return Ok(CancelAllocationResponse {
            status: ManualOperationStatus::NotFound,
            allocation_status: None,
            detail: Some("allocation not found".to_owned()),
        });
    }

    let items = load_items_tx(&mut tx, &request.federation_id, None).await?;
    let mut active_item_ids = Vec::new();
    let mut wallet_ids_to_cancel = Vec::new();
    let mut already_cancelled = !items.is_empty();
    for item in &items {
        already_cancelled &= item.status == ItemAllocationStatus::Cancelled;
        if !matches!(
            item.status,
            ItemAllocationStatus::Pending
                | ItemAllocationStatus::Running
                | ItemAllocationStatus::ActionRequired
        ) {
            continue;
        }

        let operations = load_operations_for_item_tx(&mut tx, &item.item_id).await?;
        for operation in &operations {
            if !cancel_safe_wallet_status(operation) {
                let detail = format!(
                    "wallet operation {} is in state {} and cannot be cancelled",
                    operation.operation_id, operation.status
                );
                insert_audit_tx(
                    &mut tx,
                    "cancel_allocation",
                    &request.federation_id,
                    ManualOperationStatus::Rejected,
                    Some(&item.item_id),
                    Some(&WalletOperationId(operation.operation_id.clone())),
                    Some(&detail),
                )
                .await?;
                tx.commit().await.map_err(internal_error)?;
                let status = load_allocation_status(database, &request.federation_id).await?;
                return Ok(CancelAllocationResponse {
                    status: ManualOperationStatus::Rejected,
                    allocation_status: status,
                    detail: Some(detail),
                });
            }
            if matches!(
                operation.status,
                WalletOperationStatus::Pending | WalletOperationStatus::Failed
            ) {
                wallet_ids_to_cancel.push(WalletOperationId(operation.operation_id.clone()));
            }
        }
        active_item_ids.push(item.item_id.clone());
    }

    if active_item_ids.is_empty() {
        let outcome = if already_cancelled {
            ManualOperationStatus::AlreadyApplied
        } else {
            ManualOperationStatus::Rejected
        };
        let detail = if already_cancelled {
            "allocation is already cancelled"
        } else {
            "allocation has no cancellable active items"
        };
        insert_audit_tx(
            &mut tx,
            "cancel_allocation",
            &request.federation_id,
            outcome,
            None,
            None,
            Some(detail),
        )
        .await?;
        tx.commit().await.map_err(internal_error)?;
        let status = load_allocation_status(database, &request.federation_id).await?;
        return Ok(CancelAllocationResponse {
            status: outcome,
            allocation_status: status,
            detail: Some(detail.to_owned()),
        });
    }

    let cancel_reason = request
        .reason
        .as_deref()
        .unwrap_or("operator cancelled funding work");
    for operation_id in &wallet_ids_to_cancel {
        wallet::cancel_wallet_operation_tx(&mut tx, operation_id, cancel_reason).await?;
    }
    for item_id in &active_item_ids {
        allocation_store::cancel_item_tx(&mut tx, &request.federation_id, item_id).await?;
    }
    insert_audit_tx(
        &mut tx,
        "cancel_allocation",
        &request.federation_id,
        ManualOperationStatus::Accepted,
        None,
        None,
        request.reason.as_deref(),
    )
    .await?;
    tx.commit().await.map_err(internal_error)?;

    let status = load_allocation_status(database, &request.federation_id).await?;
    Ok(CancelAllocationResponse {
        status: ManualOperationStatus::Accepted,
        allocation_status: status,
        detail: Some("allocation cancellation was applied".to_owned()),
    })
}

#[derive(Clone, Debug)]
struct ItemRecord {
    item_id: ItemId,
    status: ItemAllocationStatus,
}

#[derive(Clone, Debug)]
struct WalletOperationRecord {
    operation_id: String,
    federation_id: Option<String>,
    item_id: Option<String>,
    status: WalletOperationStatus,
    txid: Option<String>,
}

async fn allocation_exists_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    federation_id: &FederationId,
) -> ServiceResult<bool> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM allocations WHERE federation_id = ?")
        .bind(&federation_id.0)
        .fetch_one(&mut **tx)
        .await
        .map_err(internal_error)?;
    Ok(count > 0)
}

async fn load_items_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    federation_id: &FederationId,
    item_id: Option<&ItemId>,
) -> ServiceResult<Vec<ItemRecord>> {
    let rows = if let Some(item_id) = item_id {
        sqlx::query(
            "SELECT item_id, status FROM allocation_items \
             WHERE federation_id = ? AND item_id = ? \
             ORDER BY item_id ASC",
        )
        .bind(&federation_id.0)
        .bind(&item_id.0)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal_error)?
    } else {
        sqlx::query(
            "SELECT item_id, status FROM allocation_items \
             WHERE federation_id = ? ORDER BY item_id ASC",
        )
        .bind(&federation_id.0)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal_error)?
    };

    rows.into_iter().map(item_from_row).collect()
}

async fn load_operation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    operation_id: &WalletOperationId,
) -> ServiceResult<Option<WalletOperationRecord>> {
    let Some(row) = sqlx::query(
        "SELECT operation_id, federation_id, item_id, status, txid \
         FROM wallet_operations WHERE operation_id = ?",
    )
    .bind(&operation_id.0)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_error)?
    else {
        return Ok(None);
    };
    operation_from_row(row).map(Some)
}

async fn load_operations_for_item_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    item_id: &ItemId,
) -> ServiceResult<Vec<WalletOperationRecord>> {
    let rows = sqlx::query(
        "SELECT operation_id, federation_id, item_id, status, txid \
         FROM wallet_operations WHERE item_id = ? ORDER BY created_at ASC, operation_id ASC",
    )
    .bind(&item_id.0)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal_error)?;
    rows.into_iter().map(operation_from_row).collect()
}

fn item_from_row(row: SqliteRow) -> ServiceResult<ItemRecord> {
    let status: String = row.get("status");
    Ok(ItemRecord {
        item_id: ItemId(row.get("item_id")),
        status: status
            .parse()
            .map_err(|_| internal_error(format!("unknown allocation item status {status:?}")))?,
    })
}

fn operation_from_row(row: SqliteRow) -> ServiceResult<WalletOperationRecord> {
    let status: String = row.get("status");
    Ok(WalletOperationRecord {
        operation_id: row.get("operation_id"),
        federation_id: row.get("federation_id"),
        item_id: row.get("item_id"),
        status: status
            .parse()
            .map_err(|_| internal_error(format!("unknown wallet operation status {status:?}")))?,
        txid: row.get("txid"),
    })
}

fn is_terminal_item(status: ItemAllocationStatus) -> bool {
    matches!(
        status,
        ItemAllocationStatus::Completed
            | ItemAllocationStatus::Failed
            | ItemAllocationStatus::Cancelled
    )
}

fn retry_safe_wallet_status(operation: &WalletOperationRecord) -> bool {
    match operation.status {
        WalletOperationStatus::Pending | WalletOperationStatus::Failed => operation.txid.is_none(),
        _ => false,
    }
}

fn cancel_safe_wallet_status(operation: &WalletOperationRecord) -> bool {
    matches!(
        operation.status,
        WalletOperationStatus::Pending
            | WalletOperationStatus::Failed
            | WalletOperationStatus::Cancelled
    )
}

async fn insert_audit_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    action: &str,
    federation_id: &FederationId,
    outcome: ManualOperationStatus,
    item_id: Option<&ItemId>,
    operation_id: Option<&WalletOperationId>,
    detail: Option<&str>,
) -> ServiceResult<()> {
    let detail_json = serde_json::json!({
        "federation_id": federation_id,
        "outcome": outcome.to_string(),
        "item_id": item_id,
        "operation_id": operation_id,
        "detail": detail,
    });
    sqlx::query(
        "INSERT INTO audit_log (action, detail_json, created_at) VALUES (?, ?, unixepoch())",
    )
    .bind(action)
    .bind(detail_json.to_string())
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;
    // Logged here rather than in each verb: this is the one place all of them
    // record a decision, so a verb added later cannot forget the line. A
    // refusal is warned about because it is the outcome an operator asks
    // questions about.
    let item = item_id.map(|id| id.0.as_str()).unwrap_or_default();
    let operation = operation_id.map(|id| id.0.as_str()).unwrap_or_default();
    let detail = detail.unwrap_or_default();
    match outcome {
        ManualOperationStatus::Rejected => tracing::warn!(
            action,
            federation_id = %federation_id.0,
            outcome = %outcome,
            item_id = item,
            operation_id = operation,
            detail,
            "operator action refused"
        ),
        _ => tracing::info!(
            action,
            federation_id = %federation_id.0,
            outcome = %outcome,
            item_id = item,
            operation_id = operation,
            detail,
            "operator action applied"
        ),
    }
    Ok(())
}

fn manual_retry_response(
    status: ManualOperationStatus,
    detail: impl Into<String>,
) -> RetryFundingStepResponse {
    RetryFundingStepResponse {
        status,
        detail: Some(detail.into()),
    }
}

fn manual_review_response(
    status: ManualOperationStatus,
    operation: Option<fedi_decentralized_service_liquidity_manager::WalletOperation>,
    detail: impl Into<String>,
) -> ResolveManualReviewResponse {
    ResolveManualReviewResponse {
        status,
        operation,
        detail: Some(detail.into()),
    }
}

async fn insert_manual_review_audit_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    request: &ResolveManualReviewRequest,
    outcome: ManualOperationStatus,
    detail: Option<&str>,
) -> ServiceResult<()> {
    let detail_json = serde_json::json!({
        "operation_id": request.operation_id,
        "resolution": request.resolution.to_string(),
        "txid": request.txid,
        "reason": request.reason,
        "outcome": outcome.to_string(),
        "detail": detail,
    });
    sqlx::query(
        "INSERT INTO audit_log (action, detail_json, created_at) VALUES (?, ?, unixepoch())",
    )
    .bind("resolve_manual_review")
    .bind(detail_json.to_string())
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;
    // A review conclusion is an operator's assertion rather than something
    // FLIP observed, so the log carries who concluded what, in their words.
    let reason = request.reason.as_deref().unwrap_or_default();
    let txid = request.txid.as_deref().unwrap_or_default();
    match outcome {
        ManualOperationStatus::Rejected => tracing::warn!(
            operation_id = %request.operation_id.0,
            resolution = %request.resolution,
            outcome = %outcome,
            txid,
            reason,
            "manual review resolution refused"
        ),
        _ => tracing::info!(
            operation_id = %request.operation_id.0,
            resolution = %request.resolution,
            outcome = %outcome,
            txid,
            reason,
            "manual review resolved by the operator"
        ),
    }
    Ok(())
}

async fn load_allocation_status(
    database: &Database,
    federation_id: &FederationId,
) -> ServiceResult<Option<AllocationStatus>> {
    allocation_store::load_allocation_status_by_federation(database, federation_id).await
}

#[cfg(test)]
#[path = "../tests/manual_ops.rs"]
mod tests;
