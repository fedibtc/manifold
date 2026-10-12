//! Spare-seat purchase into the FI seat pool.

use super::*;
use crate::selection::{ApprovedFmanSeat, SpareSeatApproval};

/// The first FMan outside the formation's pinned seats.
pub(super) const SPARE_INDEX: usize = MIN_FEDERATION_SIZE as usize;

pub(super) async fn formed_paid_client() -> (
    fedimint_core::db::Database,
    TestPayments,
    Arc<PaymentState>,
    Arc<FmanState>,
    TestClient,
) {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    let client = open_client_with_registry(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
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
    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
    (database, payments, payment_state, fman_state, client)
}

pub(super) fn spare_approval(max_msats: u64) -> SpareSeatApproval {
    SpareSeatApproval {
        verifier_provenance: test_peer_badge_verifier().provenance(),
        fedimintd_dkg_version: fedimintd_version().dkg_version(),
        seat: ApprovedFmanSeat {
            fman_id: test_fman_id(SPARE_INDEX),
            holder: test_fman_id(SPARE_INDEX),
            locator: locator(SPARE_INDEX),
        },
        max_msats,
        valid_until: Timestamp(test_now_secs() + 120),
    }
}

fn spare_create_records(state: &FmanState) -> Vec<CreateRecord> {
    state
        .create_records
        .lock()
        .expect("test lock")
        .iter()
        .filter(|record| {
            record
                .signed_quote
                .verify(&manager_key(SPARE_INDEX).x_only_public_key().0)
                .is_ok()
        })
        .cloned()
        .collect()
}

#[tokio::test]
async fn a_paid_spare_seat_lands_in_the_pool_and_the_backup() {
    let (_database, _payments, payment_state, fman_state, client) = formed_paid_client().await;
    let funded_before = payment_state.create_calls.load(Ordering::SeqCst);

    let spare = client
        .buy_spare_seat(spare_approval(PAYMENT_AMOUNT_MSATS), long_request_options())
        .await
        .unwrap();

    let records = spare_create_records(&fman_state);
    assert_eq!(records.len(), 1);
    assert_eq!(spare.seat_id, records[0].seat_id);
    assert_eq!(spare.locator, locator(SPARE_INDEX));
    assert_eq!(spare.fman_id, test_fman_id(SPARE_INDEX));
    // The spare is quoted for this federation, not for a seat of size one.
    let quote = records[0]
        .signed_quote
        .verify(&manager_key(SPARE_INDEX).x_only_public_key().0)
        .unwrap();
    assert_eq!(
        quote.terms.request.federation_size,
        FederationSize(MIN_FEDERATION_SIZE)
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        funded_before + 1
    );
    assert_eq!(client.spare_seats().await.unwrap(), vec![spare.clone()]);
    assert!(
        client
            .inner
            .store
            .load_seat_purchase()
            .await
            .unwrap()
            .is_none()
    );

    let backup = client.inner.store.backup_payload().await.unwrap().payload;
    assert_eq!(backup.seats.len(), usize::from(MIN_FEDERATION_SIZE));
    assert_eq!(
        backup.spare_seats,
        vec![crate::backup::FiBackupSeat {
            fman_identity: spare.fman_id,
            seat_id: spare.seat_id.clone(),
            locator: spare.locator.clone(),
        }]
    );

    let restored = db::FiStore::new(MemDatabase::new().into_database());
    restored
        .restore_backup_payload_with_completion(TestIdentity::fi_id(), backup, None)
        .await
        .unwrap();
    assert_eq!(
        restored
            .list_spare_seats()
            .await
            .unwrap()
            .into_iter()
            .map(|seat| (seat.fman_id, seat.locator, seat.seat_id))
            .collect::<Vec<_>>(),
        vec![(spare.fman_id, spare.locator, spare.seat_id)]
    );
}

#[tokio::test]
async fn a_lost_acceptance_after_payment_replays_the_exact_quote() {
    let (_database, _payments, payment_state, fman_state, client) = formed_paid_client().await;
    fman_state
        .failed_create_indices
        .lock()
        .expect("test lock")
        .insert(SPARE_INDEX);

    client
        .buy_spare_seat(spare_approval(PAYMENT_AMOUNT_MSATS), long_request_options())
        .await
        .unwrap_err();
    let pending = client.spare_seat_purchase().await.unwrap().unwrap();
    assert_eq!(pending.fman_id, test_fman_id(SPARE_INDEX));
    assert_eq!(pending.max_msats, PAYMENT_AMOUNT_MSATS);
    assert!(pending.outputs_started);
    let quotes_after_failure = fman_state.quote_calls.load(Ordering::SeqCst);
    let funded_after_failure = payment_state.create_calls.load(Ordering::SeqCst);
    assert!(
        client
            .inner
            .store
            .load_seat_purchase()
            .await
            .unwrap()
            .unwrap()
            .outputs_started
    );

    fman_state
        .failed_create_indices
        .lock()
        .expect("test lock")
        .clear();
    let spare = client
        .resume_spare_seat_purchase(long_request_options())
        .await
        .unwrap()
        .unwrap();

    // No fresh quote and no second funding: the paid quote is replayed.
    assert_eq!(
        fman_state.quote_calls.load(Ordering::SeqCst),
        quotes_after_failure
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        funded_after_failure
    );
    let records = spare_create_records(&fman_state);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].quote_id, records[1].quote_id);
    assert_eq!(spare.seat_id, records[1].seat_id);
    assert_eq!(client.spare_seat_purchase().await.unwrap(), None);
}

#[tokio::test]
async fn an_unarmed_reservation_is_released_and_the_quote_refreshed() {
    let (_database, _payments, payment_state, fman_state, client) = formed_paid_client().await;
    // Formation made one reservation; lose the result of the purchase's.
    let next_reservation = payment_state.readiness_calls.load(Ordering::SeqCst) + 1;
    payment_state
        .lose_reservation_result_on_call
        .store(next_reservation, Ordering::SeqCst);

    client
        .buy_spare_seat(spare_approval(PAYMENT_AMOUNT_MSATS), long_request_options())
        .await
        .unwrap_err();
    let purchase = client
        .inner
        .store
        .load_seat_purchase()
        .await
        .unwrap()
        .unwrap();
    assert!(!purchase.outputs_started);
    let stale_quote = purchase.signed_quote.unwrap();
    let quotes_before = fman_state.quote_calls.load(Ordering::SeqCst);
    let releases_before = payment_state.whole_release_calls.load(Ordering::SeqCst);

    client
        .resume_spare_seat_purchase(long_request_options())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        payment_state.whole_release_calls.load(Ordering::SeqCst),
        releases_before + 1
    );
    assert_eq!(
        fman_state.quote_calls.load(Ordering::SeqCst),
        quotes_before + 1
    );
    let records = spare_create_records(&fman_state);
    assert_eq!(records.len(), 1);
    assert_ne!(records[0].signed_quote, stale_quote);
}

#[tokio::test]
async fn a_refused_purchase_settles_its_refund_and_adds_no_seat() {
    let (_database, _payments, payment_state, fman_state, client) = formed_paid_client().await;
    fman_state
        .refused_create_indices
        .lock()
        .expect("test lock")
        .insert(SPARE_INDEX);
    let refunds_before = payment_state.refund_calls.load(Ordering::SeqCst);

    let error = client
        .buy_spare_seat(spare_approval(PAYMENT_AMOUNT_MSATS), long_request_options())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        FiError::Fman(FmanFailure {
            cause: FmanCause::OfferChanged,
            ..
        })
    ));
    assert_eq!(
        payment_state.refund_calls.load(Ordering::SeqCst),
        refunds_before + 1
    );
    assert!(client.spare_seats().await.unwrap().is_empty());
    assert!(
        client
            .inner
            .store
            .load_seat_purchase()
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_quote_above_the_approved_cap_moves_no_value() {
    let (_database, _payments, payment_state, _fman_state, client) = formed_paid_client().await;
    let reservations_before = payment_state.readiness_calls.load(Ordering::SeqCst);

    let error = client
        .buy_spare_seat(
            spare_approval(PAYMENT_AMOUNT_MSATS - 1),
            long_request_options(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::QuoteTotalExceedsLimit,
            ..
        }
    ));
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        reservations_before
    );
    assert!(
        client
            .inner
            .store
            .load_seat_purchase()
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn only_one_purchase_is_open_at_a_time() {
    let (_database, _payments, _payment_state, fman_state, client) = formed_paid_client().await;
    fman_state
        .failed_create_indices
        .lock()
        .expect("test lock")
        .insert(SPARE_INDEX);
    client
        .buy_spare_seat(spare_approval(PAYMENT_AMOUNT_MSATS), long_request_options())
        .await
        .unwrap_err();

    let error = client
        .buy_spare_seat(spare_approval(PAYMENT_AMOUNT_MSATS), long_request_options())
        .await
        .unwrap_err();

    assert!(matches!(error, FiError::InvalidIntent(_)));
}

#[test]
fn a_backup_without_spares_keeps_its_earlier_encoding() {
    let payload = crate::backup::FiBackupPayload {
        schema_version: crate::backup::BACKUP_SCHEMA_VERSION,
        snapshot_generation: 1,
        federation_invite: test_invite(0),
        seats: Vec::new(),
        liquidity: None,
        spare_seats: Vec::new(),
        past_hosts: Vec::new(),
    };
    let json = serde_json::to_value(&payload).unwrap();
    assert!(json.get("spare_seats").is_none());
    let mut with_spare = payload.clone();
    with_spare.spare_seats.push(crate::backup::FiBackupSeat {
        fman_identity: test_fman_id(SPARE_INDEX),
        seat_id: SeatId::from(QuoteId([7; 32])),
        locator: locator(SPARE_INDEX),
    });
    let mut encoded = Vec::new();
    ciborium::into_writer(&with_spare, &mut encoded).unwrap();
    let decoded: crate::backup::FiBackupPayload = ciborium::from_reader(&encoded[..]).unwrap();
    assert_eq!(decoded, with_spare);
}

pub(super) fn fman_ad(index: usize, price_msats: u64, version: &str) -> nostr_sdk::Event {
    selection::issuer_ad_for_version_and_service_key_at(
        &fman_keys(index),
        &discovery::issuer_keys(0),
        price_msats,
        version,
        manager_key(index).x_only_public_key().0,
        test_now_secs(),
    )
}

/// A fresh FI restored from the backup of a formed paid federation, with
/// the given FMan advertisements in its registry.
pub(super) async fn restored_client(
    advertisements: Vec<nostr_sdk::Event>,
) -> (
    fedimint_core::db::Database,
    Arc<PaymentState>,
    Arc<FmanState>,
    TestClient,
) {
    let (_database, _payments, _payment_state, fman_state, source) = formed_paid_client().await;
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
        fman_state.clone(),
        FmanConfig::paid(),
        registry,
    )
    .await;
    let status = restored
        .inner
        .store
        .restore_backup_payload_with_completion(TestIdentity::fi_id(), backup, None)
        .await
        .unwrap();
    restored.inner.progress.send_replace(status);
    assert!(matches!(restored.status(), FiStatus::Restored(_)));
    (database, payment_state, fman_state, restored)
}

async fn eligible_spare_fmans(client: &TestClient) -> usize {
    match client
        .preview_spare_seat(crate::FmanDiscoveryOptions::default())
        .await
    {
        Err(FiError::InsufficientFmanSeats { eligible, .. }) => eligible,
        other => panic!("the test registry cannot verify badges: {other:?}"),
    }
}

#[tokio::test]
async fn a_restored_fi_buys_a_spare_on_the_federation_line() {
    // Neither a guardian of the federation nor an FMan on another Fedimint
    // line may hold a spare.
    let (_, payment_state, fman_state, restored) = restored_client(vec![
        fman_ad(0, PAYMENT_AMOUNT_MSATS, FEDIMINTD_VERSION_0_1),
        fman_ad(20, PAYMENT_AMOUNT_MSATS, "0.13.0+fedi"),
    ])
    .await;
    assert_eq!(eligible_spare_fmans(&restored).await, 0);

    let spare = restored
        .buy_spare_seat(spare_approval(PAYMENT_AMOUNT_MSATS), long_request_options())
        .await
        .unwrap();

    assert_eq!(spare.fman_id, test_fman_id(SPARE_INDEX));
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 1);
    let records = spare_create_records(&fman_state);
    assert_eq!(records.len(), 1);
    let quote = records[0]
        .signed_quote
        .verify(&manager_key(SPARE_INDEX).x_only_public_key().0)
        .unwrap();
    assert_eq!(
        quote.terms.request.federation_size,
        FederationSize(MIN_FEDERATION_SIZE)
    );
    assert_eq!(
        quote.terms.request.fedimintd_version.dkg_version(),
        fedimintd_version().dkg_version()
    );
    assert_eq!(restored.spare_seats().await.unwrap(), vec![spare]);
}

#[tokio::test]
async fn the_spare_line_is_the_one_most_guardians_advertise() {
    let newer = "0.13.0+fedi";
    // A tie between lines picks the newer one.
    let (_, _, _, restored) = restored_client(vec![
        fman_ad(0, PAYMENT_AMOUNT_MSATS, FEDIMINTD_VERSION_0_1),
        fman_ad(1, PAYMENT_AMOUNT_MSATS, newer),
        fman_ad(20, PAYMENT_AMOUNT_MSATS, newer),
    ])
    .await;
    assert_eq!(eligible_spare_fmans(&restored).await, 1);

    // A majority outweighs the newer line.
    let (_, _, _, restored) = restored_client(vec![
        fman_ad(0, PAYMENT_AMOUNT_MSATS, FEDIMINTD_VERSION_0_1),
        fman_ad(1, PAYMENT_AMOUNT_MSATS, newer),
        fman_ad(2, PAYMENT_AMOUNT_MSATS, FEDIMINTD_VERSION_0_1),
        fman_ad(20, PAYMENT_AMOUNT_MSATS, newer),
    ])
    .await;
    assert_eq!(eligible_spare_fmans(&restored).await, 0);
}

/// Badge verifier that reports a chosen holder for some FMans, and the FMan
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
async fn a_spare_holder_cannot_sell_a_second_spare_through_another_fman() {
    let other = 20;
    let (_, _, _, restored) = restored_client(vec![
        fman_ad(0, PAYMENT_AMOUNT_MSATS, FEDIMINTD_VERSION_0_1),
        fman_ad(other, PAYMENT_AMOUNT_MSATS, FEDIMINTD_VERSION_0_1),
    ])
    .await;
    restored
        .buy_spare_seat(spare_approval(PAYMENT_AMOUNT_MSATS), long_request_options())
        .await
        .unwrap();

    // The holder of the bought spare also holds the other FMan.
    let error = restored
        .preview_spare_seat_with_verifier(
            crate::FmanDiscoveryOptions::default(),
            &HolderVerifier(BTreeMap::from([(
                test_fman_id(other),
                test_fman_id(SPARE_INDEX),
            )])),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, FiError::InsufficientFmanSeats { selected: 0, .. }),
        "{error:?}"
    );

    // With its own holder, the other FMan can sell a spare.
    let preview = restored
        .preview_spare_seat_with_verifier(
            crate::FmanDiscoveryOptions::default(),
            &HolderVerifier(BTreeMap::new()),
        )
        .await
        .unwrap();
    assert_eq!(preview.seat().candidate().fman_id(), test_fman_id(other));
}
