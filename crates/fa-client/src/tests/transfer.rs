//! Guardian transfer onto a spare seat.

use super::seat_pool::{SPARE_INDEX, formed_paid_client, restored_client, spare_approval};
use super::*;

/// The guardian these tests move.
const MOVED: usize = 1;

fn transfer_options() -> MaintenanceRunOptions {
    MaintenanceRunOptions::new(MaintenanceRunOptionsConfig {
        poll_interval: Duration::from_millis(10),
        run_timeout: Duration::from_secs(5),
        request_timeout: Duration::from_secs(2),
    })
    .unwrap()
}

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
        .insert(SPARE_INDEX, ServiceStatus::Running);
    state
        .attested_peer_overrides
        .lock()
        .expect("test lock")
        .insert(SPARE_INDEX, MOVED);
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

async fn buy_spare(client: &TestClient, state: &MgState) -> SpareSeat {
    let spare = client
        .buy_spare_seat(spare_approval(PAYMENT_AMOUNT_MSATS), long_request_options())
        .await
        .unwrap();
    // A bought seat is unformed until a guardian arrives.
    state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(SPARE_INDEX, ServiceStatus::New);
    spare
}

#[tokio::test]
async fn a_guardian_moves_onto_a_spare_seat() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let spare = buy_spare(&client, &mg_state).await;
    let formation = formation(&client.status()).clone();
    let source_seat_id = formation.seats[MOVED].seat_id.clone().unwrap();
    let holders_before = formation_holders(&client).await;
    let moved_holder = test_mg_id(MOVED);
    assert!(holders_before.contains(&moved_holder));
    let source_account = guardian_fee_account(32 + MOVED as u8);
    let destination_account = guardian_fee_account(32 + SPARE_INDEX as u8);
    assert!(
        consensus_field(&client, &mg_state, GUARDIAN_FEE_RECIPIENTS_META_FIELD_KEY)
            .contains(&serde_json::to_string(&source_account).unwrap())
    );

    let started = client
        .start_guardian_transfer(
            MOVED as u16,
            spare.seat_id.clone(),
            None,
            transfer_options(),
        )
        .await
        .unwrap();

    assert_eq!(started.phase, GuardianTransferPhase::AwaitingSource);
    assert_eq!(started.source_seat_id, source_seat_id);
    assert_eq!(started.destination, spare);
    let pulls = mg_state.pull_requests.lock().expect("test lock").clone();
    assert_eq!(pulls.len(), 1);
    let (pulled_by, pull) = &pulls[0];
    assert_eq!(*pulled_by, SPARE_INDEX);
    assert_eq!(pull.seat_id, spare.seat_id);
    let grant = pull
        .grant
        .clone()
        .verify(Timestamp(test_now_secs()))
        .unwrap()
        .into_inner();
    assert_eq!(grant.source_seat_id, source_seat_id);
    assert_eq!(grant.source, locator(MOVED));
    assert_eq!(grant.destination_seat_id, spare.seat_id);
    assert_eq!(
        grant.destination_service_pubkey,
        manager_key(SPARE_INDEX).x_only_public_key().0
    );
    assert_eq!(grant.expires_at, started.expires_at);
    assert_eq!(grant.expires_at.0 - grant.ts.0, 14 * 24 * 60 * 60);
    // Nothing moved yet: the spare stays in the pool and the backup.
    assert_eq!(client.spare_seats().await.unwrap(), vec![spare.clone()]);

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
        .chain([SPARE_INDEX])
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
        test_attestation_for_peer(SPARE_INDEX, MOVED).attestation
    );
    let recipients = consensus_field(&client, &mg_state, GUARDIAN_FEE_RECIPIENTS_META_FIELD_KEY);
    assert!(!recipients.contains(&serde_json::to_string(&source_account).unwrap()));
    assert!(recipients.contains(&serde_json::to_string(&destination_account).unwrap()));
    assert_eq!(
        mg_state.removed_seats.lock().expect("test lock").clone(),
        vec![(MOVED, source_seat_id)]
    );

    // The federation's seat list now names the destination, locally and in
    // the backup, and the spare left the pool.
    let FaStatus::Restored(restored) = client.status() else {
        panic!("a transferred federation keeps the post-formation record");
    };
    assert_eq!(restored.freshness, FormationFreshness::Fresh);
    assert_eq!(restored.formation_id, formation.formation_id);
    assert_eq!(restored.seats.len(), usize::from(MIN_FEDERATION_SIZE));
    assert_eq!(restored.seats[MOVED].seat_id, spare.seat_id);
    assert_eq!(restored.seats[MOVED].mg_identity, spare.mg_id);
    assert_eq!(
        restored.seats[0].seat_id,
        formation.seats[0].seat_id.clone().unwrap()
    );
    assert!(client.spare_seats().await.unwrap().is_empty());
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    // Selection still knows who holds every seat.
    assert_eq!(
        client.inner.store.restored_seat_holders().await,
        holders_before
            .into_iter()
            .filter(|holder| *holder != moved_holder)
            .chain([test_mg_id(SPARE_INDEX)])
            .collect()
    );
    let backup = client.inner.store.backup_payload().await.unwrap().payload;
    assert_eq!(backup.seats[MOVED].seat_id, spare.seat_id);
    assert!(backup.spare_seats.is_empty());

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
    let spare = buy_spare(&client, &mg_state).await;
    client
        .start_guardian_transfer(
            MOVED as u16,
            spare.seat_id.clone(),
            None,
            transfer_options(),
        )
        .await
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
    assert_eq!(restored.seats[MOVED].seat_id, spare.seat_id);

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
    let spare = buy_spare(&client, &mg_state).await;
    client
        .start_guardian_transfer(
            MOVED as u16,
            spare.seat_id.clone(),
            None,
            transfer_options(),
        )
        .await
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
    assert_eq!(client.spare_seats().await.unwrap(), vec![spare]);
}

#[tokio::test]
async fn an_expired_grant_returns_the_spare_to_the_pool() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let spare = buy_spare(&client, &mg_state).await;
    client
        .start_guardian_transfer(
            MOVED as u16,
            spare.seat_id.clone(),
            None,
            transfer_options(),
        )
        .await
        .unwrap();
    // The source never answered, so the destination gave up and is new again.
    client.inner.store.expire_guardian_transfer_for_test().await;
    mg_state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(SPARE_INDEX, ServiceStatus::New);

    let expired = client
        .resume_guardian_transfer(transfer_options())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expired.phase, GuardianTransferPhase::Expired);
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    assert_eq!(client.spare_seats().await.unwrap(), vec![spare]);
    assert_eq!(mg_state.pull_requests.lock().expect("test lock").len(), 1);
    assert!(mg_state.binding_votes.lock().expect("test lock").is_empty());
    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
}

#[tokio::test]
async fn a_destination_that_attests_another_guardian_is_refused() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let spare = buy_spare(&client, &mg_state).await;
    client
        .start_guardian_transfer(
            MOVED as u16,
            spare.seat_id.clone(),
            None,
            transfer_options(),
        )
        .await
        .unwrap();
    guardian_arrives(&mg_state);
    mg_state
        .attested_peer_overrides
        .lock()
        .expect("test lock")
        .insert(SPARE_INDEX, MOVED + 1);

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
    let spare = restored
        .buy_spare_seat(spare_approval(PAYMENT_AMOUNT_MSATS), long_request_options())
        .await
        .unwrap();
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 1);
    mg_state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(SPARE_INDEX, ServiceStatus::New);
    restored.resume().await.unwrap();

    restored
        .start_guardian_transfer(
            MOVED as u16,
            spare.seat_id.clone(),
            None,
            transfer_options(),
        )
        .await
        .unwrap();
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
    assert_eq!(snapshot.seats[MOVED].seat_id, spare.seat_id);
    assert_eq!(snapshot.seats[MOVED].locator, locator(SPARE_INDEX));
}

/// A run that gives up soon, for steps that cannot finish.
fn short_options() -> MaintenanceRunOptions {
    MaintenanceRunOptions::new(MaintenanceRunOptionsConfig {
        poll_interval: Duration::from_millis(10),
        run_timeout: Duration::from_millis(300),
        request_timeout: Duration::from_millis(200),
    })
    .unwrap()
}

/// A restored FA with a spare, whose transfer of `MOVED` got its votes but
/// whose run ended before it saw consensus adopt them.
async fn transfer_adopted_but_unrecorded() -> (
    fedimint_core::db::Database,
    Arc<MgState>,
    TestClient,
    SpareSeat,
    crate::backup::FaBackupPayload,
) {
    let (database, _payment_state, mg_state, restored) = restored_client(Vec::new()).await;
    let spare = buy_spare(&restored, &mg_state).await;
    restored.resume().await.unwrap();
    restored
        .start_guardian_transfer(
            MOVED as u16,
            spare.seat_id.clone(),
            None,
            transfer_options(),
        )
        .await
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
    (database, mg_state, restored, spare, backup)
}

#[tokio::test]
async fn a_reopened_fa_finishes_a_transfer_that_consensus_adopted() {
    let (database, mg_state, restored, spare, _backup) = transfer_adopted_but_unrecorded().await;
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
    assert_eq!(after.seats[MOVED].seat_id, spare.seat_id);
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
async fn a_backup_from_before_the_move_restores_onto_the_destination() {
    let (_database, mg_state, _lost, spare, backup) = transfer_adopted_but_unrecorded().await;
    assert_eq!(backup.spare_seats.len(), 1);
    assert_ne!(backup.seats[MOVED].seat_id, spare.seat_id);
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
    assert_eq!(restored.seats[MOVED].seat_id, spare.seat_id);
    assert_eq!(restored.seats[MOVED].mg_identity, spare.mg_id);
    assert!(replacement.spare_seats().await.unwrap().is_empty());
    let backup = replacement
        .inner
        .store
        .backup_payload()
        .await
        .unwrap()
        .payload;
    assert_eq!(backup.seats[MOVED].seat_id, spare.seat_id);
    assert!(backup.spare_seats.is_empty());
}

#[tokio::test]
async fn a_live_liquidity_request_blocks_a_transfer() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let spare = buy_spare(&client, &mg_state).await;
    let operation = stored_liquidity_operation(1);
    client
        .inner
        .store
        .insert_liquidity_operation(operation.clone())
        .await
        .unwrap();

    let error = client
        .start_guardian_transfer(
            MOVED as u16,
            spare.seat_id.clone(),
            None,
            transfer_options(),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(&error, FaError::LiquidityOperationExists { operation_id }
            if *operation_id == operation.operation_id),
        "{error:?}"
    );
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    assert!(mg_state.pull_requests.lock().expect("test lock").is_empty());
}

#[tokio::test]
async fn an_open_transfer_blocks_a_liquidity_request() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let spare = buy_spare(&client, &mg_state).await;
    client
        .start_guardian_transfer(
            MOVED as u16,
            spare.seat_id.clone(),
            None,
            transfer_options(),
        )
        .await
        .unwrap();
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
    let moved = test_attestation_for_peer(SPARE_INDEX, MOVED);

    let (new_directory, new_recipients) =
        rebind_transferred_seat(&directory, &recipient_list, &moved).unwrap();

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
                        guardian_fee_account(32 + SPARE_INDEX as u8),
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
        (new_directory, new_recipients)
    );
}
