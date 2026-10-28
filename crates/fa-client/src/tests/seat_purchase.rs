//! The purchase stage of a guardian transfer: buying its destination seat.

use super::*;
use crate::selection::{ApprovedMgSeat, TransferDestinationApproval};
use crate::transfer::TransferStage;

/// The first MG outside the formation's pinned seats.
pub(super) const DESTINATION: usize = MIN_FEDERATION_SIZE as usize;

/// The guardian these tests move.
pub(super) const MOVED: usize = 1;

pub(super) async fn formed_paid_client() -> (
    fedimint_core::db::Database,
    TestPayments,
    Arc<PaymentState>,
    Arc<MgState>,
    TestClient,
) {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    let client = open_client_with_registry(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::paid(),
        registry,
    )
    .await;
    client
        .pay_and_create(
            intent(),
            selection_approval(u64::from(MIN_FEDERATION_SIZE) * PAYMENT_AMOUNT_MSATS),
            payment_federation_id(),
            long_request_options(),
        )
        .await
        .unwrap();
    formed(&client.status());
    (database, payments, payment_state, mg_state, client)
}

/// The id of the federation every formed test client forms.
pub(super) fn formed_federation_id() -> FederationId {
    FederationId(
        formation::invite_federation_id(&test_invite(0))
            .unwrap()
            .to_string(),
    )
}

pub(super) fn destination_approval(max_msats: u64) -> TransferDestinationApproval {
    TransferDestinationApproval {
        federation_id: formed_federation_id(),
        verifier_provenance: test_peer_badge_verifier().provenance(),
        fedimintd_dkg_version: fedimintd_version().dkg_version(),
        seat: ApprovedMgSeat {
            mg_id: test_mg_id(DESTINATION),
            holder: test_mg_id(DESTINATION),
            locator: locator(DESTINATION),
        },
        // The tests approve exactly the advertised price.
        advertised_price_msats: max_msats,
        max_msats,
        valid_until: Timestamp(test_now_secs() + 120),
    }
}

pub(super) fn transfer_options() -> MaintenanceRunOptions {
    MaintenanceRunOptions::new(MaintenanceRunOptionsConfig {
        poll_interval: Duration::from_millis(10),
        run_timeout: Duration::from_secs(5),
        request_timeout: Duration::from_secs(2),
    })
    .unwrap()
}

/// A run that gives up soon, for steps that cannot finish.
pub(super) fn short_options() -> MaintenanceRunOptions {
    MaintenanceRunOptions::new(MaintenanceRunOptionsConfig {
        poll_interval: Duration::from_millis(10),
        run_timeout: Duration::from_millis(300),
        request_timeout: Duration::from_millis(200),
    })
    .unwrap()
}

/// Start to move `MOVED` onto a seat bought with `approval`. The bought seat
/// is unformed until a guardian arrives.
pub(super) async fn start_transfer_with(
    client: &TestClient,
    state: &MgState,
    approval: TransferDestinationApproval,
) -> FaResult<GuardianTransfer> {
    state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(DESTINATION, ServiceStatus::New);
    client
        .start_guardian_transfer(
            MOVED as u16,
            approval,
            Some(payment_federation_id()),
            None,
            transfer_options(),
        )
        .await
}

pub(super) async fn start_transfer(client: &TestClient, state: &MgState) -> GuardianTransfer {
    start_transfer_with(client, state, destination_approval(PAYMENT_AMOUNT_MSATS))
        .await
        .unwrap()
}

fn destination_create_records(state: &MgState) -> Vec<CreateRecord> {
    state
        .create_records
        .lock()
        .expect("test lock")
        .iter()
        .filter(|record| {
            record
                .signed_quote
                .verify(&manager_key(DESTINATION).x_only_public_key().0)
                .is_ok()
        })
        .cloned()
        .collect()
}

/// The open transfer's purchase, while it still buys its seat.
async fn open_purchase(client: &TestClient) -> crate::seat_purchase::StoredSeatPurchase {
    let transfer = client
        .inner
        .store
        .load_guardian_transfer()
        .await
        .unwrap()
        .expect("an open transfer");
    let TransferStage::Buying(purchase) = transfer.stage else {
        panic!("the transfer still buys its seat");
    };
    purchase
}

#[tokio::test]
async fn a_transfer_buys_its_seat_for_this_federation() {
    let (_database, _payments, payment_state, mg_state, client) = formed_paid_client().await;
    let source_seat_id = formed(&client.status()).seats[MOVED].seat_id.clone();
    let funded_before = payment_state.create_calls.load(Ordering::SeqCst);

    let transfer = start_transfer(&client, &mg_state).await;

    let records = destination_create_records(&mg_state);
    assert_eq!(records.len(), 1);
    assert_eq!(transfer.phase, GuardianTransferPhase::AwaitingSource);
    assert_eq!(transfer.destination_mg_id, test_mg_id(DESTINATION));
    assert_eq!(
        transfer.destination_seat_id,
        Some(records[0].seat_id.clone())
    );
    // The seat is quoted for this federation, not for a seat of size one.
    let quote = records[0]
        .signed_quote
        .verify(&manager_key(DESTINATION).x_only_public_key().0)
        .unwrap();
    assert_eq!(
        quote.terms.request.federation_size,
        FederationSize(MIN_FEDERATION_SIZE)
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        funded_before + 1
    );

    // The backup names no open move: its seats change only at the rebind.
    let backup = client.inner.store.backup_payload().await.unwrap().payload;
    assert_eq!(backup.seats.len(), usize::from(MIN_FEDERATION_SIZE));
    assert_eq!(backup.seats[MOVED].seat_id, source_seat_id);
}

#[tokio::test]
async fn a_lost_acceptance_after_payment_replays_the_exact_quote() {
    let (_database, _payments, payment_state, mg_state, client) = formed_paid_client().await;
    mg_state
        .failed_create_indices
        .lock()
        .expect("test lock")
        .insert(DESTINATION);

    start_transfer_with(
        &client,
        &mg_state,
        destination_approval(PAYMENT_AMOUNT_MSATS),
    )
    .await
    .unwrap_err();
    let pending = client.guardian_transfer().await.unwrap().unwrap();
    assert_eq!(pending.phase, GuardianTransferPhase::BuyingSeat);
    assert_eq!(pending.destination_mg_id, test_mg_id(DESTINATION));
    assert_eq!(pending.destination_seat_id, None);
    assert!(open_purchase(&client).await.outputs_started);
    let quotes_after_failure = mg_state.quote_calls.load(Ordering::SeqCst);
    let funded_after_failure = payment_state.create_calls.load(Ordering::SeqCst);

    mg_state
        .failed_create_indices
        .lock()
        .expect("test lock")
        .clear();
    let resumed = client
        .resume_guardian_transfer(short_options())
        .await
        .unwrap()
        .unwrap();

    // No fresh quote and no second funding: the paid quote is replayed.
    assert_eq!(
        mg_state.quote_calls.load(Ordering::SeqCst),
        quotes_after_failure
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        funded_after_failure
    );
    let records = destination_create_records(&mg_state);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].quote_id, records[1].quote_id);
    assert_eq!(resumed.phase, GuardianTransferPhase::AwaitingSource);
    assert_eq!(
        resumed.destination_seat_id,
        Some(records[1].seat_id.clone())
    );
    assert_eq!(mg_state.pull_requests.lock().expect("test lock").len(), 1);
}

#[tokio::test]
async fn an_unarmed_reservation_is_released_and_the_quote_refreshed() {
    let (_database, _payments, payment_state, mg_state, client) = formed_paid_client().await;
    // Formation made one reservation; lose the result of the purchase's.
    let next_reservation = payment_state.readiness_calls.load(Ordering::SeqCst) + 1;
    payment_state
        .lose_reservation_result_on_call
        .store(next_reservation, Ordering::SeqCst);

    start_transfer_with(
        &client,
        &mg_state,
        destination_approval(PAYMENT_AMOUNT_MSATS),
    )
    .await
    .unwrap_err();
    let purchase = open_purchase(&client).await;
    assert!(!purchase.outputs_started);
    let stale_quote = purchase.signed_quote.unwrap();
    let quotes_before = mg_state.quote_calls.load(Ordering::SeqCst);
    let releases_before = payment_state.whole_release_calls.load(Ordering::SeqCst);

    client
        .resume_guardian_transfer(short_options())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        payment_state.whole_release_calls.load(Ordering::SeqCst),
        releases_before + 1
    );
    assert_eq!(
        mg_state.quote_calls.load(Ordering::SeqCst),
        quotes_before + 1
    );
    let records = destination_create_records(&mg_state);
    assert_eq!(records.len(), 1);
    assert_ne!(records[0].signed_quote, stale_quote);
}

#[tokio::test]
async fn a_refused_purchase_settles_its_refund_and_closes_the_transfer() {
    let (_database, _payments, payment_state, mg_state, client) = formed_paid_client().await;
    mg_state
        .refused_create_indices
        .lock()
        .expect("test lock")
        .insert(DESTINATION);
    let refunds_before = payment_state.refund_calls.load(Ordering::SeqCst);

    let error = start_transfer_with(
        &client,
        &mg_state,
        destination_approval(PAYMENT_AMOUNT_MSATS),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SelectedMgUnavailable,
            failure: Some(MgFailure {
                cause: MgCause::OfferChanged,
                ..
            }),
        }
    ));
    assert_eq!(
        payment_state.refund_calls.load(Ordering::SeqCst),
        refunds_before + 1
    );
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    assert!(mg_state.pull_requests.lock().expect("test lock").is_empty());
}

#[tokio::test]
async fn a_quote_above_the_approved_cap_moves_no_value() {
    let (_database, _payments, payment_state, mg_state, client) = formed_paid_client().await;
    let reservations_before = payment_state.readiness_calls.load(Ordering::SeqCst);

    let error = start_transfer_with(
        &client,
        &mg_state,
        destination_approval(PAYMENT_AMOUNT_MSATS - 1),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::QuoteTotalExceedsLimit,
            ..
        }
    ));
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        reservations_before
    );
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
}

#[tokio::test]
async fn a_payment_that_never_started_rechecks_its_payer() {
    // Also crash right after the purchase is marked unstarted again, before
    // the reservation is released.
    for stop in [false, true] {
        payment_that_never_started_rechecks_its_payer(stop).await;
    }
}

async fn payment_that_never_started_rechecks_its_payer(stop_before_release: bool) {
    let (_database, _payments, payment_state, mg_state, client) = formed_paid_client().await;
    // Reserve the purchase's payment but lose the result, then record the
    // payment as started: a crash just before the wallet funded it.
    let next_reservation = payment_state.readiness_calls.load(Ordering::SeqCst) + 1;
    payment_state
        .lose_reservation_result_on_call
        .store(next_reservation, Ordering::SeqCst);
    start_transfer_with(
        &client,
        &mg_state,
        destination_approval(PAYMENT_AMOUNT_MSATS),
    )
    .await
    .unwrap_err();
    let mut transfer = client
        .inner
        .store
        .load_guardian_transfer()
        .await
        .unwrap()
        .unwrap();
    let TransferStage::Buying(purchase) = &mut transfer.stage else {
        panic!("the transfer still buys its seat");
    };
    purchase.outputs_started = true;
    let lease = client
        .inner
        .store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(5))
        .await
        .unwrap();
    lease
        .write_guardian_transfer(
            TestIdentity::fa_id(),
            crate::db::GuardianTransferWrite::Update(transfer),
        )
        .await
        .unwrap();
    client
        .inner
        .store
        .release_driver_lease(lease)
        .await
        .unwrap();
    // The policy no longer allows the payer.
    payment_state.pay_none.store(true, Ordering::SeqCst);
    let funded_before = payment_state.create_calls.load(Ordering::SeqCst);
    let releases_before = payment_state.whole_release_calls.load(Ordering::SeqCst);

    if stop_before_release {
        client.inner.store.stop_after_transfer_writes(1);
        client
            .resume_guardian_transfer(short_options())
            .await
            .unwrap_err();
        assert!(!open_purchase(&client).await.outputs_started);
        assert_eq!(
            payment_state.whole_release_calls.load(Ordering::SeqCst),
            releases_before
        );
    }

    let error = client
        .resume_guardian_transfer(short_options())
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SelectedPayerUnavailable,
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        funded_before
    );
    assert_eq!(
        payment_state.whole_release_calls.load(Ordering::SeqCst),
        releases_before + 1
    );
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    assert!(destination_create_records(&mg_state).is_empty());
}

#[tokio::test]
async fn a_priced_destination_needs_a_payer_before_the_transfer_opens() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    let connects_before = mg_state.connect_calls.load(Ordering::SeqCst);

    let error = client
        .start_guardian_transfer(
            MOVED as u16,
            destination_approval(PAYMENT_AMOUNT_MSATS),
            None,
            None,
            transfer_options(),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::PaymentFederationRequired,
                ..
            }
        ),
        "{error:?}"
    );
    // Refused before the transfer opened, so no MG was asked.
    assert_eq!(
        mg_state.connect_calls.load(Ordering::SeqCst),
        connects_before
    );
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    assert!(destination_create_records(&mg_state).is_empty());
}

#[tokio::test]
async fn a_free_destination_that_starts_to_charge_closes_the_transfer() {
    let (_database, _payments, payment_state, mg_state, client) = formed_paid_client().await;
    let reservations_before = payment_state.readiness_calls.load(Ordering::SeqCst);
    mg_state
        .seat_statuses
        .lock()
        .expect("test lock")
        .insert(DESTINATION, ServiceStatus::New);

    // The approval saw a free seat, so the FA names no payer. The MG now
    // charges for it.
    let error = client
        .start_guardian_transfer(
            MOVED as u16,
            destination_approval(0),
            None,
            None,
            transfer_options(),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::PaymentFederationRequired,
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        reservations_before
    );
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    assert!(destination_create_records(&mg_state).is_empty());
}

#[tokio::test]
async fn a_destination_that_stops_selling_closes_the_transfer() {
    let (_database, _payments, payment_state, mg_state, client) = formed_paid_client().await;
    let reservations_before = payment_state.readiness_calls.load(Ordering::SeqCst);
    // After the preview, the destination moves to a Fedimint line this
    // federation does not run.
    set_mg_version(&mg_state, DESTINATION, "0.13.0+fedi");

    let error = start_transfer_with(
        &client,
        &mg_state,
        destination_approval(PAYMENT_AMOUNT_MSATS),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(
            error,
            FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SelectedMgUnavailable,
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        reservations_before
    );
    assert!(destination_create_records(&mg_state).is_empty());
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    // The slot is free: a new selection starts a new transfer.
    set_mg_version(&mg_state, DESTINATION, &fedimintd_version().to_string());
    let transfer = start_transfer(&client, &mg_state).await;
    assert_eq!(transfer.phase, GuardianTransferPhase::AwaitingSource);
}

#[tokio::test]
async fn only_one_transfer_is_open_at_a_time() {
    let (_database, _payments, _payment_state, mg_state, client) = formed_paid_client().await;
    mg_state
        .failed_create_indices
        .lock()
        .expect("test lock")
        .insert(DESTINATION);
    start_transfer_with(
        &client,
        &mg_state,
        destination_approval(PAYMENT_AMOUNT_MSATS),
    )
    .await
    .unwrap_err();

    let error = start_transfer_with(
        &client,
        &mg_state,
        destination_approval(PAYMENT_AMOUNT_MSATS),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, FaError::InvalidIntent(_)), "{error:?}");
}

#[test]
fn a_backup_without_past_hosts_keeps_its_earlier_encoding() {
    let payload = crate::backup::FaBackupPayload {
        schema_version: crate::backup::BACKUP_SCHEMA_VERSION,
        snapshot_generation: 1,
        federation_invite: test_invite(0),
        seats: Vec::new(),
        liquidity: None,
        past_hosts: Vec::new(),
    };
    let json = serde_json::to_value(&payload).unwrap();
    assert!(json.get("past_hosts").is_none());
    let mut with_past_host = payload.clone();
    with_past_host.past_hosts = vec![crate::backup::FaBackupPastHost {
        mg_identity: test_mg_id(MOVED),
        holder: None,
    }];
    let mut encoded = Vec::new();
    ciborium::into_writer(&with_past_host, &mut encoded).unwrap();
    let decoded: crate::backup::FaBackupPayload = ciborium::from_reader(&encoded[..]).unwrap();
    assert_eq!(decoded, with_past_host);
}

pub(super) fn mg_ad(index: usize, price_msats: u64, version: &str) -> nostr_sdk::Event {
    selection::issuer_ad_for_version_and_service_key_at(
        &mg_keys(index),
        &discovery::issuer_keys(0),
        price_msats,
        version,
        manager_key(index).x_only_public_key().0,
        test_now_secs(),
    )
}

/// A fresh FA restored from the backup of a formed paid federation, with
/// the given MG advertisements in its registry.
pub(super) async fn restored_client(
    advertisements: Vec<nostr_sdk::Event>,
) -> (
    fedimint_core::db::Database,
    Arc<PaymentState>,
    Arc<MgState>,
    TestClient,
) {
    let (_database, _payments, _payment_state, mg_state, source) = formed_paid_client().await;
    let backup = source.inner.store.backup_payload().await.unwrap().payload;
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    *registry.advertisements.lock().expect("test lock") = advertisements;
    let (payments, payment_state) = TestPayments::new();
    let database = MemDatabase::new().into_database();
    let restored = open_client_with_registry(
        database.clone(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
        registry,
    )
    .await;
    let status = restored
        .inner
        .store
        .restore_backup_payload_with_completion(TestIdentity::fa_id(), backup, None)
        .await
        .unwrap();
    restored.inner.progress.send_replace(status);
    assert!(matches!(restored.status(), FaStatus::Formed(_)));
    (database, payment_state, mg_state, restored)
}

async fn eligible_destination_mgs(client: &TestClient) -> usize {
    match client
        .preview_transfer_destination(crate::MgDiscoveryOptions::default())
        .await
    {
        Err(FaError::InsufficientMgSeats { eligible, .. }) => eligible,
        other => panic!("the test registry cannot verify badges: {other:?}"),
    }
}

#[tokio::test]
async fn a_restored_fa_buys_a_destination_on_the_federation_line() {
    // Neither a guardian of the federation nor an MG on another Fedimint
    // line may receive the guardian.
    let (_, payment_state, mg_state, restored) = restored_client(vec![
        mg_ad(0, PAYMENT_AMOUNT_MSATS, FEDIMINTD_VERSION_0_1),
        mg_ad(20, PAYMENT_AMOUNT_MSATS, "0.13.0+fedi"),
    ])
    .await;
    assert_eq!(eligible_destination_mgs(&restored).await, 0);
    restored.resume().await.unwrap();

    let transfer = start_transfer(&restored, &mg_state).await;

    assert_eq!(transfer.destination_mg_id, test_mg_id(DESTINATION));
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 1);
    let records = destination_create_records(&mg_state);
    assert_eq!(records.len(), 1);
    let quote = records[0]
        .signed_quote
        .verify(&manager_key(DESTINATION).x_only_public_key().0)
        .unwrap();
    assert_eq!(
        quote.terms.request.federation_size,
        FederationSize(MIN_FEDERATION_SIZE)
    );
    assert_eq!(
        quote.terms.request.fedimintd_version.dkg_version(),
        fedimintd_version().dkg_version()
    );
}

#[tokio::test]
async fn the_destination_line_is_the_one_most_guardians_report() {
    let older = FEDIMINTD_VERSION_0_1.to_string();
    let newer = "0.13.0+fedi".to_owned();
    // No MG of the federation advertises: they are full or offline. Only
    // the guardians' own answers name the line.
    let eligible = async |guardians: Vec<String>| {
        let (_, _, mg_state, restored) =
            restored_client(vec![mg_ad(20, PAYMENT_AMOUNT_MSATS, &newer)]).await;
        *mg_state.guardian_versions.lock().expect("test lock") = Some(guardians);
        eligible_destination_mgs(&restored).await
    };

    // A tie between lines picks the newer one. An answer that does not
    // parse counts for no line.
    assert_eq!(
        eligible(vec![older.clone(), newer.clone(), "garbage".to_owned()]).await,
        1
    );
    // A majority outweighs the newer line.
    assert_eq!(
        eligible(vec![older.clone(), newer.clone(), older.clone()]).await,
        0
    );

    // The saved invite names an offline guardian: the MGs' own invites
    // reach the others.
    let (_, _, mg_state, restored) =
        restored_client(vec![mg_ad(20, PAYMENT_AMOUNT_MSATS, &newer)]).await;
    *mg_state.guardian_versions.lock().expect("test lock") = Some(vec![newer.clone()]);
    let FaStatus::Formed(snapshot) = restored.status() else {
        panic!("restored status");
    };
    mg_state
        .unreachable_version_invites
        .lock()
        .expect("test lock")
        .insert(snapshot.federation_invite.0.clone());
    assert_eq!(eligible_destination_mgs(&restored).await, 1);
    // No invite of an MG reaches a guardian either: no line.
    mg_state
        .unreachable_version_invites
        .lock()
        .expect("test lock")
        .extend((0..usize::from(MIN_FEDERATION_SIZE)).map(|index| test_invite(index).0));
    assert!(matches!(
        restored
            .preview_transfer_destination(crate::MgDiscoveryOptions::default())
            .await,
        Err(FaError::Selection(_))
    ));

    // With no answer, the preview names no line rather than guess one.
    let (_, _, mg_state, restored) =
        restored_client(vec![mg_ad(20, PAYMENT_AMOUNT_MSATS, &newer)]).await;
    *mg_state.guardian_versions.lock().expect("test lock") = Some(vec!["garbage".to_owned()]);
    assert!(matches!(
        restored
            .preview_transfer_destination(crate::MgDiscoveryOptions::default())
            .await,
        Err(FaError::Selection(_))
    ));
}

/// Badge verifier that reports a chosen holder for some MGs, and the MG
/// itself for every other one.
struct HolderVerifier(BTreeMap<PublicKey, PublicKey>);

impl crate::selection::SelectionBadgeVerifier for HolderVerifier {
    async fn verify_badge(
        &self,
        envelope: &fedi_decentralized_domain::HolderAuthorizationEnvelope,
    ) -> Result<
        crate::VerifiedBadgeFacts,
        fedi_decentralized_peer_badge_verifier::PeerBadgeVerificationError,
    > {
        let subject = envelope.holder_authorization.authorization.subject_pubkey.0;
        Ok(crate::VerifiedBadgeFacts {
            issuer: envelope.signed_credential.credential.issuer_id_pubkey.0,
            holder: self.0.get(&subject).copied().unwrap_or(subject),
            subject,
            badge: fedi_decentralized_domain::parse_trust_score_badge_v1(
                &envelope.signed_credential.credential,
            )
            .expect("test badge parses"),
        })
    }
}

#[tokio::test]
async fn a_guardian_holder_cannot_receive_a_guardian_through_another_mg() {
    let other = 20;
    let (_database, _payments, _payment_state, _mg_state, client) = formed_paid_client().await;
    *client
        .inner
        .ports
        .registry
        .advertisements
        .lock()
        .expect("test lock") = vec![
        mg_ad(0, PAYMENT_AMOUNT_MSATS, FEDIMINTD_VERSION_0_1),
        mg_ad(other, PAYMENT_AMOUNT_MSATS, FEDIMINTD_VERSION_0_1),
    ];

    // The holder of a guardian's MG also holds the other MG.
    let error = client
        .preview_transfer_destination_with_verifier(
            crate::MgDiscoveryOptions::default(),
            &HolderVerifier(BTreeMap::from([(test_mg_id(other), test_mg_id(0))])),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, FaError::InsufficientMgSeats { selected: 0, .. }),
        "{error:?}"
    );

    // With its own holder, the other MG can receive the guardian.
    let preview = client
        .preview_transfer_destination_with_verifier(
            crate::MgDiscoveryOptions::default(),
            &HolderVerifier(BTreeMap::new()),
        )
        .await
        .unwrap();
    assert_eq!(preview.seat().candidate().mg_id(), test_mg_id(other));
}

#[tokio::test]
async fn a_rejected_payment_closes_the_transfer_for_another_payer() {
    let (_database, _payments, payment_state, mg_state, client) = formed_paid_client().await;
    // The run stops after it reserves the payment. Mark the payment started:
    // the wallet then reports it rejected and refunded.
    let next_reservation = payment_state.readiness_calls.load(Ordering::SeqCst) + 1;
    payment_state
        .lose_reservation_result_on_call
        .store(next_reservation, Ordering::SeqCst);
    start_transfer_with(
        &client,
        &mg_state,
        destination_approval(PAYMENT_AMOUNT_MSATS),
    )
    .await
    .unwrap_err();
    let mut transfer = client
        .inner
        .store
        .load_guardian_transfer()
        .await
        .unwrap()
        .unwrap();
    let TransferStage::Buying(purchase) = &mut transfer.stage else {
        panic!("the transfer still buys its seat");
    };
    purchase.outputs_started = true;
    let lease = client
        .inner
        .store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(5))
        .await
        .unwrap();
    lease
        .write_guardian_transfer(
            TestIdentity::fa_id(),
            crate::db::GuardianTransferWrite::Update(transfer),
        )
        .await
        .unwrap();
    client
        .inner
        .store
        .release_driver_lease(lease)
        .await
        .unwrap();
    payment_state.reject_recovery.store(true, Ordering::SeqCst);

    let error = client
        .resume_guardian_transfer(short_options())
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SelectedPayerUnavailable,
                failure: None,
            }
        ),
        "{error:?}"
    );
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    assert!(mg_state.pull_requests.lock().expect("test lock").is_empty());
}

/// A preview is a public value that a caller may hand to another client.
/// The approval names the federation it was previewed for, and another
/// federation's transfer refuses it before it asks any MG or opens a
/// transfer.
#[tokio::test]
async fn an_approval_previewed_for_another_federation_is_refused() {
    let other = 20;
    let (_database, _payments, _payment_state, _mg_state, previewing) = formed_paid_client().await;
    *previewing
        .inner
        .ports
        .registry
        .advertisements
        .lock()
        .expect("test lock") = vec![mg_ad(other, PAYMENT_AMOUNT_MSATS, FEDIMINTD_VERSION_0_1)];
    let approval = previewing
        .preview_transfer_destination_with_verifier(
            crate::MgDiscoveryOptions::default(),
            &HolderVerifier(BTreeMap::new()),
        )
        .await
        .unwrap()
        .approve(PAYMENT_AMOUNT_MSATS)
        .unwrap();
    assert_eq!(approval.federation_id, formed_federation_id());
    // The previewing client itself accepts it.
    let own = previewing
        .transfer_federation(TestIdentity::fa_id())
        .await
        .unwrap();
    previewing
        .seat_purchase(approval.clone(), &own, Some(payment_federation_id()))
        .unwrap();

    // Another FA restored a federation with other guardians.
    let (_database, _payments, _payment_state, mg_state, source) = formed_paid_client().await;
    let mut backup = source.inner.store.backup_payload().await.unwrap().payload;
    backup.federation_invite = test_invite_for_federation(0, 1);
    let (payments, payment_state) = TestPayments::new();
    let other_client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    let status = other_client
        .inner
        .store
        .restore_backup_payload_with_completion(TestIdentity::fa_id(), backup, None)
        .await
        .unwrap();
    other_client.inner.progress.send_replace(status);
    let connects_before = mg_state.connect_calls.load(Ordering::SeqCst);
    let readiness_before = payment_state.readiness_calls.load(Ordering::SeqCst);

    let error = other_client
        .start_guardian_transfer(
            MOVED as u16,
            approval,
            Some(payment_federation_id()),
            None,
            transfer_options(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, FaError::InvalidIntent(reason) if reason.contains("another federation")),
        "{error:?}"
    );
    assert_eq!(
        mg_state.connect_calls.load(Ordering::SeqCst),
        connects_before
    );
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        readiness_before
    );
    assert_eq!(other_client.guardian_transfer().await.unwrap(), None);
}

/// The preview kept the destination off the guardians' service keys. The
/// start checks again against the guardians it has now.
#[tokio::test]
async fn a_destination_with_a_guardian_service_key_is_refused_at_start() {
    let (_database, _payments, payment_state, mg_state, client) = formed_paid_client().await;
    let mut approval = destination_approval(PAYMENT_AMOUNT_MSATS);
    approval.seat.locator = Locator::new(
        locator(DESTINATION).endpoint_addr.clone(),
        manager_key(2).x_only_public_key().0,
    );
    let connects_before = mg_state.connect_calls.load(Ordering::SeqCst);
    let readiness_before = payment_state.readiness_calls.load(Ordering::SeqCst);

    let error = client
        .start_guardian_transfer(
            MOVED as u16,
            approval,
            Some(payment_federation_id()),
            None,
            transfer_options(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, FaError::InvalidIntent(reason) if reason.contains("service key")),
        "{error:?}"
    );
    assert_eq!(
        mg_state.connect_calls.load(Ordering::SeqCst),
        connects_before
    );
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        readiness_before
    );
    assert_eq!(client.guardian_transfer().await.unwrap(), None);
    assert!(destination_create_records(&mg_state).is_empty());
}
