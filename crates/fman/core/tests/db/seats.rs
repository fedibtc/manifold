use std::time::Duration;

use fedi_decentralized_service_fleet_manager::{
    DkgCompletionCallback, DkgCompletionCallbackInput, FederationSize, InviteCode, Plan,
};
use tempfile::TempDir;

use super::*;

/// Deterministic x-only pubkey seeded from a test name.
fn test_key(name: &str) -> secp256k1::XOnlyPublicKey {
    let mut seed = [0_u8; 32];
    seed[..name.len()].copy_from_slice(name.as_bytes());
    secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &seed)
        .unwrap()
        .x_only_public_key()
        .0
}

fn new_seat(quote: u8) -> NewSeat {
    let fi_id = FiId(test_key(&format!("fi{quote}")));
    NewSeat {
        seat_id: SeatId::from(QuoteId([quote; 32])),
        fi_id,
        plan: Plan::InfiniteBestEffort { price_msats: 0 },
        federation_size: FederationSize(7),
        payment: Some(NewPayment {
            evidence: crate::wallet::EcashClaimEvidence::test(quote),
        }),
    }
}

fn completion_callback(name: &str) -> DkgCompletionCallback {
    DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: format!("https://push.example/hooks/{name}/secret"),
        idempotency_key: name.to_owned(),
    })
    .unwrap()
}

#[tokio::test]
async fn completion_callback_is_installed_once_for_the_whole_formation() {
    let (_dir, db) = open_db().await;
    let seat = crate::test_support::insert_test_seat(&db, new_seat(40)).await;
    let first = completion_callback("first");
    db.install_completion_callback(&seat.seat_id, Some(&first))
        .await
        .unwrap();
    db.install_completion_callback(&seat.seat_id, Some(&completion_callback("later-attempt")))
        .await
        .unwrap();

    assert_eq!(
        db.completion_callback(&seat.seat_id)
            .await
            .unwrap()
            .unwrap()
            .callback,
        Some(first)
    );

    let without_callback = crate::test_support::insert_test_seat(&db, new_seat(42)).await;
    db.install_completion_callback(&without_callback.seat_id, None)
        .await
        .unwrap();
    db.install_completion_callback(
        &without_callback.seat_id,
        Some(&completion_callback("too-late")),
    )
    .await
    .unwrap();
    let retained = db
        .completion_callback(&without_callback.seat_id)
        .await
        .unwrap()
        .unwrap();
    assert!(retained.callback.is_none());
    assert_eq!(retained.status, CompletionCallbackStatus::NotConfigured);
}

#[tokio::test]
async fn completion_callback_attempt_start_requires_formation_and_live_seat() {
    let (_dir, db) = open_db().await;
    let seat = crate::test_support::insert_test_seat(&db, new_seat(41)).await;
    db.install_completion_callback(&seat.seat_id, Some(&completion_callback("callback")))
        .await
        .unwrap();
    assert!(
        !db.record_completion_callback_attempt_started(&seat.seat_id, now_ms() + 1_000)
            .await
            .unwrap(),
        "an unformed seat cannot start delivery"
    );

    db.record_formed(&seat.seat_id, &InviteCode("invite".to_owned()))
        .await
        .unwrap();
    assert!(
        db.record_completion_callback_attempt_started(&seat.seat_id, now_ms() + 1_000)
            .await
            .unwrap()
    );

    db.decommission_seat(&seat.seat_id).await.unwrap();
    assert!(
        !db.record_completion_callback_attempt_started(&seat.seat_id, now_ms() + 1_000)
            .await
            .unwrap(),
        "a decommissioned seat cannot start delivery"
    );
}

#[tokio::test]
async fn completion_callback_attempt_counter_saturates_at_its_public_maximum() {
    let (_dir, db) = open_db().await;
    let seat = crate::test_support::insert_test_seat(&db, new_seat(48)).await;
    db.install_completion_callback(&seat.seat_id, Some(&completion_callback("callback")))
        .await
        .unwrap();
    db.record_formed(&seat.seat_id, &InviteCode("invite".to_owned()))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE completion_callbacks SET completion_callback_attempts = 4294967295 \
         WHERE quote_id = ?",
    )
    .bind(seat.seat_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();

    assert!(
        db.record_completion_callback_attempt_started(&seat.seat_id, now_ms() + 1_000)
            .await
            .unwrap()
    );
    assert_eq!(
        db.completion_callback(&seat.seat_id)
            .await
            .unwrap()
            .unwrap()
            .status
            .attempts(),
        u32::MAX
    );
}

#[tokio::test]
async fn startup_validation_rejects_a_malformed_retained_bearer() {
    let (_dir, db) = open_db().await;
    let seat = crate::test_support::insert_test_seat(&db, new_seat(46)).await;
    db.install_completion_callback(&seat.seat_id, Some(&completion_callback("callback")))
        .await
        .unwrap();
    sqlx::query("UPDATE completion_callbacks SET completion_callback = '{' WHERE quote_id = ?")
        .bind(seat.seat_id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();

    assert!(matches!(
        db.validate_completion_callbacks().await,
        Err(DbError::CorruptRow {
            table: "completion_callbacks",
            ..
        })
    ));
}

#[tokio::test]
async fn operator_status_projection_never_deserializes_the_callback_bearer() {
    let (_dir, db) = open_db().await;
    let seat = crate::test_support::insert_test_seat(&db, new_seat(47)).await;
    db.install_completion_callback(&seat.seat_id, Some(&completion_callback("callback")))
        .await
        .unwrap();
    sqlx::query("UPDATE completion_callbacks SET completion_callback = '{' WHERE quote_id = ?")
        .bind(seat.seat_id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();

    assert!(matches!(
        db.completion_callback_status(&seat.seat_id)
            .await
            .unwrap()
            .unwrap(),
        CompletionCallbackStatus::Pending { attempts: 0, .. }
    ));
    assert!(matches!(
        db.completion_callback(&seat.seat_id).await,
        Err(DbError::CorruptRow {
            table: "completion_callbacks",
            ..
        })
    ));
}

#[tokio::test]
async fn seat_creation_facts_and_startup_load_roundtrip() {
    let (_dir, db) = open_db().await;
    let created = crate::test_support::insert_test_seat(&db, new_seat(1)).await;

    assert_eq!(created.seat_no, SeatNo(0));
    let stored_plan: String = sqlx::query_scalar("SELECT plan FROM seats WHERE quote_id = ?")
        .bind(created.seat_id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        stored_plan,
        serde_json_canonicalizer::to_string(&created.plan).unwrap()
    );
    assert_eq!(
        db.list_seats().await.unwrap(),
        vec![SeatRecord {
            facts: created.clone(),
            decommissioned_at_ms: None,
        }]
    );
    let payment = db.payment(&created.seat_id).await.unwrap().unwrap();
    assert_eq!(payment.evidence, crate::wallet::EcashClaimEvidence::test(1));
    assert_eq!(payment.outcome, None);
}

#[tokio::test]
async fn free_seat_has_no_payment_ledger_row() {
    let (_dir, db) = open_db().await;
    let mut seat = new_seat(9);
    seat.payment = None;
    let created = crate::test_support::insert_test_seat(&db, seat).await;
    assert!(db.payment(&created.seat_id).await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admission_transaction_serializes_the_last_slot() {
    let (_dir, db) = open_db().await;
    db.set_max_seats(1).await.unwrap();
    let epoch = db.offer_epoch().await.unwrap();
    let base = crate::facts::PortBase::new(30_000).unwrap();

    let (first, second) = tokio::join!(
        db.admit_seat(new_seat(21), epoch, base),
        db.admit_seat(new_seat(22), epoch, base),
    );
    let outcomes = [first.unwrap(), second.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SeatAdmissionResult::Inserted(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SeatAdmissionResult::OfferChanged))
            .count(),
        1
    );
}

#[tokio::test]
async fn durable_acceptance_precedes_a_later_epoch_mismatch() {
    let (_dir, db) = open_db().await;
    db.set_max_seats(1).await.unwrap();
    let epoch = db.offer_epoch().await.unwrap();
    let base = crate::facts::PortBase::new(30_000).unwrap();

    assert!(matches!(
        db.admit_seat(new_seat(23), epoch, base).await.unwrap(),
        SeatAdmissionResult::Inserted(_)
    ));
    assert!(matches!(
        db.admit_seat(new_seat(23), epoch, base).await.unwrap(),
        SeatAdmissionResult::Existing(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_snapshots_resolve_to_one_acceptance_without_a_refusal() {
    let (_dir, db) = open_db().await;
    db.set_max_seats(1).await.unwrap();
    let epoch = db.offer_epoch().await.unwrap();
    let base = crate::facts::PortBase::new(30_000).unwrap();
    let seat_id = new_seat(23).seat_id;

    // Force both duplicate requests through the formerly unsafe window: each
    // observes an absent seat and the same current epoch before either enters
    // the writer boundary.
    for _ in 0..2 {
        let (existing, observed_epoch) = db.admission_snapshot(&seat_id).await.unwrap();
        assert!(existing.is_none());
        assert_eq!(observed_epoch, epoch);
    }

    let (first, second) = tokio::join!(
        db.admit_seat_at_writer_boundary(new_seat(23), epoch, base),
        db.admit_seat_at_writer_boundary(new_seat(23), epoch, base),
    );
    let outcomes = [first.unwrap(), second.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SeatAdmissionResult::Inserted(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SeatAdmissionResult::Existing(_)))
            .count(),
        1
    );
    assert!(
        outcomes
            .iter()
            .all(|outcome| !matches!(outcome, SeatAdmissionResult::OfferChanged)),
        "a duplicate of a durable acceptance must never receive a refund-bearing refusal"
    );
}

#[tokio::test]
async fn capacity_cannot_shrink_below_active_seats_and_changes_rotate_the_offer() {
    let (_dir, db) = open_db().await;
    db.set_max_seats(2).await.unwrap();
    let first_epoch = db.offer_epoch().await.unwrap();
    assert!(matches!(
        db.admit_seat(
            new_seat(24),
            first_epoch,
            crate::facts::PortBase::new(30_000).unwrap()
        )
        .await
        .unwrap(),
        SeatAdmissionResult::Inserted(_)
    ));

    let error = db.set_max_seats(0).await.unwrap_err();
    assert!(matches!(
        error,
        crate::db::DbError::SeatLimitBelowActive {
            requested: 0,
            active: 1
        }
    ));
    assert_eq!(db.max_seats().await.unwrap(), 2);
    assert_eq!(db.offer_epoch().await.unwrap(), first_epoch);

    db.set_max_seats(1).await.unwrap();
    let second_epoch = db.offer_epoch().await.unwrap();
    assert_ne!(second_epoch, first_epoch);
    db.set_max_seats(1).await.unwrap();
    assert_eq!(db.offer_epoch().await.unwrap(), second_epoch);
}

#[tokio::test]
async fn terminal_read_snapshot_outcomes_do_not_wait_for_the_sqlite_writer() {
    let (_dir, db) = open_db().await;
    db.set_max_seats(2).await.unwrap();
    let epoch = db.offer_epoch().await.unwrap();
    let base = crate::facts::PortBase::new(30_000).unwrap();
    assert!(matches!(
        db.admit_seat(new_seat(23), epoch, base).await.unwrap(),
        SeatAdmissionResult::Inserted(_)
    ));
    db.set_offered_price(Some(crate::wallet::Msats(1)))
        .await
        .unwrap();

    // Hold the single writer. WAL read snapshots still complete, proving that
    // immutable replay and an already-stale absent quote do not request it.
    let writer = db.begin_write().await.unwrap();
    let existing = tokio::time::timeout(
        Duration::from_secs(2),
        db.admit_seat(new_seat(23), epoch, base),
    )
    .await
    .expect("existing admission should not wait for the writer")
    .unwrap();
    assert!(matches!(existing, SeatAdmissionResult::Existing(_)));

    let stale = tokio::time::timeout(
        Duration::from_secs(2),
        db.admit_seat(new_seat(24), epoch, base),
    )
    .await
    .expect("stale admission should not wait for the writer")
    .unwrap();
    assert!(matches!(stale, SeatAdmissionResult::OfferChanged));
    writer.rollback().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admission_and_offer_change_share_the_sqlite_writer_order() {
    let (_dir, db) = open_db().await;
    db.set_max_seats(2).await.unwrap();
    let epoch = db.offer_epoch().await.unwrap();
    let base = crate::facts::PortBase::new(30_000).unwrap();

    let (admission, price) = tokio::join!(
        db.admit_seat(new_seat(25), epoch, base),
        db.set_offered_price(Some(crate::wallet::Msats(1))),
    );
    price.unwrap();
    match admission.unwrap() {
        SeatAdmissionResult::Inserted(_) => {
            assert_eq!(db.list_seats().await.unwrap().len(), 1)
        }
        SeatAdmissionResult::OfferChanged => assert!(db.list_seats().await.unwrap().is_empty()),
        SeatAdmissionResult::Existing(_) => panic!("the quote was not previously accepted"),
    }
}

#[tokio::test]
async fn seat_identity_cannot_be_updated_deleted_or_replaced() {
    let (_dir, db) = open_db().await;
    let created = crate::test_support::insert_test_seat(&db, new_seat(1)).await;

    for statement in [
        "UPDATE seats SET fi_id = 'attacker' WHERE quote_id = ?",
        "UPDATE seats SET seat_no = 9 WHERE quote_id = ?",
        "DELETE FROM seats WHERE quote_id = ?",
    ] {
        assert!(
            sqlx::query(statement)
                .bind(created.seat_id.as_bytes().as_slice())
                .execute(db.pool())
                .await
                .is_err()
        );
    }

    let replace = sqlx::query(
        "INSERT OR REPLACE INTO seats (quote_id, seat_no, fi_id, plan, federation_size, created_at_ms) \
         SELECT quote_id, seat_no, 'attacker', plan, federation_size, created_at_ms \
         FROM seats WHERE quote_id = ?",
    )
    .bind(created.seat_id.as_bytes().as_slice())
    .execute(db.pool())
    .await;
    assert!(replace.is_err());
    assert_eq!(reload(&db, &created.seat_id).await.facts, created);
}

#[tokio::test]
async fn payment_recovery_material_survives_terminal_outcome() {
    let (_dir, db) = open_db().await;
    let seat = crate::test_support::insert_test_seat(&db, new_seat(7)).await;
    db.record_claim_outcome(&seat.seat_id, crate::wallet::ClaimOutcome::Success)
        .await
        .unwrap();
    let payment = db.payment(&seat.seat_id).await.unwrap().unwrap();
    assert_eq!(payment.evidence, crate::wallet::EcashClaimEvidence::test(7));
    assert_eq!(payment.outcome, Some(crate::wallet::ClaimOutcome::Success));
    assert!(
        sqlx::query("DELETE FROM ecash_claims WHERE quote_id = ?")
            .bind(seat.seat_id.as_bytes().as_slice())
            .execute(db.pool())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn already_spent_claim_outcome_roundtrips() {
    let (_dir, db) = open_db().await;
    let seat = crate::test_support::insert_test_seat(&db, new_seat(8)).await;
    db.record_claim_outcome(&seat.seat_id, crate::wallet::ClaimOutcome::AlreadySpent)
        .await
        .unwrap();

    let payment = db.payment(&seat.seat_id).await.unwrap().unwrap();
    assert_eq!(
        payment.outcome,
        Some(crate::wallet::ClaimOutcome::AlreadySpent)
    );
}

#[tokio::test]
async fn mint_generations_roundtrip_through_cbor_claim_evidence() {
    use fedi_decentralized_service_fleet_manager::{
        InviteCode, LockedBlindedSignature, LockedIssuanceRequest, LockedIssuanceRequestV2,
    };

    let (_dir, db) = open_db().await;
    let v1 = crate::wallet::EcashClaimEvidence::MintV1 {
        federation_invite: InviteCode("invite-v1".to_owned()),
        module_id: 7,
        quote_nonce: [3; 32],
        issuance: vec![LockedIssuanceRequest {
            amount_msats: u64::MAX,
            blind_nonce: vec![1, 2],
        }],
        signatures: vec![LockedBlindedSignature(vec![3, 4])],
    };
    let v2 = crate::wallet::EcashClaimEvidence::MintV2 {
        federation_invite: InviteCode("invite-v2".to_owned()),
        module_id: 8,
        issuance: vec![LockedIssuanceRequestV2 {
            amount_msats: 42,
            blind_nonce: vec![5, 6],
            tweak: [7; 16],
        }],
        signatures: vec![LockedBlindedSignature(vec![8, 9])],
    };
    for (quote, evidence) in [(3, v1), (4, v2)] {
        let mut seat = new_seat(quote);
        seat.payment = Some(NewPayment {
            evidence: evidence.clone(),
        });
        let seat = crate::test_support::insert_test_seat(&db, seat).await;
        assert_eq!(
            db.payment(&seat.seat_id).await.unwrap().unwrap().evidence,
            evidence
        );
    }
}

#[tokio::test]
async fn schema_separates_payout_settings_from_offer_state_and_has_no_refund_ledger() {
    let (_dir, db) = open_db().await;
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(db.pool())
            .await
            .unwrap();

    assert!(!tables.iter().any(|table| table == "refund_ledger"));
    assert!(tables.iter().any(|table| table == "offer_state"));
    assert!(tables.iter().any(|table| table == "payout_settings"));
    assert!(!tables.iter().any(|table| table == "ecash_claim_notes"));

    let claim_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('ecash_claims')")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert!(claim_columns.iter().any(|column| column == "evidence"));
    assert!(
        !claim_columns
            .iter()
            .any(|column| column == "mint_generation")
    );

    crate::test_support::insert_test_seat(&db, new_seat(1)).await;
    let evidence_type: String =
        sqlx::query_scalar("SELECT typeof(evidence) FROM ecash_claims LIMIT 1")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(evidence_type, "blob");

    let offer_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('offer_state')")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert!(!offer_columns.iter().any(|column| column == "destination"));
    assert!(
        !offer_columns
            .iter()
            .any(|column| column == "payout_destination")
    );
}

#[tokio::test]
async fn decommission_write_is_set_once_without_releasing_creation_facts() {
    let (_dir, db) = open_db().await;
    let seat = crate::test_support::insert_test_seat(&db, new_seat(1)).await;

    let at_ms = db.decommission_seat(&seat.seat_id).await.unwrap();
    assert!(db.decommission_seat(&seat.seat_id).await.is_err());

    let loaded = reload(&db, &seat.seat_id).await;
    assert_eq!(loaded.facts, seat);
    assert_eq!(loaded.decommissioned_at_ms, Some(at_ms));
}

#[tokio::test]
async fn formed_seat_is_set_once_and_immutable() {
    let (_dir, db) = open_db().await;
    let seat = crate::test_support::insert_test_seat(&db, new_seat(1)).await;
    let invite = InviteCode("invite-one".to_owned());

    let formed_at = db.record_formed(&seat.seat_id, &invite).await.unwrap();
    assert_eq!(
        db.record_formed(&seat.seat_id, &invite).await.unwrap(),
        formed_at
    );
    assert!(
        db.record_formed(&seat.seat_id, &InviteCode("invite-two".to_owned()))
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE formed_seats SET federation_invite = 'other' WHERE quote_id = ?")
            .bind(seat.seat_id.as_bytes().as_slice())
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM formed_seats WHERE quote_id = ?")
            .bind(seat.seat_id.as_bytes().as_slice())
            .execute(db.pool())
            .await
            .is_err()
    );
    assert_eq!(
        db.formed_federation_invite(&seat.seat_id).await.unwrap(),
        Some(invite)
    );
}

async fn open_db() -> (TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).await.unwrap();
    (dir, db)
}

async fn reload(db: &Db, seat_id: &SeatId) -> SeatRecord {
    db.list_seats()
        .await
        .unwrap()
        .into_iter()
        .find(|seat| &seat.facts.seat_id == seat_id)
        .unwrap()
}
