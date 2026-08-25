//! Durable allocation and allocation-item state.
//!
//! An accepted allocation is the only durable record of a request outcome. This
//! module owns the rows the funding workers advance, the per-federation holding
//! that makes an allocation exclusive, and the item transitions the Admin API
//! reports.

use fedi_decentralized_service_liquidity_manager::{
    AdminAllocationDetail, AdminAllocationSummary, AllocationItemStatus, AllocationItemTarget,
    AllocationStatus, CompletionEvidence, FederationId, FederationName, GatewayId,
    GetAdminAllocationResponse, GetVerificationSummaryResponse, InviteCode, ItemAllocationStatus,
    ItemId, LiquidityFailure, LiquidityFailureCode, ListResponse, Pubkey, RequestLiquidityRequest,
    Sats, ServiceResult, Sha256Digest, SourceType, TimeRange, Timestamp, VerificationSummary,
};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row};

use crate::database::{Database, OffsetPage, push_in_list};
use crate::stability_deposit::{StabilityDepositOperationId, StabilityDepositSubmission};
use crate::wallet;
use crate::{internal_error, invalid_argument, not_found, to_i64_sats};

/// Item statuses under which an allocation item still holds work (and
/// reserves wallet funds). The `idx_allocation_items_active` partial index in
/// the baseline migration restates this list as SQL literals; a test pins the
/// two together, so adding a status here without the index fails CI rather
/// than silently narrowing the index.
pub(crate) const ACTIVE_ITEM_STATUSES: [ItemAllocationStatus; 2] =
    [ItemAllocationStatus::Pending, ItemAllocationStatus::Running];
/// The provider deposit's local progress vocabulary, in order.
///
/// One deposit runs `Submitting` -> `Initiated` -> `TxAccepted` -> `Success`
/// and never the other way. The order is a property of one operation id: a
/// different id is a different deposit and starts again.
///
/// Stored inside the item step's JSON as the string each variant displays as,
/// so the stored vocabulary is exactly what it was before this was a type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub(crate) enum SpDepositStatus {
    Submitting,
    Initiated,
    TxAccepted,
    Success,
    /// A status string this build does not know.
    ///
    /// Kept rather than refused, and it round-trips unchanged. It has no rank,
    /// so it can never order against a known status and wedge an item on a
    /// value a later build introduced or an earlier one left behind. Refusing
    /// it would make the whole step unreadable instead.
    Unknown(String),
}

impl SpDepositStatus {
    /// How far along the sequence this status is, or `None` for one this build
    /// does not know.
    fn rank(&self) -> Option<u8> {
        match self {
            Self::Submitting => Some(0),
            Self::Initiated => Some(1),
            Self::TxAccepted => Some(2),
            Self::Success => Some(3),
            Self::Unknown(_) => None,
        }
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Submitting => "submitting",
            Self::Initiated => "initiated",
            Self::TxAccepted => "tx_accepted",
            Self::Success => "success",
            Self::Unknown(value) => value,
        }
    }
}

impl std::fmt::Display for SpDepositStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<String> for SpDepositStatus {
    fn from(value: String) -> Self {
        match value.as_str() {
            "submitting" => Self::Submitting,
            "initiated" => Self::Initiated,
            "tx_accepted" => Self::TxAccepted,
            "success" => Self::Success,
            _ => Self::Unknown(value),
        }
    }
}

impl From<SpDepositStatus> for String {
    fn from(status: SpDepositStatus) -> Self {
        match status {
            SpDepositStatus::Unknown(value) => value,
            known => known.as_str().to_owned(),
        }
    }
}

/// The target peg-in's progress, as recorded in the item step.
///
/// A flattened view of [`crate::stability_pool::PegInStatus`], which the
/// backend reports with the amounts attached. Only the label is persisted; the
/// amount rides in `peg_in_amount` beside it.
///
/// Carries the same `Unknown` tolerance, for the same reason.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub(crate) enum PegInProgress {
    WaitingForTransaction,
    WaitingForConfirmation,
    Confirmed,
    Claimed,
    Unknown(String),
}

impl PegInProgress {
    fn as_str(&self) -> &str {
        match self {
            Self::WaitingForTransaction => "waiting_for_transaction",
            Self::WaitingForConfirmation => "waiting_for_confirmation",
            Self::Confirmed => "confirmed",
            Self::Claimed => "claimed",
            Self::Unknown(value) => value,
        }
    }
}

impl std::fmt::Display for PegInProgress {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<String> for PegInProgress {
    fn from(value: String) -> Self {
        match value.as_str() {
            "waiting_for_transaction" => Self::WaitingForTransaction,
            "waiting_for_confirmation" => Self::WaitingForConfirmation,
            "confirmed" => Self::Confirmed,
            "claimed" => Self::Claimed,
            _ => Self::Unknown(value),
        }
    }
}

impl From<PegInProgress> for String {
    fn from(status: PegInProgress) -> Self {
        match status {
            PegInProgress::Unknown(value) => value,
            known => known.as_str().to_owned(),
        }
    }
}

/// What a federation's allocation still holds. All zero is what "idle" means.
///
/// The three terms are the ones `SPEC-flip-rpc` names:
/// nothing reserving, nothing awaiting settlement, and no value delivered.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AllocationHolding {
    pub reserving_items: i64,
    pub pending_operations: i64,
    pub fulfilled_sats: i64,
}

impl AllocationHolding {
    pub(crate) fn is_idle(self) -> bool {
        self.reserving_items == 0 && self.pending_operations == 0 && self.fulfilled_sats == 0
    }
}

/// The result of asking for a federation's allocation to be released.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AllocationReleaseOutcome {
    /// The allocation held nothing and is gone, with the key it was bound to.
    Released { previous_requester: Pubkey },
    /// No allocation exists for this federation.
    NotFound,
    /// The allocation still holds something, so it stays. The holding says what.
    Held(AllocationHolding),
}

/// What this federation's allocation still holds, read inside a write
/// transaction so the answer cannot change before the caller acts on it.
///
/// **`allocations.committed_amount_sats` is deliberately not one of the terms.**
/// It is written once by `insert_allocation` from the plan's item amounts and no
/// production statement ever updates it, so every allocation carries a non-zero
/// value from the moment it exists. Reading "no committed value" off that column
/// would make every allocation permanently non-idle and this whole mechanism
/// dead code. `allocation_items.fulfilled_amount_sats` is what records value
/// actually delivered: `complete_item_tx` sets it and the retry path clears it.
pub(crate) async fn allocation_holding_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    federation_id: &FederationId,
) -> ServiceResult<AllocationHolding> {
    let mut builder =
        QueryBuilder::new("SELECT COUNT(*) FROM allocation_items WHERE federation_id = ");
    builder.push_bind(federation_id.0.clone());
    builder.push(" AND ");
    push_in_list(&mut builder, "status", &RESERVING_ITEM_STATUSES);
    let reserving_items: i64 = builder
        .build_query_scalar()
        .fetch_one(&mut **tx)
        .await
        .map_err(internal_error)?;

    // Both link columns are checked. An operation can be tied to the allocation
    // through `federation_id`, through `item_id`, or through both, and this
    // predicate must not depend on which.
    let mut builder =
        QueryBuilder::new("SELECT COUNT(*) FROM wallet_operations WHERE (federation_id = ");
    builder.push_bind(federation_id.0.clone());
    builder.push(" OR item_id IN (SELECT item_id FROM allocation_items WHERE federation_id = ");
    builder.push_bind(federation_id.0.clone());
    builder.push(")) AND ");
    push_in_list(
        &mut builder,
        "status",
        crate::wallet::PENDING_SETTLEMENT_STATUSES,
    );
    let pending_operations: i64 = builder
        .build_query_scalar()
        .fetch_one(&mut **tx)
        .await
        .map_err(internal_error)?;

    let fulfilled_sats: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(fulfilled_amount_sats), 0) FROM allocation_items \
         WHERE federation_id = ?",
    )
    .bind(&federation_id.0)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_error)?;

    Ok(AllocationHolding {
        reserving_items,
        pending_operations,
        fulfilled_sats,
    })
}

/// Deletes a federation's allocation and its items, but only while it holds
/// nothing.
///
/// This is the first production `DELETE FROM allocations` in the crate. Until
/// now the table had one production writer — `insert_allocation`'s
/// `INSERT OR IGNORE` — and no `UPDATE` or `DELETE`, which is what made the
/// first requester to present a valid endorsement the owner of that federation's
/// allocation forever. `SPEC-flip-rpc` removes the
/// permanence, and this is the operation both of its mechanisms use.
///
/// **The deletion does not move any capacity term**, which is why it is safe to
/// take on a live wallet. `active_reserved_amount_tx` sums `reserved_amount_sats`
/// over items in `RESERVING_ITEM_STATUSES` and `active_wallet_withdrawal_amount_tx`
/// excludes operations whose `item_id` is in that same set. An idle allocation
/// has no item in it, so its rows contribute nothing to either sum before the
/// delete and nothing after. `stamp_released_item_operations_tx` likewise stamps
/// operations whose item is *not* reserving, so a deleted item leaves its
/// operations eligible exactly as a terminal one did.
///
/// Items go first: `allocation_items.federation_id` is a declared foreign key and
/// `Database::connect` enables `foreign_keys`.
pub(crate) async fn release_allocation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    federation_id: &FederationId,
) -> ServiceResult<AllocationReleaseOutcome> {
    let previous_requester: Option<String> =
        sqlx::query_scalar("SELECT requester_pubkey FROM allocations WHERE federation_id = ?")
            .bind(&federation_id.0)
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal_error)?;
    let Some(previous_requester) = previous_requester else {
        return Ok(AllocationReleaseOutcome::NotFound);
    };

    let holding = allocation_holding_tx(tx, federation_id).await?;
    if !holding.is_idle() {
        return Ok(AllocationReleaseOutcome::Held(holding));
    }

    sqlx::query("DELETE FROM allocation_items WHERE federation_id = ?")
        .bind(&federation_id.0)
        .execute(&mut **tx)
        .await
        .map_err(internal_error)?;

    let deleted = sqlx::query("DELETE FROM allocations WHERE federation_id = ?")
        .bind(&federation_id.0)
        .execute(&mut **tx)
        .await
        .map_err(internal_error)?;
    if deleted.rows_affected() != 1 {
        return Err(internal_error(format!(
            "releasing the allocation for federation {} removed {} rows",
            federation_id.0,
            deleted.rows_affected()
        )));
    }

    Ok(AllocationReleaseOutcome::Released {
        previous_requester: Pubkey(previous_requester),
    })
}

/// Item statuses that continue reserving committed provider capacity.
pub(crate) const RESERVING_ITEM_STATUSES: [ItemAllocationStatus; 3] = [
    ItemAllocationStatus::Pending,
    ItemAllocationStatus::Running,
    ItemAllocationStatus::ActionRequired,
];

/// Allocation statuses under which the allocation still holds work. The
/// `idx_allocations_active` partial index in the baseline migration restates
/// this list as SQL literals.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct FundingTargetRecord {
    pub federation_id: FederationId,
    pub federation_name: FederationName,
    pub invite_code: InviteCode,
    /// Hex encoding of the federation config hash from the accepted request.
    pub federation_config_hash: String,
}

/// Derives an allocation item's id. Items have no identity of their own:
/// there is exactly one per (federation, source), enforced by
/// `idx_allocation_items_federation_source`.
pub(crate) fn item_id(federation_id: &FederationId, source_type: SourceType) -> ItemId {
    ItemId(format!("{}:{source_type}", federation_id.0))
}

/// One accepted allocation item, as planned by the public acceptance path.
pub(crate) struct PlannedItem {
    pub item_id: ItemId,
    pub source_type: SourceType,
    pub target: AllocationItemTarget,
    pub amount: Sats,
    pub reserved_amount: Sats,
}

/// Inserts the allocation and its items inside the caller's transaction. This
/// is the only production creator of `allocations`/`allocation_items` rows.
/// The primary key is bound from the [`FundingTargetRecord`] built here, so
/// the `federation_id` column and the one inside `target_json` cannot
/// disagree. Returns `false` without writing anything when another request
/// for the same federation (or the same details commitment) already inserted
/// one.
pub(crate) async fn insert_allocation(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    request: &RequestLiquidityRequest,
    request_json: &str,
    verification: &VerificationSummary,
    committed_amount: Sats,
    reserved_amount: Sats,
    items: &[PlannedItem],
) -> ServiceResult<bool> {
    let target = FundingTargetRecord {
        federation_id: request.federation_details.federation_id.clone(),
        federation_name: request.federation_details.federation_name.clone(),
        invite_code: request.federation_details.invite_code.clone(),
        federation_config_hash: hex::encode(&request.federation_details.federation_config_hash.0),
    };
    let verification_json = serde_json::to_string(verification).map_err(internal_error)?;
    let target_json = serde_json::to_string(&target).map_err(internal_error)?;

    let insert = sqlx::query(
        "INSERT OR IGNORE INTO allocations \
         (federation_id, requester_pubkey, provider_pubkey, network, details_payload_hash, \
          request_json, verification_json, target_json, \
          committed_amount_sats, reserved_amount_sats) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&target.federation_id.0)
    .bind(&request.requester_pubkey.0)
    .bind(&request.provider_pubkey.0)
    .bind(request.network.to_string())
    .bind(request.details_payload_hash.0.to_vec())
    .bind(request_json)
    .bind(verification_json)
    .bind(target_json)
    .bind(to_i64_sats(committed_amount)?)
    .bind(to_i64_sats(reserved_amount)?)
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;
    if insert.rows_affected() == 0 {
        return Ok(false);
    }

    for item in items {
        let item_json = serde_json::to_string(&item.target).map_err(internal_error)?;
        sqlx::query(
            "INSERT INTO allocation_items \
             (item_id, federation_id, source_type, status, committed_amount_sats, \
              reserved_amount_sats, item_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch())",
        )
        .bind(&item.item_id.0)
        .bind(&target.federation_id.0)
        .bind(item.source_type.to_string())
        .bind(ItemAllocationStatus::Pending.to_string())
        .bind(to_i64_sats(item.amount)?)
        .bind(to_i64_sats(item.reserved_amount)?)
        .bind(item_json)
        .execute(&mut **tx)
        .await
        .map_err(internal_error)?;
    }

    Ok(true)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GatewayAllocationItem {
    pub item_id: ItemId,
    pub federation_id: FederationId,
    pub target: FundingTargetRecord,
    pub item_target: AllocationItemTarget,
    pub committed_amount: Sats,
    pub reserved_amount: Sats,
    pub status: ItemAllocationStatus,
    pub step: GatewayAllocationStep,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StabilityPoolAllocationItem {
    pub item_id: ItemId,
    pub federation_id: FederationId,
    pub target: FundingTargetRecord,
    pub item_target: AllocationItemTarget,
    pub committed_amount: Sats,
    pub reserved_amount: Sats,
    pub status: ItemAllocationStatus,
    pub step: StabilityPoolAllocationStep,
    /// Raw unreadable step JSON retained for operator reconciliation.
    pub corrupt_step_json: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct GatewayAllocationStep {
    pub gateway_info_observed_at: Option<Timestamp>,
    pub initial_gateway_balance: Option<Sats>,
    pub gateway_connected: bool,
    pub deposit_address: Option<String>,
    pub wallet_operation_id: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct StabilityPoolAllocationStep {
    pub target_client_opened_at: Option<Timestamp>,
    pub peg_in_operation_id: Option<String>,
    pub peg_in_address: Option<String>,
    pub wallet_operation_id: Option<String>,
    pub peg_in_status: Option<PegInProgress>,
    pub peg_in_amount: Option<Sats>,
    pub sp_deposit_status: Option<SpDepositStatus>,
    pub sp_deposit_operation_id: Option<String>,
    pub sp_deposit_amount: Option<Sats>,
    pub sp_deposit_min_fee_rate_ppb: Option<u64>,
    pub observed_provided_amount: Option<Sats>,
}

impl StabilityPoolAllocationStep {
    /// Returns whether a provider deposit may already have been submitted.
    pub(crate) fn has_in_flight_submission(&self) -> bool {
        self.sp_deposit_status == Some(SpDepositStatus::Submitting)
    }

    /// Records how far the deposit for `operation_id` has got, refusing to move
    /// it backwards. Returns whether the write landed.
    ///
    /// Once a deposit has reached a terminal state, no writer may put it back to
    /// `initiated` or `tx_accepted`. Two writers would otherwise do exactly that.
    /// `observe_stability_deposit`'s `Initiated` and `TxAccepted` arms see a
    /// drain that replays from the start whenever
    /// `optimistically_set_operation_outcome` failed its write — which it only
    /// logs — so assigning unconditionally would overwrite a durable `success`.
    /// `target_recovery::bind_and_resume` writes the same field by direct SQL,
    /// for an operation id an operator supplies and may supply again.
    ///
    /// **Keyed on the operation id, not on the item.** Rebinding after a lost
    /// operation id has to be able to start a *new* deposit at `initiated`, and
    /// that is a different operation. The guard only refuses to walk one deposit
    /// backwards.
    ///
    /// `begin_submission` needs no such guard and deliberately does not carry
    /// one: `advance_stability_deposit_with` returns into observation whenever
    /// `sp_deposit_operation_id` is already set, so the submission path is only
    /// ever reached with no id to walk backwards.
    pub(crate) fn advance_sp_deposit_status(
        &mut self,
        operation_id: &str,
        status: SpDepositStatus,
    ) -> bool {
        let same_deposit = self.sp_deposit_operation_id.as_deref() == Some(operation_id);
        if same_deposit
            && let (Some(current), Some(next)) = (
                self.sp_deposit_status
                    .as_ref()
                    .and_then(SpDepositStatus::rank),
                status.rank(),
            )
            && next < current
        {
            return false;
        }
        self.sp_deposit_operation_id = Some(operation_id.to_owned());
        self.sp_deposit_status = Some(status);
        true
    }

    /// Atomically stages the provider operation ID and immutable request.
    pub(crate) fn begin_submission(&mut self, submission: StabilityDepositSubmission) {
        self.sp_deposit_status = Some(SpDepositStatus::Submitting);
        self.sp_deposit_operation_id = Some(submission.operation_id().to_string());
        self.sp_deposit_amount = Some(submission.amount());
        self.sp_deposit_min_fee_rate_ppb = Some(submission.min_fee_rate_ppb());
    }

    /// Rehydrates the in-flight ID and request without regenerating any member.
    pub(crate) fn submitting_submission(
        &self,
    ) -> Result<Option<StabilityDepositSubmission>, &'static str> {
        if !self.has_in_flight_submission() {
            return Ok(None);
        }
        match (
            self.sp_deposit_operation_id.clone(),
            self.sp_deposit_amount,
            self.sp_deposit_min_fee_rate_ppb,
        ) {
            (Some(operation_id), Some(amount), Some(min_fee_rate)) => {
                let operation_id = StabilityDepositOperationId::parse(&operation_id).map_err(
                    |_| "in-flight stability-pool submission has a malformed operation ID",
                )?;
                if amount.0 == 0 {
                    return Err("in-flight stability-pool submission has a zero amount");
                }
                Ok(Some(StabilityDepositSubmission::rehydrate(
                    operation_id,
                    amount,
                    min_fee_rate,
                )))
            }
            _ => Err("in-flight stability-pool submission has an incomplete persisted tuple"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct GatewayObservation {
    pub gateway_id: GatewayId,
    pub federation_id: Option<String>,
    pub status: String,
    pub observed_balance: Option<Sats>,
    pub observed_at: Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct StabilityPoolObservation {
    pub federation_id: String,
    pub status: String,
    pub observed_provided_amount: Option<Sats>,
    pub liquidity_stats_json: Option<String>,
    pub observed_at: Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ListAllocationsStoreRequest {
    pub page: fedi_decentralized_service_liquidity_manager::PageRequest,
    pub time_range: Option<TimeRange>,
}

/// Answers a requester's status poll. The details commitment identifies the
/// allocation; the requester filter is authorization, not identity: only the
/// requester whose signature accepted the allocation may read its status.
pub(crate) async fn load_allocation_status_for_poll(
    database: &Database,
    requester_pubkey: &Pubkey,
    details_payload_hash: Sha256Digest,
) -> ServiceResult<Option<AllocationStatus>> {
    let row = sqlx::query(
        "SELECT federation_id, provider_pubkey, details_payload_hash \
         FROM allocations \
         WHERE requester_pubkey = ? AND details_payload_hash = ?",
    )
    .bind(&requester_pubkey.0)
    .bind(details_payload_hash.0.to_vec())
    .fetch_optional(database.pool())
    .await
    .map_err(internal_error)?;
    match row {
        Some(row) => Ok(Some(allocation_status_from_row(database, &row).await?)),
        None => Ok(None),
    }
}

pub(crate) async fn load_allocation_status_by_federation(
    database: &Database,
    federation_id: &FederationId,
) -> ServiceResult<Option<AllocationStatus>> {
    let row = sqlx::query(
        "SELECT federation_id, provider_pubkey, details_payload_hash \
         FROM allocations \
         WHERE federation_id = ?",
    )
    .bind(&federation_id.0)
    .fetch_optional(database.pool())
    .await
    .map_err(internal_error)?;
    match row {
        Some(row) => Ok(Some(allocation_status_from_row(database, &row).await?)),
        None => Ok(None),
    }
}

async fn allocation_status_from_row(
    database: &Database,
    row: &SqliteRow,
) -> ServiceResult<AllocationStatus> {
    let federation_id: String = row.get("federation_id");
    let rows = sqlx::query(
        "SELECT status, item_json, fulfilled_amount_sats, completion_evidence_json, failure_json, updated_at \
         FROM allocation_items WHERE federation_id = ? ORDER BY item_id ASC",
    )
    .bind(&federation_id)
    .fetch_all(database.pool())
    .await
    .map_err(internal_error)?;

    let item_statuses = rows
        .into_iter()
        .map(allocation_item_status_from_row)
        .collect::<ServiceResult<Vec<_>>>()?;

    Ok(AllocationStatus {
        details_payload_hash: Sha256Digest::try_from(row.get::<Vec<u8>, _>("details_payload_hash"))
            .map_err(internal_error)?,
        provider_pubkey: Pubkey(row.get("provider_pubkey")),
        item_statuses,
    })
}

pub(crate) async fn active_gateway_items(
    database: &Database,
) -> ServiceResult<Vec<GatewayAllocationItem>> {
    let rows = active_item_rows(database, SourceType::Gateway).await?;
    rows.iter().map(gateway_item_from_row).collect()
}

pub(crate) async fn active_stability_pool_items(
    database: &Database,
) -> ServiceResult<Vec<StabilityPoolAllocationItem>> {
    let rows = active_item_rows(database, SourceType::StabilityPool).await?;
    rows.iter().map(stability_pool_item_from_row).collect()
}

/// Loads one federation's stability-pool item whatever status it is in.
///
/// The active-item query excludes `action_required` by design — the workers
/// must not pick such an item up — which is exactly the status the operator
/// reconciliation surfaces work on, so they need their own way in.
pub(crate) async fn stability_pool_item(
    database: &Database,
    federation_id: &FederationId,
) -> ServiceResult<Option<StabilityPoolAllocationItem>> {
    let row = sqlx::query(
        "SELECT i.item_id, i.status AS item_status, i.committed_amount_sats, \
                i.reserved_amount_sats, i.item_json, i.step_json, \
                a.federation_id, a.target_json \
         FROM allocation_items i \
         JOIN allocations a ON a.federation_id = i.federation_id \
         WHERE i.federation_id = ? AND i.source_type = ?",
    )
    .bind(&federation_id.0)
    .bind(SourceType::StabilityPool.to_string())
    .fetch_optional(database.pool())
    .await
    .map_err(internal_error)?;
    row.as_ref().map(stability_pool_item_from_row).transpose()
}

async fn active_item_rows(
    database: &Database,
    source_type: SourceType,
) -> ServiceResult<Vec<SqliteRow>> {
    let mut builder = QueryBuilder::new(
        "SELECT i.item_id, i.status AS item_status, i.committed_amount_sats, \
                i.reserved_amount_sats, i.item_json, i.step_json, \
                a.federation_id, a.target_json \
         FROM allocation_items i \
         JOIN allocations a ON a.federation_id = i.federation_id \
         WHERE i.source_type = ",
    );
    builder.push_bind(source_type.to_string());
    builder.push(" AND ");
    push_in_list(&mut builder, "i.status", &ACTIVE_ITEM_STATUSES);
    builder.push(" ORDER BY i.updated_at ASC, i.item_id ASC");
    builder
        .build()
        .fetch_all(database.pool())
        .await
        .map_err(internal_error)
}

pub(crate) async fn mark_item_running(
    database: &Database,
    _federation_id: &FederationId,
    item_id: &ItemId,
) -> ServiceResult<bool> {
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    let result = sqlx::query(
        "UPDATE allocation_items SET status = ?, updated_at = unixepoch() \
         WHERE item_id = ? AND status IN (?, ?)",
    )
    .bind(ItemAllocationStatus::Running.to_string())
    .bind(&item_id.0)
    .bind(ItemAllocationStatus::Pending.to_string())
    .bind(ItemAllocationStatus::Running.to_string())
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;
    tx.commit().await.map_err(internal_error)?;
    Ok(result.rows_affected() == 1)
}

/// Writes an already-serialised step. Same statement as [`update_item_step`],
/// split out so a caller can serialise before moving the write onto a task that
/// outlives its own cancellation.
pub(crate) async fn update_item_step_json(
    database: &Database,
    item_id: &ItemId,
    step_json: &str,
) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE allocation_items SET step_json = ?, updated_at = unixepoch() \
         WHERE item_id = ? AND status IN (?, ?)",
    )
    .bind(step_json)
    .bind(&item_id.0)
    .bind(ItemAllocationStatus::Pending.to_string())
    .bind(ItemAllocationStatus::Running.to_string())
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    Ok(())
}

pub(crate) async fn update_item_step<S: Serialize>(
    database: &Database,
    item_id: &ItemId,
    step: &S,
) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE allocation_items SET step_json = ?, updated_at = unixepoch() \
         WHERE item_id = ? AND status IN (?, ?)",
    )
    .bind(serde_json::to_string(step).map_err(internal_error)?)
    .bind(&item_id.0)
    .bind(ItemAllocationStatus::Pending.to_string())
    .bind(ItemAllocationStatus::Running.to_string())
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    Ok(())
}

pub(crate) async fn compare_and_set_item_step<S: Serialize>(
    database: &Database,
    item_id: &ItemId,
    expected: &S,
    next: &S,
) -> ServiceResult<bool> {
    let result = sqlx::query(
        "UPDATE allocation_items SET step_json = ?, updated_at = unixepoch() \
         WHERE item_id = ? AND status IN (?, ?) AND step_json = ?",
    )
    .bind(serde_json::to_string(next).map_err(internal_error)?)
    .bind(&item_id.0)
    .bind(ItemAllocationStatus::Pending.to_string())
    .bind(ItemAllocationStatus::Running.to_string())
    .bind(serde_json::to_string(expected).map_err(internal_error)?)
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    Ok(result.rows_affected() == 1)
}

pub(crate) async fn complete_item(
    database: &Database,
    federation_id: &FederationId,
    item_id: &ItemId,
    fulfilled_amount: Sats,
    evidence: CompletionEvidence,
) -> ServiceResult<()> {
    let completion_evidence_json = serde_json::to_string(&evidence).map_err(internal_error)?;
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    sqlx::query(
        "UPDATE allocation_items \
         SET status = ?, fulfilled_amount_sats = ?, completion_evidence_json = ?, updated_at = unixepoch() \
         WHERE item_id = ? AND status IN (?, ?)",
    )
    .bind(ItemAllocationStatus::Completed.to_string())
    .bind(to_i64_sats(fulfilled_amount)?)
    .bind(&completion_evidence_json)
    .bind(&item_id.0)
    .bind(ItemAllocationStatus::Pending.to_string())
    .bind(ItemAllocationStatus::Running.to_string())
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;
    tx.commit().await.map_err(internal_error)?;
    tracing::info!(
        federation_id = %federation_id.0,
        item_id = %item_id.0,
        fulfilled_sats = fulfilled_amount.0,
        "allocation item completed"
    );
    Ok(())
}

pub(crate) async fn fail_item(
    database: &Database,
    _federation_id: &FederationId,
    item_id: &ItemId,
    code: LiquidityFailureCode,
    reason: impl Into<String>,
) -> ServiceResult<()> {
    set_item_failure(
        database,
        item_id,
        ItemAllocationStatus::Failed,
        code,
        reason,
    )
    .await
}

pub(crate) async fn require_item_action(
    database: &Database,
    _federation_id: &FederationId,
    item_id: &ItemId,
    code: LiquidityFailureCode,
    reason: impl Into<String>,
) -> ServiceResult<()> {
    set_item_failure(
        database,
        item_id,
        ItemAllocationStatus::ActionRequired,
        code,
        reason,
    )
    .await
}

async fn set_item_failure(
    database: &Database,
    item_id: &ItemId,
    status: ItemAllocationStatus,
    code: LiquidityFailureCode,
    reason: impl Into<String>,
) -> ServiceResult<()> {
    let reason = reason.into();
    let failure = LiquidityFailure {
        code,
        reason: Some(reason.clone()),
    };
    let failure_json = serde_json::to_string(&failure).map_err(internal_error)?;
    let mut tx = database.begin_write().await.map_err(internal_error)?;
    let item_result = sqlx::query(
        "UPDATE allocation_items \
         SET status = ?, failure_json = ?, updated_at = unixepoch() \
         WHERE item_id = ? AND status IN (?, ?)",
    )
    .bind(status.to_string())
    .bind(&failure_json)
    .bind(&item_id.0)
    .bind(ItemAllocationStatus::Pending.to_string())
    .bind(ItemAllocationStatus::Running.to_string())
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;
    tx.commit().await.map_err(internal_error)?;
    // Only the pass that moved the item says so. A worker that keeps failing
    // calls this every pass, and the guard above stops all but the first from
    // changing anything, so an unconditional line here would repeat one event
    // for as long as the condition lasted.
    if item_result.rows_affected() == 1 {
        tracing::warn!(
            item_id = %item_id.0,
            %status,
            %code,
            %reason,
            "allocation item stopped"
        );
    }
    Ok(())
}

/// Resets an action-required item to `pending` and clears its outcome material
/// inside the caller's transaction. Used by admin manual operations.
pub(crate) async fn reset_item_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    _federation_id: &FederationId,
    item_id: &ItemId,
) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE allocation_items \
         SET status = ?, failure_json = NULL, fulfilled_amount_sats = NULL, \
             completion_evidence_json = NULL, updated_at = unixepoch() \
         WHERE item_id = ? AND status = ?",
    )
    .bind(ItemAllocationStatus::Pending.to_string())
    .bind(&item_id.0)
    .bind(ItemAllocationStatus::ActionRequired.to_string())
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;
    Ok(())
}

/// Marks an item `cancelled` inside the caller's transaction. Used by admin
/// manual operations.
pub(crate) async fn cancel_item_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    _federation_id: &FederationId,
    item_id: &ItemId,
) -> ServiceResult<()> {
    sqlx::query(
        "UPDATE allocation_items SET status = ?, updated_at = unixepoch() WHERE item_id = ?",
    )
    .bind(ItemAllocationStatus::Cancelled.to_string())
    .bind(&item_id.0)
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;
    Ok(())
}

pub(crate) async fn upsert_gateway_observation(
    database: &Database,
    observation: &GatewayObservation,
) -> ServiceResult<()> {
    let key = match &observation.federation_id {
        Some(federation_id) => format!(
            "gateway:{}:federation:{federation_id}",
            observation.gateway_id.0
        ),
        None => format!("gateway:{}", observation.gateway_id.0),
    };
    sqlx::query(
        "INSERT INTO gateway_observations \
         (observation_key, gateway_id, federation_id, status, observed_balance_sats, observation_json, observed_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(observation_key) DO UPDATE SET \
           gateway_id = excluded.gateway_id, federation_id = excluded.federation_id, \
           status = excluded.status, observed_balance_sats = excluded.observed_balance_sats, \
           observation_json = excluded.observation_json, observed_at = excluded.observed_at",
    )
    .bind(key)
    .bind(&observation.gateway_id.0)
    .bind(observation.federation_id.as_deref())
    .bind(&observation.status)
    .bind(observation.observed_balance.map(to_i64_sats).transpose()?)
    .bind(serde_json::to_string(observation).map_err(internal_error)?)
    .bind(timestamp_to_i64(observation.observed_at)?)
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    Ok(())
}

pub(crate) async fn upsert_stability_pool_observation(
    database: &Database,
    observation: &StabilityPoolObservation,
) -> ServiceResult<()> {
    let key = format!("stability_pool:federation:{}", observation.federation_id);
    sqlx::query(
        "INSERT INTO stability_pool_observations \
         (observation_key, federation_id, status, observed_provided_amount_sats, liquidity_stats_json, observation_json, observed_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(observation_key) DO UPDATE SET \
           federation_id = excluded.federation_id, status = excluded.status, \
           observed_provided_amount_sats = excluded.observed_provided_amount_sats, \
           liquidity_stats_json = excluded.liquidity_stats_json, \
           observation_json = excluded.observation_json, observed_at = excluded.observed_at",
    )
    .bind(key)
    .bind(&observation.federation_id)
    .bind(&observation.status)
    .bind(observation.observed_provided_amount.map(to_i64_sats).transpose()?)
    .bind(observation.liquidity_stats_json.as_deref())
    .bind(serde_json::to_string(observation).map_err(internal_error)?)
    .bind(timestamp_to_i64(observation.observed_at)?)
    .execute(database.pool())
    .await
    .map_err(internal_error)?;
    Ok(())
}

pub(crate) async fn list_allocations(
    database: &Database,
    request: ListAllocationsStoreRequest,
) -> ServiceResult<ListResponse<AdminAllocationSummary>> {
    let page = OffsetPage::from_page(&request.page, "allocation")?;

    let mut builder = QueryBuilder::new(
        "SELECT federation_id, committed_amount_sats, created_at, \
          MAX(updated_at, COALESCE((SELECT MAX(updated_at) FROM allocation_items i \
                                    WHERE i.federation_id = allocations.federation_id), updated_at)) AS updated_at, \
          (SELECT status FROM allocation_items i WHERE i.federation_id = allocations.federation_id \
           AND i.source_type = 'gateway') AS gateway_status, \
          (SELECT status FROM allocation_items i WHERE i.federation_id = allocations.federation_id \
           AND i.source_type = 'stability_pool') AS stability_pool_status \
         FROM allocations WHERE TRUE",
    );
    if let Some(time_range) = request.time_range {
        if let Some(from) = time_range.from {
            builder.push(" AND created_at >= ");
            builder.push_bind(timestamp_to_i64(from)?);
        }
        if let Some(to) = time_range.to {
            builder.push(" AND created_at < ");
            builder.push_bind(timestamp_to_i64(to)?);
        }
    }
    builder.push(" ORDER BY created_at DESC, federation_id DESC LIMIT ");
    builder.push_bind(page.fetch_limit());
    builder.push(" OFFSET ");
    builder.push_bind(page.offset());

    let rows = builder
        .build()
        .fetch_all(database.pool())
        .await
        .map_err(internal_error)?;
    page.list_response(rows, admin_allocation_summary_from_row)
}

pub(crate) async fn get_admin_allocation(
    database: &Database,
    federation_id: &FederationId,
) -> ServiceResult<GetAdminAllocationResponse> {
    let status = load_allocation_status_by_federation(database, federation_id)
        .await?
        .ok_or_else(|| not_found("allocation not found"))?;
    let wallet_operations =
        wallet::wallet_operations_for_federation(database, federation_id).await?;
    let failures = wallet_operations
        .iter()
        .filter_map(|operation| operation.failure.clone())
        .collect();
    Ok(GetAdminAllocationResponse {
        allocation: AdminAllocationDetail {
            federation_id: federation_id.clone(),
            status,
            wallet_operations,
            failures,
        },
    })
}

pub(crate) async fn get_verification_summary(
    database: &Database,
    federation_id: &FederationId,
) -> ServiceResult<GetVerificationSummaryResponse> {
    let verification_json: Option<String> =
        sqlx::query_scalar("SELECT verification_json FROM allocations WHERE federation_id = ?")
            .bind(&federation_id.0)
            .fetch_optional(database.pool())
            .await
            .map_err(internal_error)?;
    let verification_json = verification_json.ok_or_else(|| not_found("allocation not found"))?;
    let summary = serde_json::from_str(&verification_json).map_err(internal_error)?;
    Ok(GetVerificationSummaryResponse { summary })
}

fn gateway_item_from_row(row: &SqliteRow) -> ServiceResult<GatewayAllocationItem> {
    let target_json: String = row.get("target_json");
    let item_json: String = row.get("item_json");
    let step_json: Option<String> = row.get("step_json");
    Ok(GatewayAllocationItem {
        item_id: ItemId(row.get("item_id")),
        federation_id: FederationId(row.get("federation_id")),
        target: serde_json::from_str(&target_json).map_err(internal_error)?,
        item_target: serde_json::from_str(&item_json).map_err(internal_error)?,
        committed_amount: Sats(row.get::<i64, _>("committed_amount_sats").max(0) as u64),
        reserved_amount: Sats(row.get::<i64, _>("reserved_amount_sats").max(0) as u64),
        status: item_status_from_str(row.get::<String, _>("item_status").as_str())?,
        step: step_json
            .map(|json| serde_json::from_str(&json).map_err(internal_error))
            .transpose()?
            .unwrap_or_default(),
    })
}

fn stability_pool_item_from_row(row: &SqliteRow) -> ServiceResult<StabilityPoolAllocationItem> {
    let target_json: String = row.get("target_json");
    let item_json: String = row.get("item_json");
    let step_json: Option<String> = row.get("step_json");
    let (step, corrupt_step_json) = match step_json {
        Some(json) => match serde_json::from_str(&json) {
            Ok(step) => (step, None),
            Err(_) => (StabilityPoolAllocationStep::default(), Some(json)),
        },
        None => (StabilityPoolAllocationStep::default(), None),
    };
    Ok(StabilityPoolAllocationItem {
        item_id: ItemId(row.get("item_id")),
        federation_id: FederationId(row.get("federation_id")),
        target: serde_json::from_str(&target_json).map_err(internal_error)?,
        item_target: serde_json::from_str(&item_json).map_err(internal_error)?,
        committed_amount: Sats(row.get::<i64, _>("committed_amount_sats").max(0) as u64),
        reserved_amount: Sats(row.get::<i64, _>("reserved_amount_sats").max(0) as u64),
        status: item_status_from_str(row.get::<String, _>("item_status").as_str())?,
        step,
        corrupt_step_json,
    })
}

fn allocation_item_status_from_row(row: SqliteRow) -> ServiceResult<AllocationItemStatus> {
    let item_json: String = row.get("item_json");
    let target: AllocationItemTarget = serde_json::from_str(&item_json).map_err(internal_error)?;
    Ok(AllocationItemStatus {
        target,
        status: item_status_from_str(row.get::<String, _>("status").as_str())?,
        fulfilled_amount: row
            .get::<Option<i64>, _>("fulfilled_amount_sats")
            .map(|amount| Sats(amount.max(0) as u64)),
        completion_evidence: row
            .get::<Option<String>, _>("completion_evidence_json")
            .map(|json| serde_json::from_str(&json).map_err(internal_error))
            .transpose()?,
        failure: row
            .get::<Option<String>, _>("failure_json")
            .map(|json| serde_json::from_str(&json).map_err(internal_error))
            .transpose()?,
        updated_at: Timestamp(row.get::<i64, _>("updated_at").max(0) as u64),
    })
}

fn admin_allocation_summary_from_row(row: SqliteRow) -> ServiceResult<AdminAllocationSummary> {
    Ok(AdminAllocationSummary {
        federation_id: FederationId(row.get("federation_id")),
        gateway_status: row
            .get::<Option<String>, _>("gateway_status")
            .map(|status| item_status_from_str(&status))
            .transpose()?,
        stability_pool_status: row
            .get::<Option<String>, _>("stability_pool_status")
            .map(|status| item_status_from_str(&status))
            .transpose()?,
        committed_amount: Sats(row.get::<i64, _>("committed_amount_sats").max(0) as u64),
        created_at: Timestamp(row.get::<i64, _>("created_at").max(0) as u64),
        updated_at: Timestamp(row.get::<i64, _>("updated_at").max(0) as u64),
    })
}

fn item_status_from_str(status: &str) -> ServiceResult<ItemAllocationStatus> {
    status
        .parse()
        .map_err(|_| internal_error(format!("unknown allocation item status {status:?}")))
}

fn timestamp_to_i64(timestamp: Timestamp) -> ServiceResult<i64> {
    i64::try_from(timestamp.0).map_err(|_| invalid_argument("timestamp exceeds SQLite i64 range"))
}

#[cfg(test)]
#[path = "../tests/allocation_store.rs"]
mod tests;
