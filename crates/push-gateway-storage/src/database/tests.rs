use sqlx::{Executor, any::AnyPoolOptions};

use super::DatabaseBackend;

#[test]
fn database_backend_detects_supported_url_schemes() {
    assert_eq!(
        DatabaseBackend::from_url("sqlite://push-gateway.sqlite?mode=rwc").unwrap(),
        DatabaseBackend::Sqlite
    );
    assert_eq!(
        DatabaseBackend::from_url("postgres://localhost/push").unwrap(),
        DatabaseBackend::Postgres
    );
    assert_eq!(
        DatabaseBackend::from_url("postgresql://localhost/push").unwrap(),
        DatabaseBackend::Postgres
    );
}

#[tokio::test]
async fn populated_upgrade_backfills_owners_and_retained_idempotency() {
    sqlx::any::install_default_drivers();
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect SQLite");
    for migration in [
        include_str!("../../migrations/20260526000000_initial_push_gateway_schema.sql"),
        include_str!("../../migrations/20260527000000_opaque_hook_token_hash.sql"),
        include_str!(
            "../../migrations/20260528000000_registration_installation_ids_per_recipient.sql"
        ),
        include_str!("../../migrations/20260701000000_retention_purge_indexes.sql"),
        include_str!("../../migrations/20260702000000_hook_secret_and_idempotency_names.sql"),
    ] {
        pool.execute(migration)
            .await
            .expect("apply pre-upgrade schema");
    }

    sqlx::query(
        "INSERT INTO push_registrations (
             recipient_id, installation_id, fcm_token, created_at, updated_at, last_seen_at
         ) VALUES
             ('single', 'single-device', 'single-token', 1, 2, 2),
             ('ambiguous', 'device-a', 'token-a', 1, 2, 2),
             ('ambiguous', 'device-b', 'token-b', 1, 2, 2)",
    )
    .execute(&pool)
    .await
    .expect("seed registrations");
    sqlx::query(
        "INSERT INTO notification_hooks (
             hook_id, hook_secret_hash, recipient_id, created_at, expires_at
         ) VALUES
             ('retained-hook', 'retained-secret', 'single', 10, 1000),
             ('ambiguous-hook', 'ambiguous-secret', 'ambiguous', 10, 1000)",
    )
    .execute(&pool)
    .await
    .expect("seed hooks");
    sqlx::query(
        "INSERT INTO notification_events (
             event_id, hook_id, caller_idempotency_key, recipient_id,
             notification_json, target_count, created_at
         ) VALUES
             ('retained-event', 'retained-hook', 'retained-key', 'single', '{}', 1, 20),
             ('ambiguous-event', 'ambiguous-hook', 'ambiguous-key', 'ambiguous', '{}', 2, 20)",
    )
    .execute(&pool)
    .await
    .expect("seed accepted keyed events");
    sqlx::query(
        "INSERT INTO delivery_outbox (
             outbox_id, event_id, recipient_id, installation_id, fcm_token,
             notification_json, status, attempts, next_attempt_at, created_at, updated_at
         ) VALUES (
             'ambiguous-outbox', 'ambiguous-event', 'ambiguous', 'device-a',
             'token-a', '{}', 'pending', 0, 20, 20, 20
         )",
    )
    .execute(&pool)
    .await
    .expect("seed ambiguous delivery");

    for migration in [
        include_str!("../../migrations/20260805000000_installation_scoped_hooks.sql"),
        include_str!("../../migrations/20260805010000_registration_token_ownership.sql"),
        include_str!("../../migrations/20260806000000_hook_idempotency_tombstones.sql"),
    ] {
        pool.execute(migration)
            .await
            .expect("apply populated upgrade");
    }

    let owners: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT fcm_token, recipient_id, installation_id
         FROM push_registration_token_owners ORDER BY fcm_token",
    )
    .fetch_all(&pool)
    .await
    .expect("read owner backfill");
    assert_eq!(
        owners,
        vec![
            (
                "single-token".to_owned(),
                "single".to_owned(),
                "single-device".to_owned(),
            ),
            (
                "token-a".to_owned(),
                "ambiguous".to_owned(),
                "device-a".to_owned(),
            ),
            (
                "token-b".to_owned(),
                "ambiguous".to_owned(),
                "device-b".to_owned(),
            ),
        ]
    );
    let installation_id: String = sqlx::query_scalar(
        "SELECT installation_id FROM notification_hooks WHERE hook_id = 'retained-hook'",
    )
    .fetch_one(&pool)
    .await
    .expect("read narrowed hook");
    assert_eq!(installation_id, "single-device");
    let installation_not_null: i64 = sqlx::query_scalar(
        "SELECT \"notnull\" FROM pragma_table_info('notification_hooks')
         WHERE name = 'installation_id'",
    )
    .fetch_one(&pool)
    .await
    .expect("read installation nullability");
    assert_eq!(installation_not_null, 1);
    let ambiguous_hooks: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notification_hooks WHERE hook_id = 'ambiguous-hook'",
    )
    .fetch_one(&pool)
    .await
    .expect("count invalidated hooks");
    assert_eq!(ambiguous_hooks, 0);
    for (table, predicate) in [
        ("notification_events", "event_id = 'ambiguous-event'"),
        ("delivery_outbox", "outbox_id = 'ambiguous-outbox'"),
        ("hook_idempotency_tombstones", "hook_id = 'ambiguous-hook'"),
    ] {
        let count: i64 =
            sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE {predicate}"))
                .fetch_one(&pool)
                .await
                .expect("count invalidated legacy state");
        assert_eq!(count, 0, "{table} retained ambiguous legacy state");
    }
    let tombstone: (i64, i64) = sqlx::query_as(
        "SELECT target_count, retain_until FROM hook_idempotency_tombstones
         WHERE hook_id = 'retained-hook' AND caller_idempotency_key = 'retained-key'",
    )
    .fetch_one(&pool)
    .await
    .expect("read tombstone backfill");
    assert_eq!(tombstone, (1, 605_800));
}

#[tokio::test]
#[ignore = "requires PUSH_GATEWAY_POSTGRES_TEST_URL pointing at a disposable PostgreSQL database"]
async fn postgres_connect_runs_migrations_when_url_is_provided() {
    let database_url = std::env::var("PUSH_GATEWAY_POSTGRES_TEST_URL")
        .expect("PUSH_GATEWAY_POSTGRES_TEST_URL must be set");
    let database = crate::Database::connect(&database_url)
        .await
        .expect("connect and migrate PostgreSQL database");

    assert_eq!(database.backend(), DatabaseBackend::Postgres);
    assert!(database.is_ready().await);
    let statement_timeout: String = sqlx::query_scalar("SHOW statement_timeout")
        .fetch_one(database.pool())
        .await
        .expect("read PostgreSQL statement timeout");
    assert_eq!(statement_timeout, "5s");
}
