use fedi_decentralized_service_manifold_guardian::{
    FaId, FederationId, FederationSize, InviteCode, Locator, Plan, QuoteId,
};
use secp256k1::{Keypair, SECP256K1};

use super::*;
use crate::db::NewSeat;

fn keys() -> ImportedKeys {
    ImportedKeys::from_bytes(&[41; 32], &[42; 32])
}

fn key(byte: u8) -> Keypair {
    Keypair::from_seckey_slice(SECP256K1, &[byte; 32]).unwrap()
}

fn grant() -> SeatTransferGrant {
    let now = (now_ms() / 1000) as u64;
    SeatTransferGrant {
        ts: Timestamp(now),
        expires_at: Timestamp(now + 3600),
        fa_id: FaId(key(7).x_only_public_key().0),
        federation_id: FederationId("ab".repeat(32)),
        source: Locator::new(
            fedi_iroh_rpc::iroh::EndpointAddr::new(
                fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[5; 32]).public(),
            ),
            key(6).x_only_public_key().0,
        ),
        source_seat_id: SeatId::from(QuoteId([1; 32])),
        destination_service_pubkey: key(9).x_only_public_key().0,
        destination_seat_id: SeatId::from(QuoteId([2; 32])),
    }
}

async fn insert(db: &Db, grant: &SeatTransferGrant, source: bool) {
    crate::test_support::insert_test_seat(
        db,
        NewSeat {
            seat_id: if source {
                grant.source_seat_id.clone()
            } else {
                grant.destination_seat_id.clone()
            },
            fa_id: grant.fa_id,
            plan: Plan::InfiniteBestEffort { price_msats: 0 },
            federation_size: FederationSize(4),
            payment: None,
        },
    )
    .await;
}

async fn make_waiting_expired(db: &Db, grant: &SeatTransferGrant) {
    let seat = &grant.destination_seat_id;
    let mut record = db.incoming_transfer(seat).await.unwrap().unwrap();
    let mut expired = grant.clone();
    expired.ts.0 -= 7200;
    expired.expires_at.0 = (now_ms() / 1000) as u64 - 1;
    record.state = IncomingState::Waiting {
        grant: SignedRequest::create(&expired, &key(7)).unwrap(),
    };
    save(&mut db.pool().acquire().await.unwrap(), seat, &record)
        .await
        .unwrap();
}

#[tokio::test]
async fn expired_poll_is_not_a_spare_and_old_generation_cannot_accept() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(temp.path()).await.unwrap();
    let grant = grant();
    insert(&db, &grant, false).await;
    let signed = SignedRequest::create(&grant, &key(7)).unwrap();
    let first = db.admit_transfer(&signed, None).await.unwrap().unwrap();
    let replay = db.admit_transfer(&signed, None).await.unwrap().unwrap();
    assert_eq!(first.generation, replay.generation);
    make_waiting_expired(&db, &grant).await;
    assert!(
        !db.accept_transfer(
            &grant.destination_seat_id,
            first.generation,
            &first.binding,
            &keys(),
            &InviteCode("test".into())
        )
        .await
        .unwrap()
    );
    assert_eq!(
        db.transfer_phase(&grant.destination_seat_id).await.unwrap(),
        Some(TransferPhase::Expired)
    );
    let next = db.admit_transfer(&signed, None).await.unwrap().unwrap();
    assert_eq!(next.generation, first.generation + 1);
    assert!(
        !db.accept_transfer(
            &grant.destination_seat_id,
            first.generation,
            &first.binding,
            &keys(),
            &InviteCode("test".into())
        )
        .await
        .unwrap()
    );
    assert!(
        db.accept_transfer(
            &grant.destination_seat_id,
            next.generation,
            &next.binding,
            &keys(),
            &InviteCode("test".into())
        )
        .await
        .unwrap()
    );
    assert_eq!(
        db.transfer_phase(&grant.destination_seat_id).await.unwrap(),
        Some(TransferPhase::Imported)
    );
}

#[tokio::test]
async fn accepted_material_survives_restart_and_same_move_renewal() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(temp.path()).await.unwrap();
    let mut grant = grant();
    insert(&db, &grant, false).await;
    let signed = SignedRequest::create(&grant, &key(7)).unwrap();
    let first = db.admit_transfer(&signed, None).await.unwrap().unwrap();
    assert!(
        db.accept_transfer(
            &grant.destination_seat_id,
            first.generation,
            &first.binding,
            &keys(),
            &InviteCode("test".into())
        )
        .await
        .unwrap()
    );
    drop(db);
    let db = Db::open(temp.path()).await.unwrap();
    let accepted = db
        .incoming_transfer(&grant.destination_seat_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.phase(), TransferPhase::Imported);
    assert!(accepted.accepted().unwrap().same_as(&keys()));
    assert_eq!(
        db.formed_federation_invite(&grant.destination_seat_id)
            .await
            .unwrap(),
        Some(InviteCode("test".into())),
        "acceptance and the formed row commit together"
    );
    grant.expires_at.0 += 1;
    let renewal = SignedRequest::create(&grant, &key(7)).unwrap();
    assert_eq!(
        db.admit_transfer(&renewal, None)
            .await
            .unwrap()
            .unwrap()
            .generation,
        first.generation
    );
    grant.source_seat_id = SeatId::from(QuoteId([3; 32]));
    assert!(
        db.admit_transfer(&SignedRequest::create(&grant, &key(7)).unwrap(), None)
            .await
            .unwrap()
            .is_none()
    );
    let seat = &grant.destination_seat_id;
    assert!(!db.record_transfer_removal(seat).await.unwrap());
    assert!(db.incoming_transfer(seat).await.unwrap().is_some());
    assert!(db.retire_for_transfer(seat).await.unwrap());
    assert_eq!(
        db.transfer_phase(seat).await.unwrap(),
        Some(TransferPhase::Outgoing(OutgoingTransfer::Stopped))
    );
    assert!(db.incoming_transfer(seat).await.unwrap().is_some());
    assert!(db.record_transfer_removal(seat).await.unwrap());
    assert!(db.incoming_transfer(seat).await.unwrap().is_none());
    assert_eq!(
        db.outgoing_transfer(seat).await.unwrap(),
        Some(OutgoingTransfer::Removed)
    );
    assert_eq!(
        db.formed_federation_invite(seat).await.unwrap(),
        Some(InviteCode("test".into()))
    );
    assert!(!db.record_transfer_removal(seat).await.unwrap());
}

#[tokio::test]
async fn retirement_is_monotone_and_retains_formed_and_seat_facts() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(temp.path()).await.unwrap();
    let grant = grant();
    insert(&db, &grant, true).await;
    let seat = &grant.source_seat_id;
    assert!(!db.retire_for_transfer(seat).await.unwrap());
    db.record_formed(seat, &InviteCode("test-invite".into()))
        .await
        .unwrap();
    assert!(db.retire_for_transfer(seat).await.unwrap());
    assert!(!db.retire_for_transfer(seat).await.unwrap());
    assert!(db.record_transfer_removal(seat).await.unwrap());
    assert!(
        sqlx::query("UPDATE seat_transfer_out SET phase = 'stopped' WHERE quote_id = ?")
            .bind(seat.as_bytes().as_slice())
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM seat_transfer_out WHERE quote_id = ?")
            .bind(seat.as_bytes().as_slice())
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(db.formed_federation_invite(seat).await.unwrap().is_some());
    assert_eq!(db.list_seats().await.unwrap().len(), 1);
}

#[tokio::test]
async fn transfer_admission_cannot_race_dkg_or_revive_decommission() {
    let temp = tempfile::tempdir().unwrap();
    let db = Db::open(temp.path()).await.unwrap();
    let grant = grant();
    insert(&db, &grant, false).await;
    let signed = SignedRequest::create(&grant, &key(7)).unwrap();
    db.admit_transfer(&signed, None).await.unwrap().unwrap();
    assert!(
        sqlx::query("INSERT INTO dkg_inputs (quote_id, guardian_codes) VALUES (?, '[]')")
            .bind(grant.destination_seat_id.as_bytes().as_slice())
            .execute(db.pool())
            .await
            .is_err()
    );
    sqlx::query("INSERT INTO decommissioned_seats (quote_id, decommissioned_at_ms) VALUES (?, ?)")
        .bind(grant.destination_seat_id.as_bytes().as_slice())
        .bind(now_ms())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(db.admit_transfer(&signed, None).await.unwrap().is_none());
    assert!(
        !db.accept_transfer(
            &grant.destination_seat_id,
            1,
            &TransferMove::from(&grant),
            &keys(),
            &InviteCode("test".into())
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn additive_upgrade_preserves_native_seats_and_old_migrator_refuses_downgrade() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("fleet-manager.sqlite");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    // The exact migration runner an old binary executes before loading seats.
    let mut old = sqlx::migrate!("./migrations");
    old.migrations
        .to_mut()
        .retain(|migration| migration.version < 8);
    old.run(&pool).await.unwrap();
    let grant = grant();
    let plan = serde_json::to_string(&Plan::InfiniteBestEffort { price_msats: 0 }).unwrap();
    sqlx::query(
        "INSERT INTO seats (quote_id, seat_no, fi_id, plan, federation_size, created_at_ms) \
         VALUES (?, 9, ?, ?, 4, 123)",
    )
    .bind(grant.source_seat_id.as_bytes().as_slice())
    .bind(grant.fa_id.0.to_string())
    .bind(plan)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    let db = Db::open(temp.path()).await.unwrap();
    let seats = db.list_seats().await.unwrap();
    assert_eq!(seats.len(), 1);
    assert_eq!(seats[0].facts.seat_id, grant.source_seat_id);
    assert_eq!(seats[0].facts.seat_no.0, 9);
    assert_eq!(seats[0].facts.created_at_ms, 123);
    assert!(
        db.transfer_phase(&grant.source_seat_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        old.run(db.pool()).await,
        Err(sqlx::migrate::MigrateError::VersionMissing(8))
    ));
}
