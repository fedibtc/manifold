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
async fn callback_is_durable_private_state_and_future_schemas_fail_closed() {
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
            resolved_intent_with_size(FederationSize(1)),
            vec![seat_progress(0)],
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
            resolved_intent_with_size(FederationSize(1)),
            vec![seat_progress(0)],
            crate::db::FormationCreationMode::Pinned,
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
            .load_recovery(TestIdentity::fi_id())
            .await,
        Err(FiError::Storage(error))
            if error.contains("unsupported FI storage schema version 12")
    ));
}

async fn legacy_formation(
    schema: u16,
    version: &str,
    callback: Option<DkgCompletionCallback>,
) -> (fedimint_core::db::Database, serde_json::Value) {
    let database = MemDatabase::new().into_database();
    let store = crate::db::FiStore::new(database.clone());
    store
        .initialize(
            TestIdentity::fi_id(),
            FormationId(format!("legacy-schema-{schema}")),
            resolved_intent_with_size(FederationSize(1)),
            vec![seat_progress(0)],
            crate::db::FormationCreationMode::Pinned,
            callback,
        )
        .await
        .unwrap();
    store.install_legacy_fixture_for_test(schema, version).await;
    let before = store.active_formation_json_for_test().await;
    (database, before)
}

#[tokio::test]
async fn storage_schemas_nine_and_ten_migrate_once_without_losing_state() {
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: "https://push.example/hooks/hook-id/bearer-secret".to_owned(),
        idempotency_key: "legacy-dkg-complete".to_owned(),
    })
    .unwrap();
    for (schema, version, expected_callback) in [
        (9, "0.11.1-fedi0", None),
        (10, "0.11.1-fedi17", Some(callback.clone())),
        (10, "0.11.1-fedi99999", None),
    ] {
        let (database, before) = legacy_formation(schema, version, expected_callback.clone()).await;
        let store = crate::db::FiStore::new(database.clone());
        let recovery = active_recovery(store.load_recovery(TestIdentity::fi_id()).await.unwrap());
        assert_eq!(
            recovery.snapshot.formation_id.0,
            format!("legacy-schema-{schema}")
        );
        assert_eq!(recovery.snapshot.phase, FormationPhase::Preparing);
        assert_eq!(recovery.seats.len(), 1);
        assert_eq!(recovery.dkg_completion_callback, expected_callback);
        assert_eq!(
            recovery.snapshot.intent.fedimintd_versions,
            FedimintdVersionRange::new("0.11.1".parse().unwrap(), "0.11.3".parse().unwrap())
                .unwrap()
        );
        assert_eq!(
            recovery.snapshot.intent.fedimintd_dkg_version.to_string(),
            "0.11+fedi"
        );

        let migrated = store.active_formation_json_for_test().await;
        assert_eq!(migrated["schema_version"], serde_json::json!(11));
        assert!(
            migrated
                .as_object()
                .unwrap()
                .contains_key("dkg_completion_callback")
        );
        let mut restored = migrated.clone();
        restored["schema_version"] = serde_json::json!(schema);
        {
            let intent = restored["intent"].as_object_mut().unwrap();
            intent.remove("fedimintd_versions");
            intent.remove("fedimintd_dkg_version");
            intent.insert("fedimintd_version".to_owned(), serde_json::json!(version));
        }
        if schema == 9 {
            restored
                .as_object_mut()
                .unwrap()
                .remove("dkg_completion_callback");
        }
        assert_eq!(
            restored, before,
            "migration changed unrelated formation state"
        );

        drop(store);
        let reopened = crate::db::FiStore::new(database);
        reopened.load_recovery(TestIdentity::fi_id()).await.unwrap();
        assert_eq!(
            reopened.active_formation_json_for_test().await,
            migrated,
            "reopen must not rewrite schema 11"
        );
    }
}

#[tokio::test]
async fn legacy_migration_failures_leave_storage_unchanged() {
    for (index, version) in [
        "0.11.1",
        "0.11.1-fedi",
        "0.11.1-fedi1x",
        "0.11.2-fedi17",
        "0.11.1-fedi17+fedi",
    ]
    .into_iter()
    .enumerate()
    {
        let schema = 9 + u16::try_from(index % 2).unwrap();
        let (database, before) = legacy_formation(schema, version, None).await;
        let store = crate::db::FiStore::new(database);
        assert!(matches!(
            store.load_recovery(TestIdentity::fi_id()).await,
            Err(FiError::Storage(error)) if error.contains("unsupported legacy FI Fedimint version")
        ));
        assert_eq!(store.active_formation_json_for_test().await, before);
    }

    let (database, before) = legacy_formation(9, "0.11.1-fedi17", None).await;
    let store = crate::db::FiStore::new(database);
    assert!(matches!(
        store.load_recovery(OtherIdentity.public_key().unwrap()).await,
        Err(FiError::Storage(error)) if error.contains("different identity")
    ));
    assert_eq!(store.active_formation_json_for_test().await, before);
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
            resolved_intent_with_size(FederationSize(1)),
            vec![seat_progress(0)],
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
            if error.contains("schema 11 formation omits dkg_completion_callback")
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
            resolved_intent_with_size(FederationSize(1)),
            vec![seat_progress(0)],
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
            if error.contains("schema 9 unexpectedly contains a DKG completion callback field")
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
            resolved_intent_with_size(FederationSize(1)),
            vec![seat_progress(0)],
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
            if error.contains("schema 9 unexpectedly contains a DKG completion callback field")
    ));
}
