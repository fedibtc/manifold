use super::*;

#[tokio::test]
async fn one_durable_completion_callback_is_sent_to_every_guardian() {
    let (payments, _) = TestPayments::new();
    let state = Arc::new(MgState::default());
    let database = MemDatabase::new().into_database();
    let client = open_client_that_cannot_pay(
        database.clone(),
        payments,
        state.clone(),
        MgConfig::given_away(),
    )
    .await;
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: "https://push.example/hooks/hook-id/bearer-secret".to_owned(),
        idempotency_key: "formation-dkg-complete".to_owned(),
    })
    .unwrap();

    client
        .create_at_with_callback(intent(), locators(), callback.clone(), options())
        .await
        .unwrap();

    {
        let callbacks = state.start_callbacks.lock().expect("test lock");
        assert_eq!(callbacks.len(), usize::from(MIN_FEDERATION_SIZE));
        assert!(
            callbacks
                .iter()
                .all(|observed| observed.as_ref() == Some(&callback))
        );
    }
    drop(client);
    let (payments, _) = TestPayments::new();
    let reopened = open_client_that_cannot_pay(
        database,
        payments,
        Arc::new(MgState::default()),
        MgConfig::given_away(),
    )
    .await;
    // The formed record replaced the formation record and its callback.
    assert!(matches!(
        reopened
            .inner
            .store
            .load_recovery(TestIdentity::fa_id())
            .await
            .unwrap(),
        FaRecovery::Formed(_)
    ));
}

#[tokio::test]
async fn paid_selection_sends_the_callback_to_every_guardian_and_clears_it_at_formed() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: "https://push.example/hooks/hook-id/bearer-secret".to_owned(),
        idempotency_key: "formation-dkg-complete".to_owned(),
    })
    .unwrap();

    client
        .pay_and_create_with_callback(
            intent(),
            selection_approval(1),
            payment_federation_id(),
            callback.clone(),
            options(),
        )
        .await
        .expect("form selected federation with a completion callback");

    {
        let callbacks = mg_state.start_callbacks.lock().expect("test lock");
        assert_eq!(callbacks.len(), usize::from(MIN_FEDERATION_SIZE));
        assert!(
            callbacks
                .iter()
                .all(|observed| observed.as_ref() == Some(&callback))
        );
    }
    // The formed record replaced the formation record and its callback.
    assert!(matches!(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fa_id())
            .await
            .unwrap(),
        FaRecovery::Formed(_)
    ));
}

#[tokio::test]
async fn callback_is_durable_private_state_and_old_schema_resets() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database.clone(),
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: "https://push.example/hooks/hook-id/bearer-secret".to_owned(),
        idempotency_key: "formation-dkg-complete".to_owned(),
    })
    .unwrap();
    client
        .inner
        .store
        .initialize(
            TestIdentity::fa_id(),
            FormationId("callback-formation".to_owned()),
            resolved_intent_with_size(FederationSize(1)),
            vec![selected_initial_seat(0, test_now_secs() + 120)],
            None,
            Some(callback.clone()),
        )
        .await
        .unwrap();
    drop(client);
    let (payments, _) = TestPayments::new();
    let reopened = open_client(
        database,
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;
    let recovery = match reopened
        .inner
        .store
        .load_recovery(TestIdentity::fa_id())
        .await
        .unwrap()
    {
        FaRecovery::Formation(recovery) => recovery,
        FaRecovery::Idle | FaRecovery::Formed(_) => panic!("callback formation is durable"),
    };
    assert_eq!(recovery.dkg_completion_callback, Some(callback));
    let public = serde_json::to_string(&recovery.snapshot).unwrap();
    assert!(!public.contains("bearer-secret"));
    assert!(!public.contains("formation-dkg-complete"));

    let legacy_database = MemDatabase::new().into_database();
    let legacy_store = crate::db::FaStore::new(legacy_database.clone());
    legacy_store.install_schema_9_fixture_for_test().await;
    let (payments, _) = TestPayments::new();
    let reset = open_client(
        legacy_database,
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;
    assert_eq!(reset.status(), FaStatus::Idle);
    assert!(legacy_store.raw_namespace_is_empty_for_test().await);

    let (payments, _) = TestPayments::new();
    let future_client = open_client(
        MemDatabase::new().into_database(),
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;
    future_client
        .inner
        .store
        .initialize(
            TestIdentity::fa_id(),
            FormationId("schema-future".to_owned()),
            resolved_intent_with_size(FederationSize(1)),
            vec![selected_initial_seat(0, test_now_secs() + 120)],
            None,
            None,
        )
        .await
        .unwrap();
    future_client
        .inner
        .store
        .set_schema_version_for_test(12)
        .await;
    assert!(matches!(
        future_client
            .inner
            .store
            .load_recovery(TestIdentity::fa_id())
            .await,
        Err(FaError::Storage(error))
            if error.contains("unsupported FA storage schema version 12")
    ));
    assert!(
        !future_client
            .inner
            .store
            .raw_namespace_is_empty_for_test()
            .await
    );

    let malformed_store = crate::db::FaStore::new(MemDatabase::new().into_database());
    malformed_store.install_raw_formation_for_test(b"{").await;
    assert!(matches!(
        malformed_store.load_recovery(TestIdentity::fa_id()).await,
        Err(FaError::Storage(error))
            if error.contains("persisted FA formation header is malformed")
    ));
    assert!(!malformed_store.raw_namespace_is_empty_for_test().await);
}

#[tokio::test]
async fn callback_schema_rejects_missing_current_field() {
    let missing_database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let missing = open_client(
        missing_database,
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;
    missing
        .inner
        .store
        .initialize(
            TestIdentity::fa_id(),
            FormationId("missing-current-callback-field".to_owned()),
            resolved_intent_with_size(FederationSize(1)),
            vec![selected_initial_seat(0, test_now_secs() + 120)],
            None,
            None,
        )
        .await
        .unwrap();
    missing.inner.store.remove_callback_field_for_test().await;
    assert!(matches!(
        missing
            .inner
            .store
            .load_recovery(TestIdentity::fa_id())
            .await,
        Err(FaError::Storage(error))
            if error.contains("schema 11 formation omits dkg_completion_callback")
    ));
}
