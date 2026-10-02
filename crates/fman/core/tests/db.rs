use super::*;

#[tokio::test]
async fn pre_origin_identities_migrate_to_restored() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::raw_sql(
        "CREATE TABLE identity (\
             id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1), \
             mnemonic TEXT NOT NULL, \
             created_at_ms INTEGER NOT NULL\
         ); \
         INSERT INTO identity (id, mnemonic, created_at_ms) VALUES (1, 'old', 0);",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::raw_sql(include_str!("../migrations/0002_wallet_origin.sql"))
        .execute(&pool)
        .await
        .unwrap();

    let origin: String = sqlx::query_scalar("SELECT wallet_origin FROM identity WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(origin, "restored");
}

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
            .contains("another Manifold Fedimint Guardian instance already runs"),
        "unexpected second-open error: {error:#}"
    );
}

#[tokio::test]
async fn holder_authorization_keeps_only_the_newest_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_owned();
    let db = Db::open(&path).await.unwrap();

    // A different credential or holder replaces the retained one when newer.
    db.replace_holder_authorization_event(10, "first", 100)
        .await
        .unwrap();
    db.replace_holder_authorization_event(20, "second", 100)
        .await
        .unwrap();
    // Equal or older statements cannot roll it back.
    db.replace_holder_authorization_event(20, "equal", 100)
        .await
        .unwrap();
    db.replace_holder_authorization_event(15, "older", 100)
        .await
        .unwrap();

    drop(db);
    let reopened = Db::open(&path).await.unwrap();
    assert_eq!(
        reopened.holder_authorization_event_json(100).await.unwrap(),
        Some("second".to_owned())
    );
}

#[tokio::test]
async fn holder_authorization_future_rows_fail_closed_and_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_owned();
    let db = Db::open(&path).await.unwrap();

    let err = db
        .replace_holder_authorization_event(101, "future", 100)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        DbError::HolderAuthorizationIssuedAtTooFarFuture
    ));
    assert_eq!(db.holder_authorization_event_json(100).await.unwrap(), None);

    sqlx::query(
        "INSERT INTO holder_authorization (id, authorization_issued_at, event_json) \
         VALUES (1, ?, ?)",
    )
    .bind(u64::MAX.to_be_bytes().to_vec())
    .bind("pinned-future")
    .execute(db.pool())
    .await
    .unwrap();
    drop(db);

    let reopened = Db::open(&path).await.unwrap();
    // A future-dated row must not block a legitimate replacement.
    reopened
        .replace_holder_authorization_event(50, "legitimate", 100)
        .await
        .unwrap();
    assert_eq!(
        reopened.holder_authorization_event_json(100).await.unwrap(),
        Some("legitimate".to_owned())
    );
}

#[tokio::test]
async fn per_credential_holder_authorizations_migrate_to_the_newest() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::raw_sql(
        "CREATE TABLE holder_authorization_events (\
             credential_digest BLOB PRIMARY KEY NOT NULL, \
             authorization_issued_at BLOB NOT NULL, \
             event_json TEXT NOT NULL\
         );",
    )
    .execute(&pool)
    .await
    .unwrap();
    // The newest in-bound row (within the one-hour future skew) is neither the
    // first row nor the lowest or highest digest; a row two hours ahead is not.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for (digest, issued_at, event) in [
        (2u8, 0x0100u64, "middle"),
        (1, now + 1800, "newest"),
        (0, 0x00ff, "older"),
        (3, now + 7200, "future"),
    ] {
        sqlx::query("INSERT INTO holder_authorization_events VALUES (?, ?, ?)")
            .bind(vec![digest; 32])
            .bind(issued_at.to_be_bytes().to_vec())
            .bind(event)
            .execute(&pool)
            .await
            .unwrap();
    }

    sqlx::raw_sql(include_str!(
        "../migrations/0005_single_holder_authorization.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();

    let retained: Vec<String> = sqlx::query_scalar("SELECT event_json FROM holder_authorization")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(retained, vec!["newest"]);
    let legacy: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM holder_authorization_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(legacy, 4, "unadopted authorizations remain recoverable");
}

#[tokio::test]
async fn an_fman_from_before_readiness_upgrades_as_ready() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for migration in [
        include_str!("../migrations/0001_initial.sql"),
        include_str!("../migrations/0002_wallet_origin.sql"),
        include_str!("../migrations/0003_payout_destination_cap.sql"),
        include_str!("../migrations/0004_dkg_inputs.sql"),
        include_str!("../migrations/0005_single_holder_authorization.sql"),
        include_str!("../migrations/0006_seat_readiness.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }

    sqlx::raw_sql(include_str!("../migrations/0007_seat_readiness_open.sql"))
        .execute(&pool)
        .await
        .unwrap();

    // Ready, so the first failing run is a change of verdict that draws a
    // fresh epoch and refuses quotes issued before the upgrade.
    let ready: bool =
        sqlx::query_scalar("SELECT ready_for_new_seats FROM offer_state WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(ready);
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
    db.replace_holder_authorization_event(1, "event", 100)
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
