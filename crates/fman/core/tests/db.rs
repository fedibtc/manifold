use super::*;

#[tokio::test]
async fn telemetry_generation_exhaustion_preserves_the_last_durable_value() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).await.unwrap();
    sqlx::query("UPDATE telemetry_capability SET generation = ? WHERE id = 1")
        .bind(i64::MAX)
        .execute(db.pool())
        .await
        .unwrap();

    assert!(matches!(
        db.rotate_telemetry_capability_generation().await,
        Err(DbError::TelemetryGenerationExhausted)
    ));
    assert_eq!(
        db.telemetry_capability_generation().await.unwrap(),
        i64::MAX as u64,
        "a refused rotation must not corrupt or wrap durable state"
    );
}

#[tokio::test]
async fn a_data_root_is_bound_to_its_first_manifold_environment() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_owned();
    let db = Db::open(&path).await.unwrap();

    db.bind_manifold_environment(ManifoldEnvironment::Development)
        .await
        .unwrap();
    db.bind_manifold_environment(ManifoldEnvironment::Development)
        .await
        .unwrap();
    sqlx::query("UPDATE manifold_environment SET environment = 'production' WHERE id = 1")
        .execute(db.pool())
        .await
        .expect_err("the durable environment binding is immutable");
    sqlx::query("DELETE FROM manifold_environment WHERE id = 1")
        .execute(db.pool())
        .await
        .expect_err("the durable environment binding cannot be removed");
    let mut connection = db.pool().acquire().await.unwrap();
    sqlx::query("PRAGMA recursive_triggers = OFF")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query(
        "INSERT OR REPLACE INTO manifold_environment (id, environment) \
         VALUES (1, 'production')",
    )
    .execute(&mut *connection)
    .await
    .expect_err("replacement cannot bypass binding immutability");
    drop(connection);
    assert!(matches!(
        db.bind_manifold_environment(ManifoldEnvironment::Production)
            .await,
        Err(DbError::ManifoldEnvironmentMismatch {
            bound,
            selected: ManifoldEnvironment::Production,
        }) if bound == "development"
    ));

    drop(db);
    let reopened = Db::open(&path).await.unwrap();
    assert!(matches!(
        reopened
            .bind_manifold_environment(ManifoldEnvironment::Staging)
            .await,
        Err(DbError::ManifoldEnvironmentMismatch {
            bound,
            selected: ManifoldEnvironment::Staging,
        }) if bound == "development"
    ));
}

#[tokio::test]
async fn a_second_database_open_on_the_same_data_root_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_owned();
    let _first = Db::open(&path).await.unwrap();
    let error = Db::open(&path).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("another Fleet Manager instance already runs"),
        "unexpected second-open error: {error:#}"
    );
}

#[tokio::test]
async fn holder_authorization_events_merge_monotonically_and_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_owned();
    let db = Db::open(&path).await.unwrap();
    let first_digest = vec![1; 32];
    let second_digest = vec![2; 32];

    db.merge_holder_authorization_events(
        &[
            (first_digest.clone(), 10, "first".to_owned()),
            (second_digest, 20, "second".to_owned()),
        ],
        100,
    )
    .await
    .unwrap();
    db.merge_holder_authorization_events(&[(first_digest.clone(), 11, "newer".to_owned())], 100)
        .await
        .unwrap();
    db.merge_holder_authorization_events(&[(first_digest, 10, "older".to_owned())], 100)
        .await
        .unwrap();
    db.merge_holder_authorization_events(&[], 100)
        .await
        .unwrap();

    drop(db);
    let reopened = Db::open(&path).await.unwrap();
    assert_eq!(
        reopened
            .bounded_holder_authorization_event_jsons(100)
            .await
            .unwrap(),
        vec!["newer", "second"]
    );
}

#[tokio::test]
async fn holder_authorization_retention_enforces_exact_aggregate_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_owned();
    let db = Db::open(&path).await.unwrap();
    let limit = fedi_decentralized_domain::FMAN_HOLDER_AUTHORIZATION_RETENTION_MAX_COUNT;
    let initial = (0..limit)
        .map(|index| {
            let mut digest = vec![0; 32];
            digest[24..].copy_from_slice(&(index as u64).to_be_bytes());
            (digest, 10, format!("event-{index}"))
        })
        .collect::<Vec<_>>();
    db.merge_holder_authorization_events(&initial, 100)
        .await
        .unwrap();

    let replay_digest = initial[0].0.clone();
    db.merge_holder_authorization_events(
        &[
            (replay_digest, 11, "updated-existing".to_owned()),
            (vec![0xff; 32], 11, "over-limit".to_owned()),
        ],
        100,
    )
    .await
    .unwrap();

    drop(db);
    let reopened = Db::open(&path).await.unwrap();
    let retained = reopened
        .bounded_holder_authorization_event_jsons(100)
        .await
        .unwrap();
    assert_eq!(retained.len(), limit);
    assert!(retained.iter().any(|event| event == "updated-existing"));
    assert!(!retained.iter().any(|event| event == "over-limit"));
}

#[tokio::test]
async fn holder_authorization_future_rows_fail_closed_and_legacy_rows_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_owned();
    let db = Db::open(&path).await.unwrap();
    let digest = vec![3; 32];

    let err = db
        .merge_holder_authorization_events(&[(digest.clone(), 101, "future".to_owned())], 100)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        DbError::HolderAuthorizationIssuedAtTooFarFuture
    ));
    assert!(
        db.bounded_holder_authorization_event_jsons(100)
            .await
            .unwrap()
            .is_empty()
    );

    sqlx::query(
        "INSERT INTO holder_authorization_events
         (credential_digest, authorization_issued_at, event_json) VALUES (?, ?, ?)",
    )
    .bind(&digest)
    .bind(u64::MAX.to_be_bytes().to_vec())
    .bind("legacy-future")
    .execute(db.pool())
    .await
    .unwrap();
    drop(db);

    let reopened = Db::open(&path).await.unwrap();
    assert!(
        reopened
            .bounded_holder_authorization_event_jsons(100)
            .await
            .unwrap()
            .is_empty(),
        "startup normalization must remove a pre-fix pin"
    );
    reopened
        .merge_holder_authorization_events(&[(digest, 50, "legitimate".to_owned())], 100)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .bounded_holder_authorization_event_jsons(100)
            .await
            .unwrap(),
        vec!["legitimate"]
    );
}

#[tokio::test]
async fn operator_settings_round_trip_with_friendly_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).await.unwrap();

    // A fresh FMan sells nothing until its operator says what it sells.
    assert_eq!(stored_price_msats(&db).await, None);
    assert_eq!(db.payout_destination().await.unwrap(), None);

    db.set_payout_destination(Some("operator@example.com"))
        .await
        .unwrap();
    assert_eq!(
        db.payout_destination().await.unwrap().as_deref(),
        Some("operator@example.com")
    );
    db.set_payout_destination(None).await.unwrap();
    assert_eq!(db.payout_destination().await.unwrap(), None);

    let price = Msats(10_000_000);
    let initial_epoch = db.offer_epoch().await.unwrap();
    let plans_epoch = db.set_offered_price(Some(price)).await.unwrap();
    assert_ne!(plans_epoch, initial_epoch);
    assert_eq!(
        db.set_offered_price(Some(price)).await.unwrap(),
        plans_epoch
    );
    assert_eq!(stored_price_msats(&db).await, Some(price.0 as i64));
}

#[tokio::test]
async fn setup_payment_policy_replacement_bumps_the_epoch_only_on_removal() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).await.unwrap();
    let snapshot = |db: &Db| {
        let db = db.clone();
        async move {
            db.offer_snapshot(crate::facts::PortBase::new(30_000).unwrap())
                .await
                .unwrap()
                .offer
        }
    };

    // A fresh FMan retains no publication and accepts no payment federation.
    assert_eq!(db.setup_payment_event_json().await.unwrap(), None);
    assert!(snapshot(&db).await.settings.payment_federations.is_empty());
    let initial_epoch = db.offer_epoch().await.unwrap();

    // Additions retain the event without invalidating outstanding quotes.
    db.replace_setup_payment_policy(r#"{"event":1}"#, &[FederationId("fed1".to_owned())])
        .await
        .unwrap();
    assert_eq!(
        db.setup_payment_event_json().await.unwrap().as_deref(),
        Some(r#"{"event":1}"#)
    );
    db.replace_setup_payment_policy(
        r#"{"event":2}"#,
        &[
            FederationId("fed1".to_owned()),
            FederationId("fed2".to_owned()),
        ],
    )
    .await
    .unwrap();
    let offer = snapshot(&db).await;
    assert_eq!(
        offer.settings.payment_federations,
        vec![
            FederationId("fed1".to_owned()),
            FederationId("fed2".to_owned()),
        ]
    );
    assert_eq!(offer.epoch, initial_epoch);

    // A removal draws a fresh epoch in the same commit, refusing (and
    // refunding) every quote minted while the member was accepted.
    db.replace_setup_payment_policy(r#"{"event":3}"#, &[FederationId("fed2".to_owned())])
        .await
        .unwrap();
    let removal_epoch = db.offer_epoch().await.unwrap();
    assert_ne!(removal_epoch, initial_epoch);
    assert_eq!(
        snapshot(&db).await.settings.payment_federations,
        vec![FederationId("fed2".to_owned())]
    );

    // Re-admitting the identical membership is idempotent for the epoch.
    db.replace_setup_payment_policy(r#"{"event":3}"#, &[FederationId("fed2".to_owned())])
        .await
        .unwrap();
    assert_eq!(db.offer_epoch().await.unwrap(), removal_epoch);

    // An empty set stops all new paid setup and is itself a removal.
    db.replace_setup_payment_policy(r#"{"event":4}"#, &[])
        .await
        .unwrap();
    assert_ne!(db.offer_epoch().await.unwrap(), removal_epoch);
    assert!(snapshot(&db).await.settings.payment_federations.is_empty());
}

/// The identity is written once, by onboarding, and never by an open: a
/// daemon that finds no identity has not been onboarded, and a second install
/// fails on the primary key rather than re-keying a fleet.
#[tokio::test]
async fn an_identity_is_installed_once_and_never_created_by_opening() {
    let temp = tempfile::TempDir::new().unwrap();
    let db = Db::open(temp.path()).await.unwrap();
    assert!(db.load_identity().await.unwrap().is_none());

    let identity = RootMnemonic::generate().unwrap();
    db.install_identity(&identity).await.unwrap();
    assert_eq!(
        db.load_identity().await.unwrap().unwrap().phrase(),
        identity.phrase()
    );
    assert!(
        db.install_identity(&RootMnemonic::generate().unwrap())
            .await
            .is_err()
    );
    assert_eq!(
        db.load_identity().await.unwrap().unwrap().phrase(),
        identity.phrase()
    );
}

#[tokio::test]
async fn onboarding_progress_is_durable_and_ordered() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().to_owned();
    let db = Db::open(&path).await.unwrap();
    assert_eq!(
        db.onboarding_stage().await.unwrap(),
        crate::db::OnboardingStage::Identity
    );

    db.install_identity(&RootMnemonic::generate().unwrap())
        .await
        .unwrap();
    db.merge_holder_authorization_events(&[(vec![1; 32], 1, "event".to_owned())], 100)
        .await
        .unwrap();
    assert_eq!(
        db.onboarding_stage().await.unwrap(),
        crate::db::OnboardingStage::InitialOffer
    );
    db.configure_initial_offer(Some(Msats(12)), 3)
        .await
        .unwrap();
    drop(db);

    let reopened = Db::open(&path).await.unwrap();
    assert_eq!(
        reopened.onboarding_stage().await.unwrap(),
        crate::db::OnboardingStage::Complete
    );
    assert_eq!(reopened.max_seats().await.unwrap(), 3);
    assert_eq!(stored_price_msats(&reopened).await, Some(12));
}

/// The database stores the offer as a price; the wire states it as plans.
/// `QuoteSettings::plans` is the one place that correspondence lives, so an
/// offer can only ever be the one plan this daemon serves.
#[test]
fn the_stored_price_is_advertised_as_the_one_plan_this_daemon_serves() {
    let settings = |price| QuoteSettings {
        price,
        payment_federations: vec![],
    };
    assert_eq!(settings(None).plans(), vec![]);
    for price_msats in [0, 10_000_000] {
        assert_eq!(
            settings(Some(Msats(price_msats))).plans(),
            vec![Plan::InfiniteBestEffort { price_msats }],
        );
    }
}

/// The stored offer read straight out of the row, so the round-trip test does
/// not depend on a reader that production has no use for.
async fn stored_price_msats(db: &Db) -> Option<i64> {
    sqlx::query_scalar::<_, Option<i64>>("SELECT price_msats FROM offer_state WHERE id = 1")
        .fetch_one(db.pool())
        .await
        .unwrap()
}
