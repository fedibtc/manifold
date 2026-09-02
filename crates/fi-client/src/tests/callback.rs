use super::*;

#[tokio::test]
async fn one_durable_completion_callback_is_sent_to_every_guardian() {
    let (payments, _) = TestPayments::new();
    let state = Arc::new(FmanState::default());
    let database = MemDatabase::new().into_database();
    let client = open_client_that_cannot_pay(
        database.clone(),
        payments,
        state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: "https://push.example/hooks/hook-id/bearer-secret".to_owned(),
        idempotency_key: "formation-dkg-complete".to_owned(),
    })
    .unwrap();

    client
        .create_with_pinned_fmans_and_callback(intent(), locators(), callback.clone(), options())
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
        Arc::new(FmanState::default()),
        FmanConfig::given_away(),
    )
    .await;
    let recovery = match reopened
        .inner
        .store
        .load_recovery(TestIdentity::fi_id())
        .await
        .unwrap()
    {
        FiRecovery::Formation(recovery) => recovery,
        FiRecovery::Idle => panic!("formed recovery remains available"),
    };
    assert_eq!(recovery.snapshot.phase, FormationPhase::Formed);
    assert!(recovery.dkg_completion_callback.is_none());
}

#[tokio::test]
async fn paid_selection_sends_the_callback_to_every_guardian_and_clears_it_at_formed() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
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
        let callbacks = fman_state.start_callbacks.lock().expect("test lock");
        assert_eq!(callbacks.len(), usize::from(MIN_FEDERATION_SIZE));
        assert!(
            callbacks
                .iter()
                .all(|observed| observed.as_ref() == Some(&callback))
        );
    }
    let recovery = match client
        .inner
        .store
        .load_recovery(TestIdentity::fi_id())
        .await
        .unwrap()
    {
        FiRecovery::Formation(recovery) => recovery,
        FiRecovery::Idle => panic!("formed recovery remains available"),
    };
    assert_eq!(recovery.snapshot.phase, FormationPhase::Formed);
    assert!(recovery.dkg_completion_callback.is_none());
}

#[tokio::test]
async fn callback_is_durable_private_state_and_old_schemas_fail_closed() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database.clone(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
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
            TestIdentity::fi_id(),
            FormationId("callback-formation".to_owned()),
            minimum_resolved_intent(),
            minimum_seat_progress(),
            crate::db::FormationCreationMode::Pinned,
            Some(callback.clone()),
        )
        .await
        .unwrap();
    drop(client);
    let (payments, _) = TestPayments::new();
    let reopened = open_client(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    let recovery = match reopened
        .inner
        .store
        .load_recovery(TestIdentity::fi_id())
        .await
        .unwrap()
    {
        FiRecovery::Formation(recovery) => recovery,
        FiRecovery::Idle => panic!("callback formation is durable"),
    };
    assert_eq!(recovery.dkg_completion_callback, Some(callback));
    let public = serde_json::to_string(&recovery.snapshot).unwrap();
    assert!(!public.contains("bearer-secret"));
    assert!(!public.contains("formation-dkg-complete"));

    let (payments, _) = TestPayments::new();
    let migration_database = MemDatabase::new().into_database();
    let migration_client = open_client(
        migration_database.clone(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    migration_client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            FormationId("schema-nine".to_owned()),
            minimum_resolved_intent(),
            minimum_seat_progress(),
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();
    migration_client
        .inner
        .store
        .install_schema_9_fixture_for_test()
        .await;
    assert_eq!(
        migration_client
            .inner
            .store
            .stored_schema_and_callback_field_for_test()
            .await,
        (9, false)
    );
    assert!(matches!(
        migration_client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await,
        Err(FiError::Storage(error))
            if error.contains("unsupported FI storage schema version 9")
                && error.contains("reset this unreleased FI namespace")
    ));

    let (payments, _) = TestPayments::new();
    let future_client = open_client(
        MemDatabase::new().into_database(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    future_client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            FormationId("schema-future".to_owned()),
            minimum_resolved_intent(),
            minimum_seat_progress(),
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();
    future_client
        .inner
        .store
        .set_schema_version_for_test(13)
        .await;
    assert!(matches!(
        future_client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await,
        Err(FiError::Storage(error))
            if error.contains("unsupported FI storage schema version 13")
    ));
}

#[tokio::test]
async fn callback_schema_rejects_missing_current_field_and_hybrid_legacy_bytes() {
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: "https://push.example/hooks/hook-id/bearer-secret".to_owned(),
        idempotency_key: "formation-dkg-complete".to_owned(),
    })
    .unwrap();

    let missing_database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let missing = open_client(
        missing_database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    missing
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            FormationId("missing-current-callback-field".to_owned()),
            minimum_resolved_intent(),
            minimum_seat_progress(),
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();
    missing.inner.store.remove_callback_field_for_test().await;
    assert!(matches!(
        missing
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await,
        Err(FiError::Storage(error))
            if error.contains("schema 12 formation omits dkg_completion_callback")
    ));

    let hybrid_database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let hybrid = open_client(
        hybrid_database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    hybrid
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            FormationId("hybrid-schema-nine".to_owned()),
            minimum_resolved_intent(),
            minimum_seat_progress(),
            crate::db::FormationCreationMode::Pinned,
            Some(callback),
        )
        .await
        .unwrap();
    hybrid.inner.store.set_schema_version_for_test(9).await;
    assert!(matches!(
        hybrid
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await,
        Err(FiError::Storage(error))
            if error.contains("unsupported FI storage schema version 9")
    ));

    let null_hybrid_database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let null_hybrid = open_client(
        null_hybrid_database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    null_hybrid
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            FormationId("null-hybrid-schema-nine".to_owned()),
            minimum_resolved_intent(),
            minimum_seat_progress(),
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();
    null_hybrid.inner.store.set_schema_version_for_test(9).await;
    assert!(matches!(
        null_hybrid
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await,
        Err(FiError::Storage(error))
            if error.contains("unsupported FI storage schema version 9")
    ));
}
