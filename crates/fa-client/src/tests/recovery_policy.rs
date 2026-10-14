//! Funded-seat recovery policy regression tests.

use super::*;

#[tokio::test]
async fn a_rejected_payment_leaves_and_prepared_seats_replay() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;
    let quote_ids = seed_bound_selected_purchase(&client).await;
    let (rejected, funded) = quote_ids.split_last().expect("at least one quote");
    payment_state
        .funded_quotes
        .lock()
        .expect("test lock")
        .extend(funded.iter().copied());
    payment_state
        .rejected_quotes
        .lock()
        .expect("test lock")
        .insert(*rejected);

    assert!(matches!(
        client.drive().await,
        Err(FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SelectedPayerUnavailable,
            failure: None,
        })
    ));

    bought_without(&client.status(), &[MIN_FEDERATION_SIZE - 1]);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        mg_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE) - 1
    );
    assert_eq!(
        client.status().reauthorization,
        Some(SelectionReauthorizationReason::SelectedPayerUnavailable)
    );
}

#[tokio::test]
async fn a_refusal_interrupted_before_its_refund_replays_on_resume() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let config = MgConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..MgConfig::paid()
    };
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;
    let quote_ids = seed_bound_selected_purchase(&client).await;
    payment_state
        .funded_quotes
        .lock()
        .expect("test lock")
        .extend(quote_ids.iter().copied());
    payment_state
        .hang_first_refund
        .store(true, Ordering::SeqCst);

    let refund_started = payment_state.refund_started.notified();
    let running = client.clone();
    let operation =
        tokio::spawn(async move { running.drive_with_options(long_request_options()).await });
    refund_started.await;
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let refused = mg_state
        .refused_quote
        .lock()
        .expect("test lock")
        .expect("one quote refused");
    let refused_index = u16::try_from(
        quote_ids
            .iter()
            .position(|quote_id| *quote_id == refused)
            .expect("the refused quote belongs to the purchase"),
    )
    .unwrap();
    assert!(
        seat_indices(&active_recovery(
            client
                .inner
                .store
                .load_purchase(TestIdentity::fa_id())
                .await
                .unwrap(),
        ))
        .contains(&refused_index),
        "the refused seat stays until its refund settles",
    );
    drop(client);

    let reopened = open_client(database, payments, mg_state, config).await;
    assert!(matches!(
        reopened.drive().await,
        Err(FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SelectedMgUnavailable,
            ..
        })
    ));

    bought_without(&reopened.status(), &[refused_index]);
    assert_eq!(payment_state.refund_calls.load(Ordering::SeqCst), 2);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn funded_recovery_ignores_removed_or_empty_current_policy() {
    let now = test_now_secs();
    for replacement in [
        setup_payment_event(now + 1, &[]),
        setup_payment_event(now + 1, &[&second_payment_invite()]),
    ] {
        let database = MemDatabase::new().into_database();
        let (payments, payment_state) = TestPayments::new();
        let mg_state = Arc::new(MgState::default());
        let registry = TestRegistry::default();
        *registry.candidates.lock().expect("test lock") =
            vec![setup_payment_event(now, &[PAYMENT_INVITE])];
        let mut config = MgConfig::paid();
        config.create_behavior = CreateBehavior::HangFirst;
        let client = open_client_with_registry(
            database.clone(),
            payments.clone(),
            mg_state.clone(),
            config,
            registry.clone(),
        )
        .await;
        let quote_ids = seed_bound_selected_purchase(&client).await;
        payment_state
            .funded_quotes
            .lock()
            .expect("test lock")
            .extend(quote_ids);

        let create_started = mg_state.create_started.notified();
        let running = client.clone();
        let operation =
            tokio::spawn(async move { running.drive_with_options(long_request_options()).await });
        create_started.await;
        operation.abort();
        assert!(operation.await.unwrap_err().is_cancelled());
        assert_eq!(
            payment_state.funded_quotes.lock().expect("test lock").len(),
            usize::from(MIN_FEDERATION_SIZE)
        );
        let payable_calls_before_resume = payment_state.payable_calls.load(Ordering::SeqCst);
        *registry.candidates.lock().expect("test lock") = vec![replacement];

        let reopened =
            open_client_with_registry(database, payments, mg_state, config, registry).await;
        reopened.drive().await.unwrap();
        bought(&reopened.status());
        assert_eq!(
            payment_state.payable_calls.load(Ordering::SeqCst),
            payable_calls_before_resume
        );
    }
}

#[tokio::test]
async fn mixed_funded_recovery_replays_before_empty_policy_stops_unfunded_seats() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let registry = TestRegistry::default();
    let now = test_now_secs();
    *registry.candidates.lock().expect("test lock") =
        vec![setup_payment_event(now, &[PAYMENT_INVITE])];
    let mut config = MgConfig::paid();
    config.create_behavior = CreateBehavior::HangFirst;
    let client = open_client_with_registry(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        config,
        registry.clone(),
    )
    .await;
    let create_started = mg_state.create_started.notified();
    let running = client.clone();
    let operation = tokio::spawn(async move {
        running
            .buy_seats_at(&intent(), locators(), long_request_options())
            .await
    });
    create_started.await;
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    assert_eq!(
        payment_state.funded_quotes.lock().expect("test lock").len(),
        1
    );
    *registry.candidates.lock().expect("test lock") = vec![setup_payment_event(now + 1, &[])];

    let reopened =
        open_client_with_registry(database, payments, mg_state.clone(), config, registry).await;
    assert!(matches!(
        reopened.drive().await,
        Err(FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SelectedPayerUnavailable,
            failure: None,
        })
    ));
    assert_eq!(mg_state.create_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        mg_state.allocated_quotes.lock().expect("test lock").len(),
        1
    );
}

#[tokio::test]
async fn a_refused_free_seat_leaves_and_the_paid_seats_complete() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let free = usize::from(MIN_FEDERATION_SIZE) - 1;
    mg_state
        .price_overrides_msats
        .lock()
        .expect("test lock")
        .insert(free, 0);
    mg_state
        .refused_create_indices
        .lock()
        .expect("test lock")
        .insert(free);
    let config = MgConfig::paid();
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;
    let paid_seats = usize::from(MIN_FEDERATION_SIZE) - 1;

    let error = client
        .buy_seats_at(&intent(), locators(), options())
        .await
        .expect_err("the free member refuses its presentation");
    assert!(
        matches!(
            &error,
            FaError::SelectionReauthorizationRequired {
                failure: Some(MgFailure {
                    cause: MgCause::OfferChanged,
                    ..
                }),
                ..
            }
        ),
        "{error:?}"
    );
    bought_without(&client.status(), &[u16::try_from(free).unwrap()]);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        paid_seats
    );
}
