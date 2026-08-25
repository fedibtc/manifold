//! Startup recovery: what the workers resume before fresh work is accepted.
//!
//! Inventories active allocations, items, and wallet operations. Accepted
//! allocations are the only durable request outcome, so there are no
//! request-level records to reconcile or expire.

use fedi_decentralized_service_liquidity_manager::{
    FederationId, ItemAllocationStatus, ItemId, SourceType, Timestamp, WalletOperationId,
    WalletOperationStatus, WalletOperationType,
};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row};
use tracing::info;

use crate::allocation_store::ACTIVE_ITEM_STATUSES;
use crate::database::{Database, push_in_list};
use crate::now_timestamp;
use crate::wallet::PENDING_SETTLEMENT_STATUSES;

/// Durable work found during startup recovery.
///
/// Accepted allocations are the only durable request outcome, so recovery has
/// nothing to reconcile: it takes inventory of active allocations, items, and
/// wallet operations for the workers to resume.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RecoverySnapshot {
    /// Recovery pass start time.
    pub started_at: Timestamp,

    /// Recovery pass completion time.
    pub completed_at: Timestamp,

    /// Active allocation item rows.
    pub active_allocation_items: Vec<RecoveredAllocationItem>,

    /// Active wallet operation rows.
    pub active_wallet_operations: Vec<RecoveredWalletOperation>,
}

impl RecoverySnapshot {
    /// Count of all active durable work rows found during recovery.
    #[must_use]
    pub(crate) fn active_work_count(&self) -> usize {
        self.active_allocation_items.len() + self.active_wallet_operations.len()
    }

    /// Compact count summary for health output and tests.
    #[must_use]
    pub(crate) fn counts(&self) -> RecoveryCounts {
        RecoveryCounts {
            active_allocation_item_count: self.active_allocation_items.len(),
            active_wallet_operation_count: self.active_wallet_operations.len(),
        }
    }
}

/// Count-only recovery summary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RecoveryCounts {
    /// Allocations with live work.
    /// Active allocation item rows.
    pub active_allocation_item_count: usize,

    /// Active wallet operation rows.
    pub active_wallet_operation_count: usize,
}

/// Active allocation item row found during recovery.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RecoveredAllocationItem {
    /// Allocation item id.
    pub item_id: ItemId,

    /// Federation the allocation funds.
    pub federation_id: FederationId,

    /// Allocation source type.
    pub source_type: SourceType,

    /// Persisted item status.
    pub status: ItemAllocationStatus,

    /// Last update timestamp.
    pub updated_at: Timestamp,
}

/// Active wallet operation row found during recovery.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RecoveredWalletOperation {
    /// Wallet operation id.
    pub operation_id: WalletOperationId,

    /// Wallet operation type.
    pub operation_type: WalletOperationType,

    /// Persisted wallet operation status.
    pub status: WalletOperationStatus,

    /// Federation of the related allocation, when known.
    pub federation_id: Option<FederationId>,

    /// Related allocation item id, when known.
    pub item_id: Option<ItemId>,

    /// Last update timestamp.
    pub updated_at: Timestamp,
}

/// Runs startup recovery inventory before fresh work can be accepted.
pub(crate) async fn run_startup_recovery(database: &Database) -> anyhow::Result<RecoverySnapshot> {
    let started_at = now_timestamp();
    let active_allocation_items = load_active_allocation_items(database).await?;
    let active_wallet_operations = load_active_wallet_operations(database).await?;
    let completed_at = now_timestamp();
    let snapshot = RecoverySnapshot {
        started_at,
        completed_at,
        active_allocation_items,
        active_wallet_operations,
    };

    persist_recovery_run(database, &snapshot).await?;
    info!(
        active_work_count = snapshot.active_work_count(),
        "startup recovery completed"
    );

    Ok(snapshot)
}

async fn load_active_allocation_items(
    database: &Database,
) -> anyhow::Result<Vec<RecoveredAllocationItem>> {
    let mut builder = QueryBuilder::new(
        "SELECT item_id, federation_id, source_type, status, updated_at \
         FROM allocation_items WHERE ",
    );
    push_in_list(&mut builder, "status", &ACTIVE_ITEM_STATUSES);
    builder.push(" ORDER BY updated_at ASC, item_id ASC");
    let rows = builder.build().fetch_all(database.pool()).await?;

    rows.into_iter()
        .map(|row| {
            Ok(RecoveredAllocationItem {
                item_id: ItemId(row.get("item_id")),
                federation_id: FederationId(row.get("federation_id")),
                source_type: parse_column(&row, "source_type")?,
                status: parse_column(&row, "status")?,
                updated_at: Timestamp(row.get::<i64, _>("updated_at") as u64),
            })
        })
        .collect()
}

async fn load_active_wallet_operations(
    database: &Database,
) -> anyhow::Result<Vec<RecoveredWalletOperation>> {
    let mut builder = QueryBuilder::new(
        "SELECT operation_id, operation_type, status, federation_id, item_id, updated_at \
         FROM wallet_operations WHERE ",
    );
    push_in_list(&mut builder, "status", PENDING_SETTLEMENT_STATUSES);
    builder.push(" ORDER BY updated_at ASC, operation_id ASC");
    let rows = builder.build().fetch_all(database.pool()).await?;

    rows.into_iter()
        .map(|row| {
            Ok(RecoveredWalletOperation {
                operation_id: WalletOperationId(row.get("operation_id")),
                operation_type: parse_column(&row, "operation_type")?,
                status: parse_column(&row, "status")?,
                federation_id: row
                    .get::<Option<String>, _>("federation_id")
                    .map(FederationId),
                item_id: row.get::<Option<String>, _>("item_id").map(ItemId),
                updated_at: Timestamp(row.get::<i64, _>("updated_at") as u64),
            })
        })
        .collect()
}

/// Reads one column as the domain type its stored string stands for.
///
/// Both queries select `WHERE status IN (...)` over the vocabulary these types
/// declare, so a row that reaches here always parses. A failure means the table
/// holds a value the filter matched and the type does not, which is a defect
/// worth stopping startup for rather than carrying forward as a string.
fn parse_column<T>(row: &sqlx::sqlite::SqliteRow, column: &str) -> anyhow::Result<T>
where
    T: std::str::FromStr,
{
    let raw: String = row.get(column);
    raw.parse().map_err(|_| {
        anyhow::anyhow!("recovery read an unknown {column} {raw:?} from an active row")
    })
}

async fn persist_recovery_run(
    database: &Database,
    snapshot: &RecoverySnapshot,
) -> anyhow::Result<()> {
    let counts = snapshot.counts();
    let summary_json = serde_json::to_string(snapshot)?;
    sqlx::query(
        "INSERT INTO recovery_runs \
         (started_at, completed_at, active_allocation_item_count, \
          active_wallet_operation_count, summary_json) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(snapshot.started_at.0 as i64)
    .bind(snapshot.completed_at.0 as i64)
    .bind(counts.active_allocation_item_count as i64)
    .bind(counts.active_wallet_operation_count as i64)
    .bind(summary_json)
    .execute(database.pool())
    .await?;

    Ok(())
}

#[cfg(test)]
#[path = "../tests/recovery.rs"]
mod tests;
