use fedi_decentralized_service_liquidity_manager::FederationId;
use serde_json::json;

use super::*;
use crate::test_support::{AllocationSeed, test_sqlite_path};

/// The recovery summary persisted in `recovery_runs.summary_json` is flat
/// strings, not tagged objects.
///
/// The rows carry domain types now rather than bare `String`s. Every one of
/// them is a `#[serde(transparent)]` newtype or a `rename_all = "snake_case"`
/// enum, so the stored JSON is unchanged — this is what says so out loud, since
/// a newtype that lost `transparent` would quietly start writing `{"0": ...}`.
#[test]
fn the_persisted_recovery_summary_stays_flat_strings() {
    let item = RecoveredAllocationItem {
        item_id: ItemId("federation-1:gateway".to_owned()),
        federation_id: FederationId("federation-1".to_owned()),
        source_type: SourceType::Gateway,
        status: ItemAllocationStatus::Running,
        updated_at: Timestamp(1_700_000_000),
    };
    assert_eq!(
        serde_json::to_value(&item).unwrap(),
        json!({
            "item_id": "federation-1:gateway",
            "federation_id": "federation-1",
            "source_type": "gateway",
            "status": "running",
            "updated_at": 1_700_000_000u64,
        })
    );

    let operation = RecoveredWalletOperation {
        operation_id: WalletOperationId("wallet-op-1".to_owned()),
        operation_type: WalletOperationType::Withdrawal,
        status: WalletOperationStatus::Broadcast,
        federation_id: Some(FederationId("federation-1".to_owned())),
        item_id: Some(ItemId("federation-1:gateway".to_owned())),
        updated_at: Timestamp(1_700_000_000),
    };
    assert_eq!(
        serde_json::to_value(&operation).unwrap(),
        json!({
            "operation_id": "wallet-op-1",
            "operation_type": "withdrawal",
            "status": "broadcast",
            "federation_id": "federation-1",
            "item_id": "federation-1:gateway",
            "updated_at": 1_700_000_000u64,
        })
    );
}

#[tokio::test]
async fn recovery_rehydrates_active_dummy_allocation_work() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("recovery-dummy-work")).await?;

    seed_active_dummy_work(&database).await?;

    let snapshot = run_startup_recovery(&database).await?;

    assert_eq!(snapshot.active_allocation_items.len(), 1);
    assert_eq!(snapshot.active_wallet_operations.len(), 1);
    assert_eq!(snapshot.active_work_count(), 2);

    let recovery_run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recovery_runs")
        .fetch_one(database.pool())
        .await?;
    assert_eq!(recovery_run_count, 1);

    Ok(())
}

#[tokio::test]
async fn recovery_is_idempotent() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("recovery-idempotency")).await?;
    seed_active_dummy_work(&database).await?;

    let first = run_startup_recovery(&database).await?;
    let second = run_startup_recovery(&database).await?;

    assert_eq!(first.active_work_count(), second.active_work_count());

    let recovery_run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recovery_runs")
        .fetch_one(database.pool())
        .await?;
    assert_eq!(recovery_run_count, 2);

    Ok(())
}

#[tokio::test]
async fn recovery_ignores_terminal_states() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("recovery-terminal-states")).await?;

    seed_active_dummy_work(&database).await?;
    for federation in [
        "federation-terminal-completed",
        "federation-terminal-failed",
        "federation-terminal-cancelled",
    ] {
        seed_allocation(&database, federation).await?;
    }
    sqlx::query(
        "INSERT INTO allocation_items (item_id, federation_id, source_type, status) \
         VALUES \
         ('item-completed', 'federation-terminal-completed', 'gateway', 'completed'), \
         ('item-failed', 'federation-terminal-failed', 'gateway', 'failed'), \
         ('item-cancelled', 'federation-terminal-cancelled', 'gateway', 'cancelled')",
    )
    .execute(database.pool())
    .await?;
    sqlx::query(
        "INSERT INTO wallet_operations (operation_id, operation_type, status) \
         VALUES \
         ('wallet-op-completed', 'gateway_funding', 'completed'), \
         ('wallet-op-failed', 'gateway_funding', 'failed'), \
         ('wallet-op-cancelled', 'gateway_funding', 'cancelled')",
    )
    .execute(database.pool())
    .await?;

    let snapshot = run_startup_recovery(&database).await?;

    assert_eq!(snapshot.active_allocation_items.len(), 1);
    assert_eq!(snapshot.active_allocation_items[0].item_id.0, "item-1");
    assert_eq!(snapshot.active_wallet_operations.len(), 1);
    assert_eq!(
        snapshot.active_wallet_operations[0].operation_id.0,
        "wallet-op-1"
    );

    Ok(())
}

#[tokio::test]
async fn phase2_uniqueness_indexes_reject_duplicate_active_work_keys() -> anyhow::Result<()> {
    let database = Database::connect(test_sqlite_path("phase2-uniqueness")).await?;

    seed_allocation(&database, "federation-1").await?;
    sqlx::query(
        "INSERT INTO allocation_items (item_id, federation_id, source_type, status) \
         VALUES ('item-1', 'federation-1', 'gateway', 'pending')",
    )
    .execute(database.pool())
    .await?;
    let duplicate_item = sqlx::query(
        "INSERT INTO allocation_items (item_id, federation_id, source_type, status) \
         VALUES ('item-duplicate', 'federation-1', 'gateway', 'pending')",
    )
    .execute(database.pool())
    .await;
    assert!(duplicate_item.is_err());

    let duplicate_allocation = sqlx::query(
        "INSERT INTO allocations \
         (federation_id, requester_pubkey, provider_pubkey, network, details_payload_hash, \
          request_json, verification_json, target_json) \
         VALUES ('federation-1', 'requester-1', 'provider-1', 'regtest', x'ff', \
                 '{}', '{}', '{}')",
    )
    .execute(database.pool())
    .await;
    assert!(duplicate_allocation.is_err());

    sqlx::query(
        "INSERT INTO wallet_operations \
         (operation_id, operation_type, status, external_operation_id) \
         VALUES ('wallet-op-1', 'gateway_funding', 'pending', 'external-1')",
    )
    .execute(database.pool())
    .await?;
    let duplicate_wallet_operation = sqlx::query(
        "INSERT INTO wallet_operations \
         (operation_id, operation_type, status, external_operation_id) \
         VALUES ('wallet-op-duplicate', 'gateway_funding', 'pending', 'external-1')",
    )
    .execute(database.pool())
    .await;
    assert!(duplicate_wallet_operation.is_err());

    Ok(())
}

async fn seed_allocation(database: &Database, federation_id: &str) -> anyhow::Result<()> {
    AllocationSeed {
        federation_id: FederationId(federation_id.to_owned()),
        ..AllocationSeed::default()
    }
    .insert(database)
    .await
}

async fn seed_active_dummy_work(database: &Database) -> anyhow::Result<()> {
    seed_allocation(database, "federation-1").await?;
    sqlx::query(
        "INSERT INTO allocation_items \
         (item_id, federation_id, source_type, status) \
         VALUES ('item-1', 'federation-1', 'gateway', 'running')",
    )
    .execute(database.pool())
    .await?;
    sqlx::query(
        "INSERT INTO wallet_operations \
         (operation_id, operation_type, status, federation_id, item_id) \
         VALUES ('wallet-op-1', 'gateway_funding', 'in_doubt', 'federation-1', 'item-1')",
    )
    .execute(database.pool())
    .await?;

    Ok(())
}
