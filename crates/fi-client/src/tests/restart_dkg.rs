use super::*;

async fn formation_waiting_on_dkg(fman_state: Arc<FmanState>) -> TestClient {
    fman_state.disagreeing_invite.store(true, Ordering::SeqCst);
    let (payments, _) = TestPayments::new();
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state,
        FmanConfig::given_away(),
    )
    .await;
    assert!(
        client
            .create_with_pinned_fmans(intent(), locators(), options())
            .await
            .is_err()
    );
    assert_eq!(
        formation(&client.status()).phase,
        FormationPhase::PreparingDkg
    );
    client
}

#[tokio::test]
async fn restart_sends_the_recorded_codes_to_every_seat_and_reports_each_answer() {
    let fman_state = Arc::new(FmanState::default());
    let client = formation_waiting_on_dkg(fman_state.clone()).await;
    assert_eq!(
        fman_state.restart_calls.load(Ordering::SeqCst),
        0,
        "ordinary formation never selects the destructive restart"
    );
    let recorded_codes = formation(&client.status())
        .seats
        .iter()
        .map(|seat| seat.guardian_code.clone().expect("seat holds a code"))
        .collect::<Vec<_>>();
    fman_state
        .offline_indices
        .lock()
        .expect("test lock")
        .insert(0);
    fman_state
        .restart_finished_indices
        .lock()
        .expect("test lock")
        .insert(1);
    let status_before = client.status();

    let outcome = client.restart_dkg().await.unwrap();

    assert_eq!(
        outcome.restarted,
        (2..MIN_FEDERATION_SIZE).collect::<Vec<_>>()
    );
    assert_eq!(outcome.already_running, vec![1]);
    assert_eq!(
        outcome
            .refused
            .iter()
            .map(|(index, _)| *index)
            .collect::<Vec<_>>(),
        vec![0]
    );
    let sent = fman_state.restart_codes.lock().expect("test lock").clone();
    assert_eq!(sent.len(), usize::from(MIN_FEDERATION_SIZE) - 1);
    assert!(sent.iter().all(|codes| codes == &recorded_codes));
    assert_eq!(client.status(), status_before);
}

#[tokio::test]
async fn restart_refuses_a_formed_federation_without_contacting_any_fleet_manager() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let connect_calls = fman_state.connect_calls.load(Ordering::SeqCst);

    assert!(matches!(
        client.restart_dkg().await,
        Err(FiError::NoActiveFormation)
    ));
    assert_eq!(fman_state.restart_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        fman_state.connect_calls.load(Ordering::SeqCst),
        connect_calls
    );
}
