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

/// Verified holders of the formed federation's seats.
async fn formation_holders(client: &TestClient) -> BTreeSet<PublicKey> {
    client.inner.store.formed_custody().await.0
}

#[tokio::test]
async fn a_guardian_moves_onto_its_new_seat() {
    let (database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let formation = formed(&client.status()).clone();
    let source_seat_id = formation.seats[MOVED].seat_id.clone();
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
    // Nothing moved yet: the backup names the source seat.
    let backup = client.inner.store.backup_payload().await.unwrap().payload;
    assert_eq!(backup.seats[MOVED].seat_id, source_seat_id);

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
    let FaStatus::Formed(restored) = client.status() else {
        panic!("a transferred federation keeps its formed record");
    };
    assert_eq!(restored.freshness, FormationFreshness::Fresh);
    assert_eq!(restored.formation_id, formation.formation_id);
    assert_eq!(restored.seats.len(), usize::from(MIN_FEDERATION_SIZE));
    assert_eq!(restored.seats[MOVED].seat_id, destination);
    assert_eq!(restored.seats[MOVED].mg_identity, test_mg_id(DESTINATION));
    assert_eq!(
        restored.seats[0].seat_id,
        formation.seats[0].seat_id.clone()
    );
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    // Selection still knows who holds every seat, and who held the moved
    // peer before.
    let (holders, past_mgs) = client.inner.store.formed_custody().await;
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
        restored.formed_custody().await,
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

    // The move changed the directory and fee recipients this device formed
    // with, and a reopened FA still reconciles the federation.
    drop(client);
    tokio::task::yield_now().await;
    let (payments, _payment_state) = TestPayments::new();
    let reopened = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;
    reopened.resume().await.unwrap();
    assert_eq!(
        formed(&reopened.status()).freshness,
        FormationFreshness::Fresh
    );
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
    let FaStatus::Formed(restored) = client.status() else {
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
        Some(formed(&client.status()).seats[MOVED].seat_id.clone()),
        client
            .guardian_transfer()
            .await
            .unwrap()
            .map(|transfer| transfer.source_seat_id)
    );
    assert!(mg_state.removed_seats.lock().expect("test lock").is_empty());
    assert_ne!(
        client
            .inner
            .store
            .backup_payload()
            .await
            .unwrap()
            .payload
            .seats[MOVED]
            .seat_id,
        destination
    );
}

#[tokio::test]
async fn a_destination_that_lost_the_grant_gets_a_grant_for_the_same_move() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let started = start_transfer(&client, &mg_state).await;
    let destination = started.destination_seat_id.clone().unwrap();
    // A destination MG restored from its backup forgets the grant and
    // reports `New` again. The test MG, like the real one, takes only a
    // grant for the same move.
    mg_state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(DESTINATION, ServiceStatus::New);

    let resumed = client
        .resume_guardian_transfer(short_options())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(resumed.phase, GuardianTransferPhase::AwaitingSource);
    assert_eq!(resumed.destination_seat_id, Some(destination));
    let pulls = mg_state.pull_requests.lock().expect("test lock").clone();
    assert_eq!(pulls.len(), 2);
    assert_eq!(pulls[1].0, DESTINATION);

    guardian_arrives(&mg_state);
    let finished = client
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.phase, GuardianTransferPhase::Completed);
}

/// The destination ran the guardian, then restored a backup from before
/// its import and reports `New`. The stopped source still holds the
/// guardian, so the FA pulls it again before it removes the source, with the
/// move's completion callback.
async fn check_destination_pulls_again(
    client: &TestClient,
    mg_state: &MgState,
    callback: Option<DkgCompletionCallback>,
) {
    let phase = client.guardian_transfer().await.unwrap().unwrap().phase;
    let source_seat_id = client
        .guardian_transfer()
        .await
        .unwrap()
        .unwrap()
        .source_seat_id;
    let pulls_before = mg_state.pull_requests.lock().expect("test lock").len();
    mg_state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(DESTINATION, ServiceStatus::New);

    // Until the destination runs the guardian again, the source stays.
    let waiting = client
        .resume_guardian_transfer(short_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(waiting.phase, phase);
    let pulls = mg_state.pull_requests.lock().expect("test lock").clone();
    assert_eq!(pulls.len(), pulls_before + 1);
    assert_eq!(pulls.last().unwrap().1.completion_callback, callback);
    assert!(mg_state.removed_seats.lock().expect("test lock").is_empty());

    guardian_arrives(mg_state);
    let finished = client
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.phase, GuardianTransferPhase::Completed);
    assert_eq!(
        mg_state.removed_seats.lock().expect("test lock").clone(),
        vec![(MOVED, source_seat_id)]
    );
}

#[tokio::test]
async fn a_destination_that_lost_the_guardian_before_the_vote_pulls_it_again() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
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
    client
        .start_guardian_transfer(
            MOVED as u16,
            destination_approval(PAYMENT_AMOUNT_MSATS),
            Some(payment_federation_id()),
            Some(callback.clone()),
            transfer_options(),
        )
        .await
        .unwrap();
    guardian_arrives(&mg_state);
    // The transfer saw the guardian run, then the vote failed.
    for index in [0, 2, 3] {
        mg_state
            .meta_terminal_errors
            .lock()
            .expect("test lock")
            .insert(index, ManifoldGuardianError::MetaValueInvalid);
    }
    client
        .resume_guardian_transfer(short_options())
        .await
        .unwrap_err();
    mg_state
        .meta_terminal_errors
        .lock()
        .expect("test lock")
        .clear();
    assert_eq!(
        client.guardian_transfer().await.unwrap().unwrap().phase,
        GuardianTransferPhase::PublishingBinding
    );

    check_destination_pulls_again(&client, &mg_state, Some(callback)).await;
}

#[tokio::test]
async fn a_destination_that_lost_the_guardian_before_the_removal_pulls_it_again() {
    let (_database, mg_state, restored, _destination, _backup) =
        transfer_adopted_but_unrecorded().await;
    // Reconciliation sees consensus name the destination: only the removal
    // is left.
    restored.resume().await.unwrap();
    assert_eq!(
        restored.guardian_transfer().await.unwrap().unwrap().phase,
        GuardianTransferPhase::RemovingSource
    );

    check_destination_pulls_again(&restored, &mg_state, None).await;
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
    let FaStatus::Formed(snapshot) = restored.status() else {
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
    // The backup names the move once its binding is voted.
    let backup = restored.inner.store.backup_payload().await.unwrap().payload;
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
    let FaStatus::Formed(before) = reopened.status() else {
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
    let FaStatus::Formed(after) = reopened.status() else {
        panic!("restored status");
    };
    assert_eq!(after.seats[MOVED].seat_id, destination);
    // The transfer checked only the moved seat, so the rest still needs a
    // reconciliation, which now accepts the destination.
    assert_eq!(after.freshness, FormationFreshness::Unsynced);
    reopened.resume().await.unwrap();
    let FaStatus::Formed(reconciled) = reopened.status() else {
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

    let error = restored
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap_err();

    assert!(
        matches!(&error, FaError::Mg(failure) if failure.cause == MgCause::WrongState(ServiceStatus::DataLoss)),
        "{error:?}"
    );
    // The destination is checked before the rebind too, so the transfer
    // stays where it stopped.
    assert_eq!(
        restored.guardian_transfer().await.unwrap().unwrap().phase,
        GuardianTransferPhase::PublishingBinding
    );
    assert!(mg_state.removed_seats.lock().expect("test lock").is_empty());
}

/// Accepted: a device lost after the vote reached consensus but before the
/// relays held the new seat list restores the old seat list. Consensus names
/// the destination, and the backup has no record of it, so the restored FA
/// cannot reconcile the federation.
#[tokio::test]
async fn a_backup_from_before_an_adopted_move_does_not_reconcile() {
    let (_database, mg_state, _lost, destination, backup) = transfer_adopted_but_unrecorded().await;
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

    let error = replacement.resume().await.unwrap_err();

    assert!(
        matches!(&error, FaError::InvalidManifoldGuardians(reason) if reason.contains("do not match")),
        "{error:?}"
    );
    let FaStatus::Formed(restored) = replacement.status() else {
        panic!("restored status");
    };
    assert_ne!(restored.freshness, FormationFreshness::Fresh);
    assert_eq!(replacement.guardian_transfer().await.unwrap(), None);
}

/// Accepted: a device lost before the vote restores a backup without the
/// move. Consensus still names the source, so the restored FA reconciles
/// and knows of no transfer, while the destination already runs the
/// guardian.
#[tokio::test]
async fn a_backup_from_before_the_vote_restores_without_the_move() {
    let (_database, _payments, _payment_state, mg_state, lost) = formed_paid_client().await;
    let source_seat_id = start_transfer(&lost, &mg_state).await.source_seat_id;
    guardian_arrives(&mg_state);
    let backup = lost.inner.store.backup_payload().await.unwrap().payload;
    assert_eq!(backup.seats[MOVED].seat_id, source_seat_id);
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

    replacement.resume().await.unwrap();

    let FaStatus::Formed(restored) = replacement.status() else {
        panic!("restored status");
    };
    assert_eq!(restored.freshness, FormationFreshness::Fresh);
    assert_eq!(restored.seats[MOVED].seat_id, source_seat_id);
    assert_eq!(replacement.guardian_transfer().await.unwrap(), None);
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
        let source_seat_id = formed(&client.status()).seats[MOVED].seat_id.clone();
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
        let FaStatus::Formed(restored) = reopened.status() else {
            panic!("a moved guardian keeps its formed record");
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
    let formation_id = formed(&client.status()).formation_id.clone();
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

/// The newest policy does not enable transfer, so the FA neither buys a seat
/// nor opens a transfer: the MG would refuse to start it.
#[tokio::test]
async fn a_policy_without_transfer_buys_no_seat() {
    let (_database, _payments, payment_state, mg_state, client) = formed_paid_client().await;
    client
        .inner
        .ports
        .registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_raw_event(
            test_now_secs() + 1,
            serde_json::json!({
                "version": 1,
                "fman_version": "0.1.0",
                "federations": [PAYMENT_INVITE],
                "telemetry_registration_url":
                    "https://push.fedi.example/v1/telemetry/registrations",
            })
            .to_string(),
        ));
    let quotes_before = mg_state.quote_calls.load(Ordering::SeqCst);
    let funded_before = payment_state.create_calls.load(Ordering::SeqCst);

    let error = start_transfer_with(
        &client,
        &mg_state,
        destination_approval(PAYMENT_AMOUNT_MSATS),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, FaError::GuardianTransferDisabled),
        "{error:?}"
    );
    assert_eq!(error.code(), FaErrorCode::CapabilityUnavailable);
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), quotes_before);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        funded_before
    );
    assert!(mg_state.pull_requests.lock().expect("test lock").is_empty());
}
