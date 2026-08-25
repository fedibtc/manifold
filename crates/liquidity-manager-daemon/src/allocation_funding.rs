//! Shared provider-wallet funding steps for the allocation workers.
//!
//! The gateway and stability-pool workers fund their items the same way:
//! ensure one durable wallet operation exists for the item, then submit the
//! withdrawal with the in-doubt-before-send guard. Each worker describes its
//! source-specific parts with a [`FundingStep`].

use fedi_decentralized_service_liquidity_manager::{
    AdminFailure, FederationId, ItemAllocationStatus, ItemId, LiquidityFailureCode, Sats,
    ServiceResult, SetupConfigView, WalletOperation, WalletOperationId, WalletOperationStatus,
    WalletOperationType,
};

use crate::allocation_store;
use crate::database::Database;
use crate::funds_admin::default_fee_rate_sat_per_vbyte;
use crate::internal_error;
use crate::wallet::{FundsWallet, SubmitWithdrawalError};
use crate::wallet::{
    WalletOperationInput, get_wallet_operation, insert_wallet_operation_tx, mark_operation_failed,
    mark_operation_in_doubt, mark_withdrawal_broadcast, wallet_operation_for_item_tx,
};

/// Which worker is funding an item.
///
/// Everything about a funding step that is not the item itself follows from
/// this: the persisted operation type, the durable operation-id prefix, and
/// the operator-visible strings. They are derived here rather than restated
/// at each call site because they must agree and the disagreement would
/// outlive the pass that wrote it: the prefix becomes part of the persisted
/// `WalletOperationId`, while the lookup that decides whether an operation
/// already exists keys on the type. A step that named one worker in its id
/// and another in its type would persist an operation id that lies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FundingKind {
    Gateway,
    StabilityPool,
}

impl FundingKind {
    /// Wallet operation type persisted for this funding step.
    fn operation_type(self) -> WalletOperationType {
        match self {
            Self::Gateway => WalletOperationType::GatewayFunding,
            Self::StabilityPool => WalletOperationType::StabilityPoolFunding,
        }
    }

    /// Stable local operation-id prefix. Durable: it is the leading component
    /// of the persisted `WalletOperationId`, so changing one renames existing
    /// operations out of the namespace their worker looks in.
    fn operation_id_prefix(self) -> &'static str {
        match self {
            Self::Gateway => "wallet-gateway-funding",
            Self::StabilityPool => "wallet-stability-pool-funding",
        }
    }

    /// Operator-visible label prefix.
    fn label_prefix(self) -> &'static str {
        match self {
            Self::Gateway => "gateway funding",
            Self::StabilityPool => "stability-pool funding",
        }
    }

    /// Error message used when the destination address is missing. The two
    /// workers send to different kinds of address, so this names the missing
    /// one rather than deriving from the label.
    fn missing_address_message(self) -> &'static str {
        match self {
            Self::Gateway => "missing gateway deposit address",
            Self::StabilityPool => "missing stability-pool peg-in address",
        }
    }

    /// Detail recorded when the operation is marked in doubt before the send.
    fn in_doubt_detail(self) -> String {
        format!(
            "{} submission started; gatewayd response not yet recorded",
            self.label_prefix()
        )
    }
}

/// One item's provider-wallet funding step.
pub(crate) struct FundingStep<'a> {
    /// Which worker is funding, and with it every source-specific constant.
    pub kind: FundingKind,

    pub federation_id: &'a FederationId,
    pub item_id: &'a ItemId,

    /// Destination address persisted in the item step, when already known.
    pub address: Option<&'a str>,

    /// Amount withdrawn from the provider wallet.
    pub amount: Sats,
}

/// Returns the item's durable funding wallet operation, creating it first
/// when missing. The destination address must already be persisted in the
/// item step before a new operation is created.
pub(crate) async fn ensure_wallet_operation(
    database: &Database,
    setup: &SetupConfigView,
    step: &FundingStep<'_>,
) -> ServiceResult<Option<WalletOperation>> {
    let address = step
        .address
        .ok_or_else(|| internal_error(step.kind.missing_address_message()))?;
    let operation_id = WalletOperationId(format!(
        "{}-{}",
        step.kind.operation_id_prefix(),
        step.item_id.0.replace(':', "-")
    ));
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM allocation_items \
         WHERE item_id = ? AND status IN (?, ?))",
    )
    .bind(&step.item_id.0)
    .bind(ItemAllocationStatus::Pending.to_string())
    .bind(ItemAllocationStatus::Running.to_string())
    .fetch_one(&mut *tx)
    .await
    .map_err(internal_error)?;
    if !active {
        tx.commit().await.map_err(internal_error)?;
        return Ok(None);
    }
    if let Some(operation) =
        wallet_operation_for_item_tx(&mut tx, step.kind.operation_type(), step.item_id).await?
    {
        tx.commit().await.map_err(internal_error)?;
        return Ok(Some(operation));
    }
    insert_wallet_operation_tx(
        &mut tx,
        &WalletOperationInput {
            operation_id: operation_id.clone(),
            operation_type: step.kind.operation_type(),
            status: WalletOperationStatus::Pending,
            amount: step.amount,
            address: Some(address.to_owned()),
            label: Some(format!(
                "{} {}",
                step.kind.label_prefix(),
                step.federation_id.0
            )),
            fee_rate_sat_per_vbyte: Some(default_fee_rate_sat_per_vbyte(setup.network)),
            federation_id: Some(step.federation_id.clone()),
            item_id: Some(step.item_id.clone()),
        },
    )
    .await?;
    tx.commit().await.map_err(internal_error)?;
    tracing::info!(
        federation_id = %step.federation_id.0,
        item_id = %step.item_id.0,
        operation_id = %operation_id.0,
        amount_sats = step.amount.0,
        destination = %address,
        "created the funding wallet operation"
    );
    Ok(Some(get_wallet_operation(database, &operation_id).await?))
}

/// Submits the pending funding withdrawal for one item: persist `in_doubt`
/// before the irreversible gatewayd send, record the txid on success, and
/// fail the item on a clean submission failure.
pub(crate) async fn submit_funding_withdrawal(
    database: &Database,
    setup: &SetupConfigView,
    wallet: &impl FundsWallet,
    step: &FundingStep<'_>,
    operation_id: &WalletOperationId,
) -> ServiceResult<()> {
    let address = step
        .address
        .ok_or_else(|| internal_error(step.kind.missing_address_message()))?;
    let prepared = match wallet
        .prepare_withdrawal(
            operation_id,
            address,
            step.amount,
            default_fee_rate_sat_per_vbyte(setup.network),
        )
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            tracing::warn!(
                federation_id = %step.federation_id.0,
                item_id = %step.item_id.0,
                operation_id = %operation_id.0,
                %error,
                "funding withdrawal could not be prepared; nothing was sent"
            );
            mark_operation_failed(database, operation_id, &error.to_string()).await?;
            fail_funding_item(database, step, error.to_string()).await?;
            return Ok(());
        }
    };

    // A lost fence means something else — a cancellation, or a terminal item —
    // committed first. Saying so is what distinguishes it from a send that
    // silently never happened.
    if !claim_funding_submission(database, step, operation_id).await? {
        tracing::info!(
            federation_id = %step.federation_id.0,
            item_id = %step.item_id.0,
            operation_id = %operation_id.0,
            "the item or its operation moved before the send fence; not submitting"
        );
        return Ok(());
    }
    tracing::info!(
        federation_id = %step.federation_id.0,
        item_id = %step.item_id.0,
        operation_id = %operation_id.0,
        amount_sats = step.amount.0,
        destination = %address,
        "submitting the funding withdrawal to gatewayd"
    );
    match wallet.submit_prepared_withdrawal(prepared).await {
        Ok(submitted) => {
            tracing::info!(
                federation_id = %step.federation_id.0,
                item_id = %step.item_id.0,
                operation_id = %operation_id.0,
                txid = %submitted.txid,
                "funding withdrawal broadcast"
            );
            if let Err(error) =
                mark_withdrawal_broadcast(database, operation_id, &submitted.txid).await
            {
                // Error, not the worker's usual warning: the payment is gone
                // and its txid is now only in the line above. The operation
                // stays `in_doubt`, so address-based chain evidence can still
                // settle it, but a person should reconcile rather than wait.
                tracing::error!(
                    federation_id = %step.federation_id.0,
                    item_id = %step.item_id.0,
                    operation_id = %operation_id.0,
                    txid = %submitted.txid,
                    %error,
                    "the funding withdrawal was broadcast but its txid could not be recorded"
                );
                return Err(error);
            }
        }
        Err(SubmitWithdrawalError::InDoubt(detail)) => {
            tracing::warn!(
                federation_id = %step.federation_id.0,
                item_id = %step.item_id.0,
                operation_id = %operation_id.0,
                %detail,
                "funding withdrawal outcome is in doubt; the send may have happened and is \
                 never resubmitted automatically"
            );
            mark_operation_in_doubt(database, operation_id, &detail).await?;
        }
        Err(SubmitWithdrawalError::Failed(detail)) => {
            tracing::warn!(
                federation_id = %step.federation_id.0,
                item_id = %step.item_id.0,
                operation_id = %operation_id.0,
                %detail,
                "funding withdrawal was refused before submission"
            );
            mark_operation_failed(database, operation_id, &detail).await?;
            fail_funding_item(database, step, detail).await?;
        }
    }
    Ok(())
}

async fn claim_funding_submission(
    database: &Database,
    step: &FundingStep<'_>,
    operation_id: &WalletOperationId,
) -> ServiceResult<bool> {
    let failure = AdminFailure {
        code: "in_doubt".to_owned(),
        message: step.kind.in_doubt_detail(),
        occurred_at: crate::now_timestamp(),
        federation_id: Some(step.federation_id.clone()),
        item_id: Some(step.item_id.clone()),
    };
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    let result = sqlx::query(
        "UPDATE wallet_operations \
         SET status = ?, failure_json = ?, submitted_at = COALESCE(submitted_at, unixepoch()), \
             updated_at = unixepoch() \
         WHERE operation_id = ? AND item_id = ? AND status = ? \
           AND EXISTS (SELECT 1 FROM allocation_items \
                       WHERE item_id = ? AND status IN (?, ?))",
    )
    .bind(WalletOperationStatus::InDoubt.to_string())
    .bind(serde_json::to_string(&failure).map_err(internal_error)?)
    .bind(&operation_id.0)
    .bind(&step.item_id.0)
    .bind(WalletOperationStatus::Pending.to_string())
    .bind(&step.item_id.0)
    .bind(ItemAllocationStatus::Pending.to_string())
    .bind(ItemAllocationStatus::Running.to_string())
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;
    tx.commit().await.map_err(internal_error)?;
    Ok(result.rows_affected() == 1)
}

async fn fail_funding_item(
    database: &Database,
    step: &FundingStep<'_>,
    reason: String,
) -> ServiceResult<()> {
    allocation_store::require_item_action(
        database,
        step.federation_id,
        step.item_id,
        LiquidityFailureCode::WithdrawFailed,
        reason,
    )
    .await
}
