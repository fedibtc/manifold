//! Funded-seat recovery policy regression tests.

use super::*;

async fn wait_for_count(counter: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while counter.load(Ordering::SeqCst) < expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("test counter reached expected value");
}

#[tokio::test]
async fn durable_reservation_suppresses_quote_refresh_and_pins_exact_set() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;
    let original_quotes = seed_authorized_selected_purchase(&client)
        .await
        .seats
        .iter()
        .map(|seat| seat.quote_id)
        .collect::<HashSet<_>>();
    *mg_state.changed_quote_index.lock().expect("test lock") = Some(0);

    client
        .resume_with_options(long_request_options())
        .await
        .unwrap();

    bought(&client.status());
    assert_eq!(
        mg_state.quote_calls.load(Ordering::SeqCst),
        0,
        "authorization reserves and consumes the authorized quote set without requoting"
    );
    assert_eq!(
        payment_state
            .created_quotes
            .lock()
            .expect("test lock")
            .iter()
            .copied()
            .collect::<HashSet<_>>(),
        original_quotes,
    );
}

#[tokio::test]
async fn terminal_clear_failure_preserves_quote_and_returns_storage_error() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;
    let rejected_quote = seed_armed_selected_purchase(&client, &payment_state)
        .await
        .seats[0]
        .quote_id;
    payment_state
        .rejected_quotes
        .lock()
        .expect("test lock")
        .insert(rejected_quote);
    client.inner.store.fail_quote_clear(0);

    assert!(matches!(
        client.resume_with_options(options()).await,
        Err(FaError::Storage(_))
    ));
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        recovery.seats[0]
            .signed_quote
            .as_ref()
            .expect("failed clear retains the exact quote")
            .verify(&recovery.seats[0].progress.locator.service_pubkey)
            .unwrap()
            .quote_id(),
        rejected_quote
    );
}

#[tokio::test]
async fn multi_terminal_release_failure_preserves_the_complete_aggregate_for_reopen() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let config = MgConfig::paid();
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;
    let requirements = seed_armed_selected_purchase(&client, &payment_state).await;
    let terminal_quotes = requirements
        .seats
        .iter()
        .take(2)
        .map(|seat| seat.quote_id)
        .collect::<Vec<_>>();
    payment_state
        .rejected_quotes
        .lock()
        .expect("test lock")
        .extend(terminal_quotes.iter().copied());
    payment_state
        .failed_terminal_release_quotes
        .lock()
        .expect("test lock")
        .insert(terminal_quotes[1]);

    assert!(matches!(
        client.resume_with_options(options()).await,
        Err(FaError::Payment(_))
    ));
    let interrupted = active_recovery(
        client
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    assert!(
        interrupted.seats[..2]
            .iter()
            .all(|seat| seat.signed_quote.is_some()),
        "the interrupted release wave retains every terminal quote"
    );
    assert!(interrupted.test_payment_authorization().is_some());
    assert!(interrupted.payment_reservation_id.is_some());
    assert!(
        payment_state
            .released_quotes
            .lock()
            .expect("test lock")
            .contains(&terminal_quotes[0]),
        "the first member released before its sibling failed"
    );
    assert!(
        !payment_state
            .released_quotes
            .lock()
            .expect("test lock")
            .contains(&terminal_quotes[1]),
        "the injected second-member failure interrupted the wave"
    );

    let reopened = open_client(database, payments, mg_state, config).await;
    assert!(reopened.resume().await.is_err());
    let recovered = active_recovery(
        reopened
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        seat_indices(&recovered),
        (2..MIN_FEDERATION_SIZE).collect::<Vec<_>>(),
        "reopen repeats idempotent releases and atomically drops the full subset"
    );
    assert!(terminal_quotes.iter().all(|quote_id| {
        payment_state
            .released_quotes
            .lock()
            .expect("test lock")
            .contains(quote_id)
    }));
}

#[tokio::test]
async fn freshly_funded_signed_refusal_clears_the_presented_quote_in_one_run() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;
    mg_state
        .refused_create_indices
        .lock()
        .expect("test lock")
        .insert(0);

    assert!(matches!(
        client
            .buy_seats_at(&intent(), locators(), options())
            .await
            .as_ref(),
        Err(FaError::Mg(MgFailure {
            cause: MgCause::OfferChanged,
            ..
        }))
    ));
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        seat_indices(&recovery),
        (1..MIN_FEDERATION_SIZE).collect::<Vec<_>>()
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(payment_state.refund_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn mixed_prepared_and_rejected_recovery_waits_for_concurrent_replay_barrier() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let config = MgConfig::paid();
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;
    let quote_ids = seed_armed_selected_purchase(&client, &payment_state)
        .await
        .seats
        .iter()
        .map(|seat| seat.quote_id)
        .collect::<Vec<_>>();
    payment_state
        .funded_quotes
        .lock()
        .expect("test lock")
        .extend(quote_ids.iter().copied().take(quote_ids.len() - 1));
    payment_state
        .rejected_quotes
        .lock()
        .expect("test lock")
        .insert(*quote_ids.last().expect("at least one quote"));
    payment_state.barrier_recovery.store(true, Ordering::SeqCst);
    mg_state.block_accepts.store(true, Ordering::SeqCst);

    let running = client.clone();
    let operation =
        tokio::spawn(async move { running.resume_with_options(long_request_options()).await });
    wait_for_count(
        &payment_state.recover_calls,
        usize::from(MIN_FEDERATION_SIZE),
    )
    .await;
    assert_eq!(mg_state.create_calls.load(Ordering::SeqCst), 0);
    payment_state.release_recovery.store(true, Ordering::SeqCst);
    payment_state.recovery_release.notify_waiters();
    wait_for_count(&mg_state.create_calls, usize::from(MIN_FEDERATION_SIZE) - 1).await;
    let interrupted = active_recovery(
        client
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    assert!(
        interrupted
            .seats
            .iter()
            .all(|seat| seat.signed_quote.is_some())
    );
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());

    payment_state
        .barrier_recovery
        .store(false, Ordering::SeqCst);
    mg_state.block_accepts.store(false, Ordering::SeqCst);
    mg_state.release_accepts.store(true, Ordering::SeqCst);
    mg_state.create_release.notify_waiters();
    let reopened = open_client(database, payments, mg_state, config).await;
    assert!(matches!(reopened.resume().await, Err(FaError::Payment(_))));
    let recovered = active_recovery(
        reopened
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        seat_indices(&recovered),
        (0..MIN_FEDERATION_SIZE - 1).collect::<Vec<_>>()
    );
    assert!(
        recovered
            .seats
            .iter()
            .all(|seat| seat.progress.seat_id.is_some())
    );
}

#[tokio::test]
async fn transient_sibling_failure_preserves_rejected_quote_recovery_entitlement() {
    #[derive(Clone, Copy)]
    enum SiblingFailure {
        Recovery,
        Presentation,
        Checkpoint,
    }

    for failure_kind in [
        SiblingFailure::Recovery,
        SiblingFailure::Presentation,
        SiblingFailure::Checkpoint,
    ] {
        let database = MemDatabase::new().into_database();
        let (payments, payment_state) = TestPayments::new();
        let mg_state = Arc::new(MgState::default());
        let config = MgConfig::paid();
        let client =
            open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;
        let requirements = seed_armed_selected_purchase(&client, &payment_state).await;
        let authorization_id = requirements.authorization_id.clone();
        let quote_ids = requirements
            .seats
            .iter()
            .map(|seat| seat.quote_id)
            .collect::<Vec<_>>();
        payment_state
            .funded_quotes
            .lock()
            .expect("test lock")
            .extend(quote_ids.iter().copied().take(quote_ids.len() - 1));
        payment_state
            .rejected_quotes
            .lock()
            .expect("test lock")
            .insert(*quote_ids.last().expect("at least one quote"));
        match failure_kind {
            SiblingFailure::Recovery => {
                payment_state
                    .failed_recovery_quotes
                    .lock()
                    .expect("test lock")
                    .insert(quote_ids[0]);
            }
            SiblingFailure::Presentation => {
                mg_state
                    .failed_create_quotes
                    .lock()
                    .expect("test lock")
                    .insert(quote_ids[0]);
            }
            SiblingFailure::Checkpoint => {
                client.inner.store.fail_seat_checkpoint(0);
            }
        }
        let funding_calls = payment_state.create_calls.load(Ordering::SeqCst);
        let payable_calls = payment_state.payable_calls.load(Ordering::SeqCst);

        assert!(client.resume_with_options(options()).await.is_err());
        let interrupted = active_recovery(
            client
                .inner
                .store
                .load_purchase(TestIdentity::fa_id())
                .await
                .unwrap(),
        );
        assert!(interrupted.seats[0].signed_quote.is_some());
        assert!(
            interrupted.seats[quote_ids.len() - 1]
                .signed_quote
                .is_some()
        );
        assert!(
            interrupted.seats[1..quote_ids.len() - 1]
                .iter()
                .all(|seat| seat.progress.seat_id.is_some())
        );
        let (recovered_authorization_id, recovered_authorizations) = interrupted
            .test_payment_authorization()
            .expect("original aggregate authorization remains durable");
        assert_eq!(recovered_authorization_id, &authorization_id);
        assert_eq!(
            recovered_authorizations
                .iter()
                .map(|authorization| authorization.quote_id)
                .collect::<Vec<_>>(),
            quote_ids
        );
        payment_state
            .failed_recovery_quotes
            .lock()
            .expect("test lock")
            .clear();
        mg_state
            .failed_create_quotes
            .lock()
            .expect("test lock")
            .clear();
        let reopened = open_client(database, payments, mg_state.clone(), config).await;
        assert!(matches!(reopened.resume().await, Err(FaError::Payment(_))));
        assert_eq!(
            payment_state.create_calls.load(Ordering::SeqCst),
            funding_calls
        );
        assert_eq!(
            payment_state.payable_calls.load(Ordering::SeqCst),
            payable_calls
        );
        let failed_quote_presentations = mg_state
            .create_records
            .lock()
            .expect("test lock")
            .iter()
            .filter(|record| record.quote_id == quote_ids[0])
            .count();
        let expected_presentations = match failure_kind {
            SiblingFailure::Recovery => 1,
            SiblingFailure::Presentation | SiblingFailure::Checkpoint => 2,
        };
        assert_eq!(failed_quote_presentations, expected_presentations);
        let recovered = active_recovery(
            reopened
                .inner
                .store
                .load_purchase(TestIdentity::fa_id())
                .await
                .unwrap(),
        );
        assert!(recovered.seats[0].progress.seat_id.is_some());
        assert_eq!(
            seat_indices(&recovered),
            (0..MIN_FEDERATION_SIZE - 1).collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn fund_new_failure_preserves_signed_refusal_quote_and_authorization_for_reopen() {
    enum FundNewFailure {
        Presentation,
        Checkpoint,
    }

    for failure in [FundNewFailure::Presentation, FundNewFailure::Checkpoint] {
        let database = MemDatabase::new().into_database();
        let (payments, payment_state) = TestPayments::new();
        let mg_state = Arc::new(MgState::default());
        let registry = TestRegistry::default();
        let now = test_now_secs();
        *registry.candidates.lock().expect("test lock") =
            vec![setup_payment_event(now, &[PAYMENT_INVITE])];
        let config = MgConfig::paid();
        let client = open_client_with_registry(
            database.clone(),
            payments.clone(),
            mg_state.clone(),
            config,
            registry.clone(),
        )
        .await;
        let authorization_id = seed_authorized_selected_purchase(&client)
            .await
            .authorization_id;
        mg_state
            .refused_create_indices
            .lock()
            .expect("test lock")
            .insert(usize::from(MIN_FEDERATION_SIZE) - 1);
        match failure {
            FundNewFailure::Presentation => {
                mg_state
                    .failed_create_indices
                    .lock()
                    .expect("test lock")
                    .insert(0);
            }
            FundNewFailure::Checkpoint => client.inner.store.fail_seat_checkpoint(0),
        }

        assert!(client.resume_with_options(options()).await.is_err());
        let interrupted = active_recovery(
            client
                .inner
                .store
                .load_purchase(TestIdentity::fa_id())
                .await
                .unwrap(),
        );
        let refused_position = usize::from(MIN_FEDERATION_SIZE) - 1;
        assert!(interrupted.seats[0].progress.seat_id.is_none());
        assert!(
            interrupted.seats[1..refused_position]
                .iter()
                .all(|seat| seat.progress.seat_id.is_none()),
            "a failed first paid seat prevents later wallet outputs from starting"
        );
        assert!(
            interrupted.seats[refused_position]
                .progress
                .seat_id
                .is_none()
        );
        let failed_signed_quote = interrupted.seats[0]
            .signed_quote
            .clone()
            .expect("failed seat retains its exact quote");
        let failed_quote_id = failed_signed_quote
            .verify(&interrupted.seats[0].progress.locator.service_pubkey)
            .expect("failed quote remains verified")
            .quote_id();
        assert!(interrupted.seats[refused_position].signed_quote.is_some());
        let (recovered_authorization_id, recovered_authorizations) = interrupted
            .test_payment_authorization()
            .expect("aggregate authorization remains durable");
        assert_eq!(recovered_authorization_id, &authorization_id);
        let recoverable_quote_ids = recovered_authorizations
            .iter()
            .map(|authorization| authorization.quote_id)
            .collect::<HashSet<_>>();
        assert!(
            interrupted
                .seats
                .iter()
                .filter(|seat| seat.progress.seat_id.is_none())
                .all(|seat| recoverable_quote_ids.contains(
                    &seat
                        .signed_quote
                        .as_ref()
                        .expect("uncheckpointed seat retains its exact quote")
                        .verify(&seat.progress.locator.service_pubkey)
                        .expect("retained quote remains verified")
                        .quote_id()
                ))
        );
        assert!(
            recoverable_quote_ids.contains(
                &interrupted.seats[refused_position]
                    .signed_quote
                    .as_ref()
                    .expect("refused quote remains stored")
                    .verify(
                        &interrupted.seats[refused_position]
                            .progress
                            .locator
                            .service_pubkey,
                    )
                    .expect("stored quote remains verified")
                    .quote_id()
            )
        );
        let funding_calls = payment_state.create_calls.load(Ordering::SeqCst);
        let payable_calls = payment_state.payable_calls.load(Ordering::SeqCst);
        let quote_calls = mg_state.quote_calls.load(Ordering::SeqCst);

        mg_state
            .failed_create_indices
            .lock()
            .expect("test lock")
            .clear();
        *registry.candidates.lock().expect("test lock") = vec![setup_payment_event(now + 1, &[])];
        let reopened = open_client_with_registry(
            database,
            payments,
            mg_state.clone(),
            config,
            registry.clone(),
        )
        .await;
        assert!(matches!(
            reopened.resume().await,
            Err(FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SelectedPayerUnavailable,
                failure: None,
            })
        ));
        assert_eq!(
            payment_state.create_calls.load(Ordering::SeqCst),
            funding_calls
        );
        assert_eq!(
            payment_state.payable_calls.load(Ordering::SeqCst),
            payable_calls
        );
        assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), quote_calls);
        let recovered = active_recovery(
            reopened
                .inner
                .store
                .load_purchase(TestIdentity::fa_id())
                .await
                .unwrap(),
        );
        assert!(recovered.seats[0].progress.seat_id.is_some());
        assert!(
            recovered.seats[1..]
                .iter()
                .all(|seat| seat.progress.seat_id.is_none()),
            "the exact first payment is recovered before current policy stops unstarted outputs"
        );
        assert!(recovered.test_payment_authorization().is_some());
        assert!(recovered.payment_reservation_id.is_some());

        *registry.candidates.lock().expect("test lock") =
            vec![setup_payment_event(now + 2, &[PAYMENT_INVITE])];
        assert!(matches!(
            reopened.resume().await.as_ref(),
            Err(FaError::Mg(MgFailure {
                cause: MgCause::OfferChanged,
                ..
            }))
        ));
        let recovered = active_recovery(
            reopened
                .inner
                .store
                .load_purchase(TestIdentity::fa_id())
                .await
                .unwrap(),
        );
        assert_eq!(
            seat_indices(&recovered),
            (0..MIN_FEDERATION_SIZE - 1).collect::<Vec<_>>(),
            "the refused seat left the purchase",
        );
        assert!(
            recovered
                .seats
                .iter()
                .all(|seat| seat.progress.seat_id.is_some())
        );
        let create_records = mg_state.create_records.lock().expect("test lock");
        let failed_presentations = create_records
            .iter()
            .filter(|record| record.quote_id == failed_quote_id)
            .map(|record| &record.signed_quote)
            .collect::<Vec<_>>();
        assert_eq!(failed_presentations.len(), 2);
        assert!(
            failed_presentations
                .iter()
                .all(|signed_quote| **signed_quote == failed_signed_quote)
        );
    }
}

#[tokio::test]
async fn mixed_prepared_and_signed_refusal_survives_interrupted_replay_wave() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let mut config = MgConfig::paid();
    config.create_behavior = CreateBehavior::RefuseFirstQuote;
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;
    let quote_ids = seed_armed_selected_purchase(&client, &payment_state)
        .await
        .seats
        .iter()
        .map(|seat| seat.quote_id)
        .collect::<Vec<_>>();
    payment_state
        .funded_quotes
        .lock()
        .expect("test lock")
        .extend(quote_ids.iter().copied());
    mg_state.block_accepts.store(true, Ordering::SeqCst);

    let mut operation = Box::pin(client.resume_with_options(long_request_options()));
    assert!(futures::poll!(&mut operation).is_pending());
    assert_eq!(
        mg_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
    let refused_quote = mg_state
        .refused_quote
        .lock()
        .expect("test lock")
        .expect("one quote refused");
    let interrupted = active_recovery(
        client
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    assert!(interrupted.seats.iter().any(|seat| {
        seat.signed_quote
            .as_ref()
            .and_then(|quote| quote.verify(&seat.progress.locator.service_pubkey).ok())
            .is_some_and(|quote| quote.quote_id() == refused_quote)
    }));
    drop(operation);

    mg_state.block_accepts.store(false, Ordering::SeqCst);
    mg_state.release_accepts.store(true, Ordering::SeqCst);
    mg_state.create_release.notify_waiters();
    let reopened = open_client(database, payments, mg_state, config).await;
    let resume = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match reopened.resume().await {
                Err(FaError::Busy) => tokio::task::yield_now().await,
                result => break result,
            }
        }
    })
    .await
    .expect("cancelled driver released its lease");
    assert!(
        matches!(
            resume.as_ref(),
            Err(FaError::Mg(MgFailure {
                cause: MgCause::OfferChanged,
                ..
            }))
        ),
        "unexpected resume result: {resume:?}"
    );
    let recovered = active_recovery(
        reopened
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    let refused_position = quote_ids
        .iter()
        .position(|quote_id| *quote_id == refused_quote)
        .expect("refused quote belongs to formation");
    assert_eq!(
        seat_indices(&recovered),
        (0..MIN_FEDERATION_SIZE)
            .filter(|index| usize::from(*index) != refused_position)
            .collect::<Vec<_>>(),
        "the refused seat left the purchase",
    );
    assert!(
        recovered
            .seats
            .iter()
            .all(|seat| seat.progress.seat_id.is_some())
    );
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
        let quote_ids = seed_armed_selected_purchase(&client, &payment_state)
            .await
            .seats
            .iter()
            .map(|seat| seat.quote_id)
            .collect::<Vec<_>>();
        payment_state
            .funded_quotes
            .lock()
            .expect("test lock")
            .extend(quote_ids);

        let create_started = mg_state.create_started.notified();
        let running = client.clone();
        let operation =
            tokio::spawn(async move { running.resume_with_options(long_request_options()).await });
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
        reopened.resume().await.unwrap();
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
        reopened.resume().await,
        Err(FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SelectedPayerUnavailable,
            failure: None,
        })
    ));
    assert!(purchase(&reopened.status()).payment_outputs_started);
    assert_eq!(mg_state.create_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        mg_state.allocated_quotes.lock().expect("test lock").len(),
        1
    );
}

#[tokio::test]
async fn terminal_paid_member_waits_for_held_siblings_before_clearing_aggregate() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let config = MgConfig {
        create_behavior: CreateBehavior::HangFirst,
        ..MgConfig::paid()
    };
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;
    let create_started = mg_state.create_started.notified();
    let running = client.clone();
    let operation = tokio::spawn(async move {
        running
            .buy_and_form(
                intent(),
                &running.preview_at(&intent(), locators()),
                payment_federation_id(),
                long_request_options(),
            )
            .await
    });
    create_started.await;
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());

    let terminal_quote = *payment_state
        .created_quotes
        .lock()
        .expect("test lock")
        .first()
        .expect("one wallet output was journaled before the crash");
    assert_eq!(
        payment_state.funded_quotes.lock().expect("test lock").len(),
        1,
    );
    payment_state
        .rejected_quotes
        .lock()
        .expect("test lock")
        .insert(terminal_quote);
    drop(client);

    let reopened = open_client(database, payments, mg_state.clone(), config).await;
    let error = reopened
        .resume()
        .await
        .expect_err("the exact terminal member leaves the purchase");
    assert!(matches!(error, FaError::Payment(_)), "{error:?}");
    assert_eq!(
        payment_state.funded_quotes.lock().expect("test lock").len(),
        usize::from(MIN_FEDERATION_SIZE),
        "every held sibling starts exactly once before the aggregate is cleared",
    );
    let interrupted = active_recovery(
        reopened
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    let dropped = dropped_index(&interrupted.snapshot);
    assert_eq!(
        interrupted.seats.len(),
        usize::from(MIN_FEDERATION_SIZE) - 1,
        "the terminal member alone leaves the purchase",
    );
    assert!(
        interrupted
            .seats
            .iter()
            .all(|seat| seat.progress.seat_id.is_some()),
        "no held sibling is stranded when the terminal subset is dropped",
    );
    assert!(interrupted.test_payment_authorization().is_none());
    assert!(interrupted.payment_reservation_id.is_none());
    {
        let reservations = payment_state.reservations.lock().expect("test lock");
        let journal = reservations
            .values()
            .next()
            .expect("the aggregate wallet journal remains inspectable");
        assert_eq!(journal.started.len(), usize::from(MIN_FEDERATION_SIZE));
        assert_eq!(journal.released, HashSet::from([terminal_quote]));
    }

    let presentations = mg_state.create_calls.load(Ordering::SeqCst);

    reopened.resume().await.unwrap();
    bought_without(&reopened.status(), &[dropped]);
    assert_eq!(
        payment_state.funded_quotes.lock().expect("test lock").len(),
        usize::from(MIN_FEDERATION_SIZE),
        "the dropped member never gets a second wallet output",
    );
    assert_eq!(mg_state.create_calls.load(Ordering::SeqCst), presentations);
}

#[tokio::test]
async fn post_output_free_refusal_drops_only_that_seat_and_the_rest_complete() {
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
            FaError::Mg(MgFailure {
                cause: MgCause::OfferChanged,
                ..
            })
        ),
        "{error:?}"
    );
    let interrupted = purchase(&client.status()).clone();
    assert!(interrupted.payment_outputs_started);
    assert_eq!(dropped_index(&interrupted), u16::try_from(free).unwrap());
    assert!(
        interrupted.seats.iter().all(|seat| seat.seat_id.is_some()),
        "every paid sibling was accepted",
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        paid_seats
    );
    drop(client);

    let reopened = open_client(database, payments, mg_state.clone(), config).await;
    let presentations = mg_state.create_calls.load(Ordering::SeqCst);
    reopened.resume().await.unwrap();
    bought_without(&reopened.status(), &[u16::try_from(free).unwrap()]);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        paid_seats
    );
    assert_eq!(mg_state.create_calls.load(Ordering::SeqCst), presentations);
}
