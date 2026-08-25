use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use super::*;
use crate::test_support::{AllocationSeed, ItemSeed};

/// The deposit status runs one way for one operation id.
///
/// Once an observation has ended a deposit, no writer may restore `initiated`
/// or `tx_accepted` for it. Observation arms that assign unconditionally break
/// that: a drain replaying from the start after a durable `success` walks the
/// status back to a nonterminal one, and the item looks unfinished again.
#[test]
fn a_deposit_status_never_walks_backwards_for_one_operation_id() {
    let mut step = StabilityPoolAllocationStep::default();

    assert!(step.advance_sp_deposit_status("operation-a", SpDepositStatus::Initiated));
    assert!(step.advance_sp_deposit_status("operation-a", SpDepositStatus::TxAccepted));
    assert!(step.advance_sp_deposit_status("operation-a", SpDepositStatus::Success));

    // The replay a failed outcome-cache write produces. Both are refused and
    // the terminal observation stands.
    assert!(!step.advance_sp_deposit_status("operation-a", SpDepositStatus::Initiated));
    assert!(!step.advance_sp_deposit_status("operation-a", SpDepositStatus::TxAccepted));
    assert_eq!(step.sp_deposit_status, Some(SpDepositStatus::Success));

    // Re-observing the same terminal state is not backwards.
    assert!(step.advance_sp_deposit_status("operation-a", SpDepositStatus::Success));
}

/// A different operation id is a different deposit and starts again.
///
/// This is what keeps `bind_target_deposit` working: an operator who binds a
/// new operation id after a lost one needs the item to resume from `initiated`,
/// and that is a different deposit, not the old one walking backwards.
#[test]
fn a_new_operation_id_starts_its_own_deposit_sequence() {
    let mut step = StabilityPoolAllocationStep::default();
    assert!(step.advance_sp_deposit_status("operation-a", SpDepositStatus::Success));

    assert!(step.advance_sp_deposit_status("operation-b", SpDepositStatus::Initiated));
    assert_eq!(step.sp_deposit_status, Some(SpDepositStatus::Initiated));
    assert_eq!(
        step.sp_deposit_operation_id.as_deref(),
        Some("operation-b"),
        "the setter owns both fields, so the id and the status cannot disagree"
    );
}

/// The stored vocabulary is exactly the strings that were stored before these
/// were types.
///
/// The step is persisted as JSON, and live databases, the e2e fixtures, and the
/// operator dashboard all read these strings. A variant that displayed as
/// anything else would silently write a value nothing else recognises.
#[test]
fn the_step_status_vocabulary_round_trips_through_its_stored_strings() {
    for (status, stored) in [
        (SpDepositStatus::Submitting, "submitting"),
        (SpDepositStatus::Initiated, "initiated"),
        (SpDepositStatus::TxAccepted, "tx_accepted"),
        (SpDepositStatus::Success, "success"),
    ] {
        assert_eq!(serde_json::to_value(&status).unwrap(), json!(stored));
        assert_eq!(
            serde_json::from_value::<SpDepositStatus>(json!(stored)).unwrap(),
            status
        );
    }

    for (status, stored) in [
        (
            PegInProgress::WaitingForTransaction,
            "waiting_for_transaction",
        ),
        (
            PegInProgress::WaitingForConfirmation,
            "waiting_for_confirmation",
        ),
        (PegInProgress::Confirmed, "confirmed"),
        (PegInProgress::Claimed, "claimed"),
    ] {
        assert_eq!(serde_json::to_value(&status).unwrap(), json!(stored));
        assert_eq!(
            serde_json::from_value::<PegInProgress>(json!(stored)).unwrap(),
            status
        );
    }

    // An unrecognised string survives the round trip unchanged rather than
    // failing the read and making the whole step unreadable.
    let later = serde_json::from_value::<SpDepositStatus>(json!("from-a-later-build")).unwrap();
    assert_eq!(
        later,
        SpDepositStatus::Unknown("from-a-later-build".to_owned())
    );
    assert_eq!(
        serde_json::to_value(&later).unwrap(),
        json!("from-a-later-build")
    );

    // A non-string is still a corrupt step, which is what the recovery path
    // reports separately.
    assert!(serde_json::from_value::<PegInProgress>(json!(42)).is_err());
}

/// A status this build does not know never blocks a write.
///
/// It cannot be ordered against anything, so treating it as a barrier would
/// wedge an item on a value a later build introduced or an earlier one left
/// behind.
#[test]
fn an_unknown_deposit_status_is_not_a_barrier() {
    let mut step = StabilityPoolAllocationStep {
        sp_deposit_operation_id: Some("operation-a".to_owned()),
        sp_deposit_status: Some(SpDepositStatus::from("from-a-later-build".to_owned())),
        ..StabilityPoolAllocationStep::default()
    };

    assert!(step.advance_sp_deposit_status("operation-a", SpDepositStatus::Initiated));
    assert_eq!(step.sp_deposit_status, Some(SpDepositStatus::Initiated));
}

#[tokio::test]
async fn mixed_item_statuses_are_returned_without_an_aggregate() -> anyhow::Result<()> {
    let database = test_database("independent-item-statuses").await?;
    let federation_id = FederationId("federation-1".to_owned());
    AllocationSeed {
        federation_id: federation_id.clone(),
        items: vec![
            ItemSeed {
                source_type: SourceType::Gateway,
                status: ItemAllocationStatus::Failed,
                ..ItemSeed::default()
            },
            ItemSeed {
                source_type: SourceType::StabilityPool,
                status: ItemAllocationStatus::Running,
                ..ItemSeed::default()
            },
        ],
        ..AllocationSeed::default()
    }
    .insert(&database)
    .await?;
    let status = load_allocation_status_by_federation(&database, &federation_id)
        .await?
        .expect("allocation");
    assert_eq!(status.item_statuses.len(), 2);
    assert!(
        status
            .item_statuses
            .iter()
            .any(|item| item.status == ItemAllocationStatus::Failed)
    );
    assert!(
        status
            .item_statuses
            .iter()
            .any(|item| item.status == ItemAllocationStatus::Running)
    );
    let listed = list_allocations(
        &database,
        ListAllocationsStoreRequest {
            page: fedi_decentralized_service_liquidity_manager::PageRequest {
                cursor: None,
                limit: 10,
            },
            time_range: None,
        },
    )
    .await?;
    assert_eq!(
        listed.items[0].gateway_status,
        Some(ItemAllocationStatus::Failed)
    );
    assert_eq!(
        listed.items[0].stability_pool_status,
        Some(ItemAllocationStatus::Running)
    );
    Ok(())
}

#[tokio::test]
async fn action_required_items_are_not_worker_active() -> anyhow::Result<()> {
    let database = test_database("action-required-not-active").await?;
    AllocationSeed {
        items: vec![ItemSeed {
            source_type: SourceType::Gateway,
            status: ItemAllocationStatus::ActionRequired,
            ..ItemSeed::default()
        }],
        ..AllocationSeed::default()
    }
    .insert(&database)
    .await?;
    assert!(active_gateway_items(&database).await?.is_empty());
    Ok(())
}

async fn test_database(name: &str) -> anyhow::Result<Database> {
    let data_dir = test_data_dir(name);
    tokio::fs::create_dir_all(&data_dir).await?;
    Database::connect(&data_dir.join("flip.sqlite")).await
}

fn test_data_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_nanos();
    std::env::temp_dir()
        .join("fedi-flip-tests")
        .join(format!("{name}-{}-{nanos}", std::process::id()))
}
