//! Guardian transfer onto its new seat.

use super::seat_purchase::{
    DESTINATION, MOVED, destination_approval, formed_paid_client, restored_client, short_options,
    start_transfer, start_transfer_with, transfer_options,
};
use super::*;

fn consensus_field(client: &TestClient, state: &MgState, key: &str) -> String {
    let _ = client;
    let raw = state
        .meta_consensus_raw
        .lock()
        .expect("test lock")
        .clone()
        .unwrap();
    let fields: BTreeMap<String, serde_json::Value> = serde_json::from_slice(&raw).unwrap();
    fields[key].as_str().unwrap().to_owned()
}

/// The destination MG obtained the guardian of `MOVED` from its source.
fn guardian_arrives(state: &MgState) {
    state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(DESTINATION, ServiceStatus::Running);
    state
        .attested_peer_overrides
        .lock()
        .expect("test lock")
        .insert(DESTINATION, MOVED);
}

/// Verified holders of the formation's seats.
async fn formation_holders(client: &TestClient) -> BTreeSet<PublicKey> {
    let fa_id = client.fa_id().unwrap();
    let crate::db::FaRecovery::Formation(recovery) =
        client.inner.store.load_recovery(fa_id).await.unwrap()
    else {
        panic!("formation record");
    };
    recovery
        .seats
        .iter()
        .filter_map(|seat| seat.admission.holder())
        .collect()
}

#[tokio::test]
async fn a_guardian_moves_onto_its_new_seat() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let formation = formation(&client.status()).clone();
    let source_seat_id = formation.seats[MOVED].seat_id.clone().unwrap();
    let holders_before = formation_holders(&client).await;
    let moved_holder = test_mg_id(MOVED);
    assert!(holders_before.contains(&moved_holder));
    let source_account = guardian_fee_account(32 + MOVED as u8);
    let destination_account = guardian_fee_account(32 + DESTINATION as u8);
    assert!(
        consensus_field(&client, &mg_state, GUARDIAN_FEE_RECIPIENTS_META_FIELD_KEY)
            .contains(&serde_json::to_string(&source_account).unwrap())
    );

    let started = start_transfer(&client, &mg_state).await;
    let destination = started.destination_seat_id.clone().unwrap();

    assert_eq!(started.phase, GuardianTransferPhase::AwaitingSource);
    assert_eq!(started.source_seat_id, source_seat_id);
    assert_eq!(started.destination_mg_id, test_mg_id(DESTINATION));
    let pulls = mg_state.pull_requests.lock().expect("test lock").clone();
    assert_eq!(pulls.len(), 1);
    let (pulled_by, pull) = &pulls[0];
    assert_eq!(*pulled_by, DESTINATION);
    assert_eq!(pull.seat_id, destination);
    let grant = pull
        .grant
        .clone()
        .verify(Timestamp(test_now_secs()))
        .unwrap()
        .into_inner();
    assert_eq!(grant.source_seat_id, source_seat_id);
    assert_eq!(grant.source, locator(MOVED));
    assert_eq!(grant.destination_seat_id, destination);
    assert_eq!(
        grant.destination_service_pubkey,
        manager_key(DESTINATION).x_only_public_key().0
    );
    assert_eq!(Some(grant.expires_at), started.expires_at);
    assert_eq!(grant.expires_at.0 - grant.ts.0, 14 * 24 * 60 * 60);
    // Nothing moved yet: the backup keeps the move beside the old seats.
    let backup = client.inner.store.backup_payload().await.unwrap().payload;
    assert_eq!(backup.seats[MOVED].seat_id, source_seat_id);
    assert_eq!(
        backup.transfer.map(|transfer| transfer.destination.seat_id),
        Some(destination.clone())
    );

    guardian_arrives(&mg_state);
    let finished = client
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(finished.phase, GuardianTransferPhase::Completed);
    // Every guardian but the stopped source voted.
    let voters = mg_state
        .binding_votes
        .lock()
        .expect("test lock")
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    let expected_voters = (0..usize::from(MIN_FEDERATION_SIZE))
        .filter(|index| *index != MOVED)
        .chain([DESTINATION])
        .collect::<BTreeSet<_>>();
    assert_eq!(voters, expected_voters);
    // Consensus names the destination for the moved peer, and its fee share
    // moved to the destination's account.
    let directory = fedi_decentralized_domain::MgSeatBindings::parse_canonical(&consensus_field(
        &client,
        &mg_state,
        fedi_decentralized_domain::MG_SEAT_BINDINGS_META_FIELD_KEY,
    ))
    .unwrap();
    assert_eq!(
        directory.seat_bindings()[MOVED].attestation,
        test_attestation_for_peer(DESTINATION, MOVED).attestation
    );
    let recipients = consensus_field(&client, &mg_state, GUARDIAN_FEE_RECIPIENTS_META_FIELD_KEY);
    assert!(!recipients.contains(&serde_json::to_string(&source_account).unwrap()));
    assert!(recipients.contains(&serde_json::to_string(&destination_account).unwrap()));
    assert_eq!(
        mg_state.removed_seats.lock().expect("test lock").clone(),
        vec![(MOVED, source_seat_id)]
    );

    // The federation's seat list now names the destination, locally and in
    // the backup, and the backup no longer carries the move.
    let FaStatus::Restored(restored) = client.status() else {
        panic!("a transferred federation keeps the post-formation record");
    };
    assert_eq!(restored.freshness, FormationFreshness::Fresh);
    assert_eq!(restored.formation_id, formation.formation_id);
    assert_eq!(restored.seats.len(), usize::from(MIN_FEDERATION_SIZE));
    assert_eq!(restored.seats[MOVED].seat_id, destination);
    assert_eq!(restored.seats[MOVED].mg_identity, test_mg_id(DESTINATION));
    assert_eq!(
        restored.seats[0].seat_id,
        formation.seats[0].seat_id.clone().unwrap()
    );
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    // Selection still knows who holds every seat, and who held the moved
    // peer before.
    let (holders, past_mgs) = client.inner.store.restored_custody().await;
    assert_eq!(
        holders,
        holders_before
            .into_iter()
            .chain([test_mg_id(DESTINATION)])
            .collect()
    );
    assert_eq!(past_mgs, BTreeSet::from([test_mg_id(MOVED)]));
    let backup = client.inner.store.backup_payload().await.unwrap().payload;
    assert_eq!(backup.seats[MOVED].seat_id, destination);
    assert_eq!(backup.transfer, None);
    assert_eq!(
        backup.past_hosts,
        vec![crate::backup::FaBackupPastHost {
            mg_identity: test_mg_id(MOVED),
            holder: Some(moved_holder),
        }]
    );
    let restored = db::FaStore::new(MemDatabase::new().into_database());
    restored
        .restore_backup_payload_with_completion(TestIdentity::fa_id(), backup, None)
        .await
        .unwrap();
    assert_eq!(
        restored.restored_custody().await,
        (
            BTreeSet::from([moved_holder]),
            BTreeSet::from([test_mg_id(MOVED)])
        )
    );

    // Maintenance keeps working on the new seat list.
    client
        .update_federation_metadata(
            FederationMetadataUpdate::name("Moved Federation").unwrap(),
            transfer_options(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn an_offline_source_leaves_only_its_removal_open() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let destination = start_transfer(&client, &mg_state)
        .await
        .destination_seat_id
        .unwrap();
    guardian_arrives(&mg_state);
    mg_state
        .offline_indices
        .lock()
        .expect("test lock")
        .insert(MOVED);

    let waiting = client
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(waiting.phase, GuardianTransferPhase::RemovingSource);
    assert_eq!(
        client
            .guardian_transfer()
            .await
            .unwrap()
            .map(|transfer| transfer.phase),
        Some(GuardianTransferPhase::RemovingSource)
    );
    let FaStatus::Restored(restored) = client.status() else {
        panic!("the seat list moves before the source removal");
    };
    assert_eq!(restored.seats[MOVED].seat_id, destination);

    mg_state.offline_indices.lock().expect("test lock").clear();
    let finished = client
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.phase, GuardianTransferPhase::Completed);
    assert_eq!(mg_state.removed_seats.lock().expect("test lock").len(), 1);
}

#[tokio::test]
async fn an_unreachable_destination_keeps_the_source_seat() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    start_transfer(&client, &mg_state).await;
    guardian_arrives(&mg_state);
    mg_state
        .offline_indices
        .lock()
        .expect("test lock")
        .insert(MOVED);
    let waiting = client
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(waiting.phase, GuardianTransferPhase::RemovingSource);

    // The source is back, but the destination is gone since it ran the
    // guardian. Its earlier answer does not count.
    {
        let mut offline = mg_state.offline_indices.lock().expect("test lock");
        offline.clear();
        offline.insert(DESTINATION);
    }
    let still = client
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still.phase, GuardianTransferPhase::RemovingSource);
    assert!(mg_state.removed_seats.lock().expect("test lock").is_empty());

    mg_state.offline_indices.lock().expect("test lock").clear();
    let finished = client
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.phase, GuardianTransferPhase::Completed);
    assert_eq!(mg_state.removed_seats.lock().expect("test lock").len(), 1);
}

#[tokio::test]
async fn a_refused_vote_keeps_the_transfer_open_and_the_seats_unchanged() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let destination = start_transfer(&client, &mg_state)
        .await
        .destination_seat_id
        .unwrap();
    guardian_arrives(&mg_state);
    // Three voters run an MG without the verb, so the other four cannot
    // reach the threshold of five.
    for index in [0, 2, 3] {
        mg_state
            .meta_terminal_errors
            .lock()
            .expect("test lock")
            .insert(
                index,
                ManifoldGuardianError::UnsupportedVerb {
                    verb: "ReplaceSeatBinding".to_owned(),
                },
            );
    }

    let error = client
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap_err();

    assert!(
        matches!(error, FaError::MaintenanceRejected { .. }),
        "{error:?}"
    );
    assert_eq!(
        client
            .guardian_transfer()
            .await
            .unwrap()
            .map(|transfer| transfer.phase),
        Some(GuardianTransferPhase::PublishingBinding)
    );
    assert_eq!(
        formation(&client.status()).seats[MOVED].seat_id,
        client
            .guardian_transfer()
            .await
            .unwrap()
            .map(|transfer| transfer.source_seat_id)
    );
    assert!(mg_state.removed_seats.lock().expect("test lock").is_empty());
    assert_eq!(
        client
            .inner
            .store
            .backup_payload()
            .await
            .unwrap()
            .payload
            .transfer
            .map(|transfer| transfer.destination.seat_id),
        Some(destination)
    );
}

#[tokio::test]
async fn an_expired_grant_is_renewed_for_the_same_move() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let started = start_transfer(&client, &mg_state).await;
    let destination = started.destination_seat_id.clone().unwrap();
    // The grant expired. The source may have answered, so the destination
    // keeps waiting for a new grant.
    client.inner.store.expire_guardian_transfer_for_test().await;
    mg_state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(DESTINATION, ServiceStatus::TransferGrantExpired);

    let renewed = client
        .resume_guardian_transfer(short_options())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(renewed.phase, GuardianTransferPhase::AwaitingSource);
    let pulls = mg_state.pull_requests.lock().expect("test lock").clone();
    assert_eq!(pulls.len(), 2);
    let (first, second) = (&pulls[0].1, &pulls[1].1);
    assert_eq!(pulls[1].0, DESTINATION);
    let first = first
        .grant
        .clone()
        .verify(Timestamp(test_now_secs()))
        .unwrap()
        .into_inner();
    let second = second
        .grant
        .clone()
        .verify(Timestamp(test_now_secs()))
        .unwrap()
        .into_inner();
    assert_eq!(
        (
            second.source,
            second.source_seat_id,
            second.destination_seat_id
        ),
        (
            first.source,
            first.source_seat_id,
            first.destination_seat_id
        )
    );
    assert_eq!(second.federation_id, first.federation_id);
    assert!(started.expires_at.unwrap() <= second.expires_at);
    assert_eq!(renewed.expires_at, Some(second.expires_at));
    assert_eq!(renewed.destination_seat_id, Some(destination));

    // The renewed grant finishes the move.
    guardian_arrives(&mg_state);
    let finished = client
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.phase, GuardianTransferPhase::Completed);
}

#[tokio::test]
async fn a_destination_held_by_a_guardian_holder_is_not_bought() {
    let (_database, _payments, payment_state, mg_state, client) = formed_paid_client().await;
    let mut approval = destination_approval(PAYMENT_AMOUNT_MSATS);
    let guardian_holder = test_mg_id(MOVED + 1);
    assert!(formation_holders(&client).await.contains(&guardian_holder));
    approval.seat.holder = guardian_holder;
    let funded_before = payment_state.create_calls.load(Ordering::SeqCst);
    let quotes_before = mg_state.quote_calls.load(Ordering::SeqCst);

    let error = start_transfer_with(&client, &mg_state, approval)
        .await
        .unwrap_err();

    assert!(matches!(error, FaError::InvalidIntent(_)), "{error:?}");
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), quotes_before);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        funded_before
    );
    assert!(mg_state.pull_requests.lock().expect("test lock").is_empty());
}

#[tokio::test]
async fn a_destination_that_attests_another_guardian_is_refused() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    start_transfer(&client, &mg_state).await;
    guardian_arrives(&mg_state);
    mg_state
        .attested_peer_overrides
        .lock()
        .expect("test lock")
        .insert(DESTINATION, MOVED + 1);

    let error = client
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap_err();

    assert!(
        matches!(error, FaError::MaintenanceConsensusInvalid { .. }),
        "{error:?}"
    );
    assert!(mg_state.binding_votes.lock().expect("test lock").is_empty());
}

#[tokio::test]
async fn a_restored_fa_moves_a_guardian() {
    let (_, payment_state, mg_state, restored) = restored_client(Vec::new()).await;
    restored.resume().await.unwrap();
    let destination = start_transfer(&restored, &mg_state)
        .await
        .destination_seat_id
        .unwrap();
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 1);

    guardian_arrives(&mg_state);
    let finished = restored
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(finished.phase, GuardianTransferPhase::Completed);
    let FaStatus::Restored(snapshot) = restored.status() else {
        panic!("restored status");
    };
    assert_eq!(snapshot.seats[MOVED].seat_id, destination);
    assert_eq!(snapshot.seats[MOVED].locator, locator(DESTINATION));
}

/// A restored FA whose transfer of `MOVED` got its votes but whose run ended
/// before it saw consensus adopt them.
async fn transfer_adopted_but_unrecorded() -> (
    fedimint_core::db::Database,
    Arc<MgState>,
    TestClient,
    SeatId,
    crate::backup::FaBackupPayload,
) {
    let (database, _payment_state, mg_state, restored) = restored_client(Vec::new()).await;
    restored.resume().await.unwrap();
    let destination = start_transfer(&restored, &mg_state)
        .await
        .destination_seat_id
        .unwrap();
    let backup = restored.inner.store.backup_payload().await.unwrap().payload;
    guardian_arrives(&mg_state);
    mg_state
        .attest_with_fresh_timestamps
        .store(true, Ordering::SeqCst);
    mg_state
        .consensus_down_after_binding_votes
        .store(true, Ordering::SeqCst);

    let error = restored
        .resume_guardian_transfer(short_options())
        .await
        .unwrap_err();

    assert!(
        matches!(error, FaError::MaintenanceConvergence { .. }),
        "{error:?}"
    );
    assert_eq!(
        restored.guardian_transfer().await.unwrap().unwrap().phase,
        GuardianTransferPhase::PublishingBinding
    );
    mg_state
        .consensus_down_after_binding_votes
        .store(false, Ordering::SeqCst);
    (database, mg_state, restored, destination, backup)
}

#[tokio::test]
async fn a_reopened_fa_finishes_a_transfer_that_consensus_adopted() {
    let (database, mg_state, restored, destination, _backup) =
        transfer_adopted_but_unrecorded().await;
    let voted = mg_state
        .binding_votes
        .lock()
        .expect("test lock")
        .values()
        .next()
        .cloned()
        .unwrap();
    drop(restored);
    let (payments, _payment_state) = TestPayments::new();
    let reopened = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;
    let FaStatus::Restored(before) = reopened.status() else {
        panic!("restored status");
    };
    assert_eq!(before.freshness, FormationFreshness::Unsynced);

    // The destination now signs a different binding for the same peer. The
    // transfer still looks for the one it saved before the vote.
    let attestations = mg_state.attestation_calls.load(Ordering::SeqCst);
    let finished = reopened
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        mg_state.attestation_calls.load(Ordering::SeqCst),
        attestations
    );

    assert_eq!(finished.phase, GuardianTransferPhase::Completed);
    let directory = fedi_decentralized_domain::MgSeatBindings::parse_canonical(&consensus_field(
        &reopened,
        &mg_state,
        fedi_decentralized_domain::MG_SEAT_BINDINGS_META_FIELD_KEY,
    ))
    .unwrap();
    assert_eq!(directory.seat_bindings()[MOVED], voted);
    let FaStatus::Restored(after) = reopened.status() else {
        panic!("restored status");
    };
    assert_eq!(after.seats[MOVED].seat_id, destination);
    // The transfer checked only the moved seat, so the rest still needs a
    // reconciliation, which now accepts the destination.
    assert_eq!(after.freshness, FormationFreshness::Unsynced);
    reopened.resume().await.unwrap();
    let FaStatus::Restored(reconciled) = reopened.status() else {
        panic!("restored status");
    };
    assert_eq!(reconciled.freshness, FormationFreshness::Fresh);
}

#[tokio::test]
async fn a_reconciliation_that_adopts_an_open_move_keeps_the_backup_restorable() {
    let (_database, mg_state, restored, destination, _backup) =
        transfer_adopted_but_unrecorded().await;

    // An ordinary resume, not the transfer, sees consensus name the
    // destination first.
    restored.resume().await.unwrap();

    assert_eq!(
        restored.guardian_transfer().await.unwrap().unwrap().phase,
        GuardianTransferPhase::RemovingSource
    );
    let backup = restored.inner.store.backup_payload().await.unwrap().payload;
    assert_eq!(backup.seats[MOVED].seat_id, destination);
    assert_eq!(backup.transfer, None);

    // The backup this device now publishes restores.
    let (payments, _payment_state) = TestPayments::new();
    let replacement = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    replacement
        .inner
        .store
        .restore_backup_payload_with_completion(TestIdentity::fa_id(), backup, None)
        .await
        .unwrap();

    // The move still owes the source removal.
    let finished = restored
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.phase, GuardianTransferPhase::Completed);
    assert_eq!(mg_state.removed_seats.lock().expect("test lock").len(), 1);
}

#[tokio::test]
async fn a_published_binding_does_not_remove_the_source_of_a_lost_destination() {
    let (_database, mg_state, restored, _destination, _backup) =
        transfer_adopted_but_unrecorded().await;
    mg_state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(DESTINATION, ServiceStatus::DataLoss);

    let waiting = restored
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(waiting.phase, GuardianTransferPhase::RemovingSource);
    assert!(mg_state.removed_seats.lock().expect("test lock").is_empty());
}

#[tokio::test]
async fn a_backup_from_before_the_move_restores_onto_the_destination() {
    let (_database, mg_state, _lost, destination, backup) = transfer_adopted_but_unrecorded().await;
    assert_eq!(
        backup
            .transfer
            .as_ref()
            .map(|transfer| transfer.destination.seat_id.clone()),
        Some(destination.clone())
    );
    assert_ne!(backup.seats[MOVED].seat_id, destination);
    let (payments, _payment_state) = TestPayments::new();
    let replacement = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    let status = replacement
        .inner
        .store
        .restore_backup_payload_with_completion(TestIdentity::fa_id(), backup, None)
        .await
        .unwrap();
    replacement.inner.progress.send_replace(status);

    replacement.resume().await.unwrap();

    let FaStatus::Restored(restored) = replacement.status() else {
        panic!("restored status");
    };
    assert_eq!(restored.freshness, FormationFreshness::Fresh);
    assert_eq!(restored.seats[MOVED].seat_id, destination);
    assert_eq!(restored.seats[MOVED].mg_identity, test_mg_id(DESTINATION));
    // Consensus shows the move done, so no transfer opens again.
    assert_eq!(replacement.guardian_transfer().await.unwrap(), None);
    let backup = replacement
        .inner
        .store
        .backup_payload()
        .await
        .unwrap()
        .payload;
    assert_eq!(backup.seats[MOVED].seat_id, destination);
    assert_eq!(backup.transfer, None);
}

#[tokio::test]
async fn a_restored_backup_continues_the_same_move() {
    let (_database, _payments, _payment_state, mg_state, lost) = formed_paid_client().await;
    let started = start_transfer(&lost, &mg_state).await;
    let destination = started.destination_seat_id.clone().unwrap();
    let source_seat_id = started.source_seat_id.clone();
    let backup = lost.inner.store.backup_payload().await.unwrap().payload;
    drop(lost);
    let (payments, _payment_state) = TestPayments::new();
    let replacement = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    let status = replacement
        .inner
        .store
        .restore_backup_payload_with_completion(TestIdentity::fa_id(), backup, None)
        .await
        .unwrap();
    replacement.inner.progress.send_replace(status);

    // Consensus still names the source, so the restored FA opens the same
    // move again rather than giving up the paid seat.
    replacement.resume().await.unwrap();
    let reopened = replacement.guardian_transfer().await.unwrap().unwrap();
    assert_eq!(reopened.phase, GuardianTransferPhase::AwaitingSource);
    assert_eq!(reopened.source_seat_id, source_seat_id);
    assert_eq!(reopened.destination_seat_id, Some(destination.clone()));
    // The backup of the restored FA keeps the move.
    assert_eq!(
        replacement
            .inner
            .store
            .backup_payload()
            .await
            .unwrap()
            .payload
            .transfer
            .map(|transfer| transfer.destination.seat_id),
        Some(destination.clone())
    );

    // The first grant was lost with the device. The restored FA pulls the
    // same move with a grant it signed itself.
    mg_state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(DESTINATION, ServiceStatus::TransferGrantExpired);
    replacement
        .resume_guardian_transfer(short_options())
        .await
        .unwrap()
        .unwrap();
    let pulls = mg_state.pull_requests.lock().expect("test lock").clone();
    assert_eq!(pulls.len(), 2);
    let second = pulls[1]
        .1
        .grant
        .clone()
        .verify(Timestamp(test_now_secs()))
        .unwrap()
        .into_inner();
    assert_eq!(second.source_seat_id, source_seat_id);
    assert_eq!(second.source, locator(MOVED));
    assert_eq!(second.destination_seat_id, destination);

    guardian_arrives(&mg_state);
    let finished = replacement
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.phase, GuardianTransferPhase::Completed);
    let FaStatus::Restored(restored) = replacement.status() else {
        panic!("restored status");
    };
    assert_eq!(restored.seats[MOVED].seat_id, destination);
    assert_eq!(
        mg_state.removed_seats.lock().expect("test lock").clone(),
        vec![(MOVED, source_seat_id)]
    );
}

#[tokio::test]
async fn a_restored_backup_renews_the_move_with_its_callback() {
    let (_database, _payments, _payment_state, mg_state, lost) = formed_paid_client().await;
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: "https://push.example/hooks/hook-id/bearer-secret".to_owned(),
        idempotency_key: "transfer-complete".to_owned(),
    })
    .unwrap();
    mg_state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(DESTINATION, ServiceStatus::New);
    lost.start_guardian_transfer(
        MOVED as u16,
        destination_approval(PAYMENT_AMOUNT_MSATS),
        Some(payment_federation_id()),
        Some(callback.clone()),
        transfer_options(),
    )
    .await
    .unwrap();
    let keys = crate::backup::FaBackupKeys::derive(&lost.inner.ports.identity.scoped_root());
    let sealed = keys
        .seal(&lost.inner.store.backup_payload().await.unwrap().payload)
        .unwrap();
    drop(lost);
    let (payments, _payment_state) = TestPayments::new();
    let replacement = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    let status = replacement
        .inner
        .store
        .restore_backup_payload_with_completion(
            TestIdentity::fa_id(),
            keys.open(&sealed).unwrap(),
            None,
        )
        .await
        .unwrap();
    replacement.inner.progress.send_replace(status);
    replacement.resume().await.unwrap();

    // The grant expired while the source was away. The destination accepts
    // the renewal only with the callback the move started with.
    mg_state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(DESTINATION, ServiceStatus::TransferGrantExpired);
    let renewed = replacement
        .resume_guardian_transfer(short_options())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(renewed.phase, GuardianTransferPhase::AwaitingSource);
    let pulls = mg_state.pull_requests.lock().expect("test lock").clone();
    assert_eq!(pulls.len(), 2);
    assert_eq!(pulls[1].1.completion_callback, Some(callback));
}

/// Check what a crash right after a transfer write leaves: the source seat
/// is gone only once the destination runs its guardian, and the backup of the
/// moment restores.
async fn check_stopped_transfer(client: &TestClient, mg_state: &MgState) {
    let removed = !mg_state.removed_seats.lock().expect("test lock").is_empty();
    let destination_runs = mg_state
        .seat_statuses
        .lock()
        .expect("test lock")
        .get(&DESTINATION)
        == Some(&ServiceStatus::Running);
    assert!(!removed || destination_runs);
    let keys = crate::backup::FaBackupKeys::derive(&client.inner.ports.identity.scoped_root());
    let sealed = keys
        .seal(&client.inner.store.backup_payload().await.unwrap().payload)
        .unwrap();
    // Opening runs the same validation as a restore from the relays.
    let backup = keys.open(&sealed).unwrap();
    db::FaStore::new(MemDatabase::new().into_database())
        .restore_backup_payload_with_completion(TestIdentity::fa_id(), backup, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_transfer_stopped_after_any_write_finishes_on_resume() {
    let mut writes = 1;
    loop {
        let (database, payments, payment_state, mg_state, client) = formed_paid_client().await;
        let source_seat_id = formation(&client.status()).seats[MOVED]
            .seat_id
            .clone()
            .unwrap();
        let funded_before = payment_state.create_calls.load(Ordering::SeqCst);
        client.inner.store.stop_after_transfer_writes(writes);

        let started = start_transfer_with(
            &client,
            &mg_state,
            destination_approval(PAYMENT_AMOUNT_MSATS),
        )
        .await;
        check_stopped_transfer(&client, &mg_state).await;
        if started.is_ok() {
            guardian_arrives(&mg_state);
            let _ = client.resume_guardian_transfer(transfer_options()).await;
            check_stopped_transfer(&client, &mg_state).await;
        }
        if client.inner.store.transfer_write_stop_pending() {
            // The whole transfer made fewer writes than this run waited
            // for: every write had its turn. Buying and moving make 8.
            assert!(8 < writes, "the transfer made only {} writes", writes - 1);
            break;
        }
        drop(client);

        // A new process on the same database and wallet.
        let registry = TestRegistry::default();
        registry
            .candidates
            .lock()
            .expect("test lock")
            .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
        let reopened = open_client_with_registry(
            database,
            payments,
            mg_state.clone(),
            MgConfig::paid(),
            registry,
        )
        .await;
        let resumed = reopened
            .resume_guardian_transfer(short_options())
            .await
            .unwrap();
        if resumed.as_ref().map(|transfer| transfer.phase)
            == Some(GuardianTransferPhase::AwaitingSource)
        {
            check_stopped_transfer(&reopened, &mg_state).await;
            guardian_arrives(&mg_state);
            let finished = reopened
                .resume_guardian_transfer(transfer_options())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(finished.phase, GuardianTransferPhase::Completed);
        }

        // Every stop ends in the same move, paid once.
        assert_eq!(
            reopened.guardian_transfer().await.unwrap(),
            None,
            "{writes}"
        );
        assert_eq!(
            mg_state.removed_seats.lock().expect("test lock").clone(),
            vec![(MOVED, source_seat_id)],
            "{writes}"
        );
        assert_eq!(
            payment_state.create_calls.load(Ordering::SeqCst),
            funded_before + 1,
            "{writes}"
        );
        let FaStatus::Restored(restored) = reopened.status() else {
            panic!("a moved guardian keeps the post-formation record");
        };
        assert_eq!(
            restored.seats[MOVED].mg_identity,
            test_mg_id(DESTINATION),
            "{writes}"
        );
        check_stopped_transfer(&reopened, &mg_state).await;
        writes += 1;
    }
}

#[tokio::test]
async fn a_live_liquidity_request_blocks_a_transfer() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let quotes_before = mg_state.quote_calls.load(Ordering::SeqCst);
    let operation = stored_liquidity_operation(1);
    client
        .inner
        .store
        .insert_liquidity_operation(operation.clone())
        .await
        .unwrap();

    let error = start_transfer_with(
        &client,
        &mg_state,
        destination_approval(PAYMENT_AMOUNT_MSATS),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(&error, FaError::LiquidityOperationExists { operation_id }
            if *operation_id == operation.operation_id),
        "{error:?}"
    );
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), quotes_before);
    assert!(mg_state.pull_requests.lock().expect("test lock").is_empty());
}

#[tokio::test]
async fn an_open_transfer_blocks_a_liquidity_request() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    start_transfer(&client, &mg_state).await;
    let formation_id = formation(&client.status()).formation_id.clone();
    let connector =
        TestLiquidityConnector(TestLiquidityProviderState::new(client.inner.store.clone()));

    let error = client
        .start_liquidity_for_test(
            &formation_id,
            &liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string()),
            LiquidityRequestIntent::gateway(300_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .unwrap_err();

    assert!(matches!(error, FaError::InvalidIntent(_)), "{error:?}");
    assert!(connector.0.requests.lock().expect("test lock").is_empty());
    assert!(
        client
            .list_liquidity_operations(None, 10)
            .await
            .unwrap()
            .operations
            .is_empty()
    );
}

#[test]
fn a_rebind_moves_only_the_peer_entry_and_its_fee_share() {
    let attestations = (0..usize::from(MIN_FEDERATION_SIZE))
        .map(test_attestation)
        .collect::<Vec<_>>();
    let directory = fedi_decentralized_domain::MgSeatBindings::new(attestations.clone())
        .unwrap()
        .canonical_string()
        .unwrap();
    let mut recipients = (0..usize::from(MIN_FEDERATION_SIZE))
        .map(|index| {
            (
                guardian_fee_account(32 + index as u8),
                GUARDIAN_GUARDIAN_FEE_WEIGHT,
            )
        })
        .chain([
            (guardian_fee_account(30), FA_GUARDIAN_FEE_WEIGHT),
            (guardian_fee_account(31), GUARDIAN_VERIFICATION_FEE_WEIGHT),
        ])
        .map(|(account, weight)| {
            GuardianFeeRecipient::new(
                fedi_decentralized_service_manifold_guardian::GuardianFeeAccount::try_from(account)
                    .unwrap(),
                weight,
            )
        })
        .collect::<Vec<_>>();
    recipients.sort_by_key(|recipient| recipient.account.as_account().id());
    let recipient_list = canonical_guardian_fee_recipient_list(&recipients).unwrap();
    let moved = test_attestation_for_peer(DESTINATION, MOVED);

    let fedi_decentralized_service_manifold_guardian::ReboundSeat {
        seat_bindings: new_directory,
        recipients: new_recipients,
    } = rebind_transferred_seat(&directory, &recipient_list, &moved).unwrap();

    let mut expected_bindings = attestations;
    expected_bindings[MOVED] = moved.clone();
    assert_eq!(
        new_directory,
        fedi_decentralized_domain::MgSeatBindings::new(expected_bindings)
            .unwrap()
            .canonical_string()
            .unwrap()
    );
    let mut expected_recipients = recipients
        .into_iter()
        .map(|mut recipient| {
            if recipient.account.as_account() == &guardian_fee_account(32 + MOVED as u8) {
                recipient.account =
                    fedi_decentralized_service_manifold_guardian::GuardianFeeAccount::try_from(
                        guardian_fee_account(32 + DESTINATION as u8),
                    )
                    .unwrap();
            }
            recipient
        })
        .collect::<Vec<_>>();
    expected_recipients.sort_by_key(|recipient| recipient.account.as_account().id());
    assert_eq!(
        new_recipients,
        canonical_guardian_fee_recipient_list(&expected_recipients).unwrap()
    );
    // A second application changes nothing.
    assert_eq!(
        rebind_transferred_seat(&new_directory, &new_recipients, &moved).unwrap(),
        fedi_decentralized_service_manifold_guardian::ReboundSeat {
            seat_bindings: new_directory,
            recipients: new_recipients,
        }
    );
}

/// A device lost before its backup names the transfer restores an older
/// backup without it. Then it could start another move of the same guardian.
/// So no grant leaves until a publish quorum of relays confirms the backup.
#[tokio::test]
async fn no_grant_leaves_before_the_relays_hold_the_transfer() {
    let (database, payments, _payment_state, mg_state, formed) = formed_paid_client().await;
    let keys = crate::backup::FaBackupKeys::derive(&formed.inner.ports.identity.scoped_root());
    let published = keys
        .seal(&formed.inner.store.backup_payload().await.unwrap().payload)
        .unwrap();
    drop(formed);
    let relays = [
        "wss://first.example",
        "wss://second.example",
        "wss://third.example",
    ]
    .map(|relay| nostr_sdk::RelayUrl::parse(relay).unwrap());
    let client = open_client_with_backup_publication(
        database,
        payments,
        mg_state.clone(),
        crate::backup_worker::BackupPublication::new(&relays, 2),
    )
    .await;
    mg_state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(DESTINATION, ServiceStatus::New);
    let confirm = async |relay: &nostr_sdk::RelayUrl| {
        let prepared = client.inner.store.backup_payload().await.unwrap();
        assert!(prepared.payload.transfer.is_some());
        client
            .inner
            .store
            .record_backup_confirmation(
                &relay.to_string(),
                crate::db::BackupRelayConfirmation {
                    document_hash: prepared.document_hash,
                    generation: prepared.payload.snapshot_generation,
                    event_id: relay.to_string(),
                    confirmed_at_secs: test_now_secs(),
                },
            )
            .await
            .unwrap();
    };

    let started = client
        .start_guardian_transfer(
            MOVED as u16,
            destination_approval(PAYMENT_AMOUNT_MSATS),
            Some(payment_federation_id()),
            None,
            short_options(),
        )
        .await
        .unwrap();
    assert_eq!(started.phase, GuardianTransferPhase::AwaitingSource);
    confirm(&relays[0]).await;
    let resumed = client
        .resume_guardian_transfer(short_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.phase, GuardianTransferPhase::AwaitingSource);
    assert!(
        mg_state.pull_requests.lock().expect("test lock").is_empty(),
        "one of three relays is less than a publish quorum"
    );

    // The device is lost now: the restore finds no transfer to finish, and
    // no grant left that another move would race.
    let (payments, _payment_state) = TestPayments::new();
    let replacement = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    replacement
        .inner
        .store
        .restore_backup_payload_with_completion(
            TestIdentity::fa_id(),
            keys.open(&published).unwrap(),
            None,
        )
        .await
        .unwrap();
    assert!(replacement.guardian_transfer().await.unwrap().is_none());
    drop(replacement);

    confirm(&relays[2]).await;
    client
        .resume_guardian_transfer(short_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mg_state.pull_requests.lock().expect("test lock").len(), 1);
}
