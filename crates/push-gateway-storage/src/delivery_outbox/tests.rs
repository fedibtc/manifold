use std::collections::BTreeSet;

use sqlx::Executor;

use super::*;

#[test]
fn postgres_deadline_clock_floors_once_for_every_failure_field() {
    let cte = database_now_cte(DatabaseBackend::Postgres);
    assert_eq!(
        cte,
        "WITH database_now(epoch) AS MATERIALIZED \
         (SELECT FLOOR(EXTRACT(EPOCH FROM clock_timestamp()))::BIGINT)"
    );

    let query = mark_failed_query(cte);
    assert_eq!(query.matches("clock_timestamp()").count(), 1);
    assert_eq!(query.matches("SELECT epoch FROM database_now").count(), 3);
}

#[tokio::test]
async fn dead_letter_admin_methods_do_not_return_sensitive_columns() {
    let (_tempdir, outbox) = sqlite_outbox().await;
    insert_dead_letter(&outbox, "outbox-a", "provider_unavailable", 10).await;
    insert_dead_letter(&outbox, "outbox-b", "payload_invalid", 11).await;

    let rows = outbox.list_dead_letter_rows(10).await.expect("list rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].outbox_id, "outbox-a");
    assert_eq!(rows[0].last_error.as_deref(), Some("provider_unavailable"));
    let debug = format!("{rows:?}");
    assert!(!debug.contains("secret-fcm-token"));
    assert!(!debug.contains("Sensitive notification"));

    let reasons = outbox
        .dead_letter_reason_counts()
        .await
        .expect("reason counts");
    assert_eq!(reasons[0].count, 1);
    assert!(
        reasons
            .iter()
            .any(|reason| reason.reason == "payload_invalid")
    );
}

#[tokio::test]
async fn dead_letter_replay_and_delete_are_bounded_and_dry_run_safe() {
    let (_tempdir, outbox) = sqlite_outbox().await;
    let now = crate::time::unix_timestamp();
    insert_dead_letter(&outbox, "outbox-a", "provider_unavailable", now).await;
    insert_dead_letter(&outbox, "outbox-b", "provider_unavailable", now).await;

    let selector = OutboxDeadLetterSelector {
        outbox_ids: Vec::new(),
        limit: Some(1),
        reason: Some("provider_unavailable".to_owned()),
    };
    assert_eq!(
        outbox
            .replay_dead_letter_rows(&selector, true)
            .await
            .expect("dry-run replay"),
        1
    );
    assert_eq!(
        outbox
            .count_by_status("dead_letter")
            .await
            .expect("dead-letter count"),
        2
    );
    assert_eq!(
        outbox
            .replay_dead_letter_rows(&selector, false)
            .await
            .expect("replay"),
        1
    );
    assert_eq!(
        outbox
            .count_by_status("pending")
            .await
            .expect("pending count"),
        1
    );

    let delete_selector = OutboxDeadLetterSelector {
        outbox_ids: vec!["outbox-b".to_owned()],
        limit: None,
        reason: None,
    };
    assert_eq!(
        outbox
            .delete_dead_letter_rows(&delete_selector, false)
            .await
            .expect("delete"),
        1
    );
    assert_eq!(
        outbox
            .count_by_status("dead_letter")
            .await
            .expect("dead-letter count"),
        0
    );
}

#[tokio::test]
async fn statement_time_deadline_rejects_replay_without_restarting_the_resolution_deadline() {
    let (_tempdir, outbox) = sqlite_outbox().await;
    let outbox = outbox
        .with_database_now_cte_for_test("WITH database_now(epoch) AS MATERIALIZED (SELECT 301)");
    let expired_at = 1;
    insert_dead_letter(
        &outbox,
        "expired-outbox",
        "resolution_deadline_exceeded",
        expired_at,
    )
    .await;
    let selector = OutboxDeadLetterSelector {
        outbox_ids: vec!["expired-outbox".to_owned()],
        limit: None,
        reason: None,
    };

    let error = outbox
        .replay_dead_letter_rows(&selector, true)
        .await
        .expect_err("expired replay must be rejected");
    assert!(
        error
            .to_string()
            .contains("past its delivery resolution deadline")
    );
    assert_eq!(
        outbox
            .count_by_status("dead_letter")
            .await
            .expect("dead-letter count"),
        1
    );
}

#[tokio::test]
async fn dead_letter_admin_selectors_reject_unbounded_or_ambiguous_inputs() {
    let (_tempdir, outbox) = sqlite_outbox().await;
    insert_dead_letter(&outbox, "outbox-a", "provider_unavailable", 10).await;

    assert!(outbox.list_dead_letter_rows(0).await.is_err());
    assert!(
        outbox
            .list_dead_letter_rows(MAX_DEAD_LETTER_ADMIN_SELECTION + 1)
            .await
            .is_err()
    );
    assert!(
        outbox
            .select_dead_letter_rows(&OutboxDeadLetterSelector {
                outbox_ids: Vec::new(),
                limit: Some(-1),
                reason: None,
            })
            .await
            .is_err()
    );
    assert!(
        outbox
            .select_dead_letter_rows(&OutboxDeadLetterSelector {
                outbox_ids: vec!["outbox-a".to_owned(), "outbox-a".to_owned()],
                limit: None,
                reason: None,
            })
            .await
            .is_err()
    );
    assert!(
        outbox
            .select_dead_letter_rows(&OutboxDeadLetterSelector {
                outbox_ids: (0..=MAX_DEAD_LETTER_ADMIN_SELECTION)
                    .map(|idx| format!("outbox-{idx}"))
                    .collect(),
                limit: None,
                reason: None,
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn retention_purge_deletes_only_old_terminal_sensitive_data() {
    let (_tempdir, outbox) = sqlite_outbox().await;
    insert_dead_letter(&outbox, "old-terminal", "provider_unavailable", 10).await;
    insert_dead_letter(&outbox, "fresh-terminal", "provider_unavailable", 90).await;
    sqlx::query(
        "INSERT INTO notification_events (
                event_id, hook_id, recipient_id, notification_json, target_count, created_at
             ) VALUES (
                'pending-event', 'hook', 'recipient',
                '{\"title\":\"Sensitive pending\"}', 1, 1
             ), (
                'terminal-only-event', 'hook', 'recipient',
                '{\"title\":\"Sensitive terminal-only\"}', 2, 1
             ), (
                'orphan-event', 'hook', 'recipient',
                '{\"title\":\"Sensitive orphan\"}', 0, 1
             )",
    )
    .execute(&outbox.pool)
    .await
    .expect("insert events");
    sqlx::query(
        "INSERT INTO delivery_outbox (
                outbox_id, event_id, recipient_id, installation_id, fcm_token, platform,
                notification_json, status, attempts, next_attempt_at, created_at, updated_at
             ) VALUES (
                'pending-row', 'pending-event', 'recipient', 'installation-pending',
                'pending-token', 'android', '{\"title\":\"Sensitive pending\"}',
                'pending', 0, 1, 1, 1
             ), (
                'succeeded-row', 'terminal-only-event', 'recipient', 'installation-succeeded',
                'succeeded-token', 'android', '{\"title\":\"Sensitive succeeded\"}',
                'succeeded', 1, 10, 10, 10
             ), (
                'invalid-token-row', 'terminal-only-event', 'recipient', 'installation-invalid',
                'invalid-token', 'android', '{\"title\":\"Sensitive invalid\"}',
                'invalid_token', 1, 10, 10, 10
             )",
    )
    .execute(&outbox.pool)
    .await
    .expect("insert pending row");
    sqlx::query(
        "INSERT INTO push_registrations (
                recipient_id, installation_id, fcm_token, created_at, updated_at, last_seen_at,
                disabled_at, disabled_reason
             ) VALUES (
                'recipient', 'disabled-old', 'disabled-old-token', 1, 1, 1, 10, 'invalid_token'
             ), (
                'recipient', 'disabled-fresh', 'disabled-fresh-token', 1, 1, 1, 90, 'invalid_token'
             ), (
                'recipient', 'active', 'active-token', 1, 1, 1, NULL, NULL
             )",
    )
    .execute(&outbox.pool)
    .await
    .expect("insert registrations");
    sqlx::query(
        "INSERT INTO hook_idempotency_tombstones (
                hook_id, caller_idempotency_key, target_count, accepted_at, retain_until
             ) VALUES (
                'hook', 'expired-key', 1, 1, 10
             ), (
                'hook', 'retained-key', 1, 1, 90
             )",
    )
    .execute(&outbox.pool)
    .await
    .expect("insert idempotency tombstones");

    let counts = outbox
        .purge_retained_sensitive_data(50, 50)
        .await
        .expect("purge retention");
    assert_eq!(
        counts,
        RetentionPurgeCounts {
            delivery_outbox_rows: 3,
            disabled_registration_rows: 1,
            notification_event_rows: 2,
            idempotency_tombstone_rows: 1,
        }
    );
    assert_eq!(
        count_rows(&outbox, "delivery_outbox", "outbox_id = 'old-terminal'").await,
        0
    );
    assert_eq!(
        count_rows(&outbox, "delivery_outbox", "outbox_id = 'fresh-terminal'").await,
        1
    );
    assert_eq!(
        count_rows(&outbox, "delivery_outbox", "outbox_id = 'pending-row'").await,
        1
    );
    assert_eq!(
        count_rows(&outbox, "notification_events", "event_id = 'event'").await,
        1
    );
    assert_eq!(
        count_rows(&outbox, "notification_events", "event_id = 'pending-event'").await,
        1
    );
    assert_eq!(
        count_rows(
            &outbox,
            "notification_events",
            "event_id = 'terminal-only-event'"
        )
        .await,
        0
    );
    assert_eq!(
        count_rows(&outbox, "notification_events", "event_id = 'orphan-event'").await,
        0
    );
    assert_eq!(
        count_rows(
            &outbox,
            "push_registrations",
            "installation_id = 'disabled-old'"
        )
        .await,
        0
    );
    assert_eq!(
        count_rows(
            &outbox,
            "push_registrations",
            "installation_id = 'disabled-fresh'"
        )
        .await,
        1
    );
    assert_eq!(
        count_rows(&outbox, "push_registrations", "installation_id = 'active'").await,
        1
    );
    assert_eq!(
        count_rows(
            &outbox,
            "hook_idempotency_tombstones",
            "caller_idempotency_key = 'expired-key'"
        )
        .await,
        0
    );
    assert_eq!(
        count_rows(
            &outbox,
            "hook_idempotency_tombstones",
            "caller_idempotency_key = 'retained-key'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn durable_state_inventory_matches_persisted_tables() {
    let (_tempdir, outbox) = sqlite_outbox().await;
    let tables: BTreeSet<String> = sqlx::query_scalar(
        "SELECT name
         FROM sqlite_master
         WHERE type = 'table'
           AND name NOT LIKE 'sqlite_%'
           AND name != '_sqlx_migrations'",
    )
    .fetch_all(&outbox.pool)
    .await
    .expect("list durable tables")
    .into_iter()
    .collect();

    assert_eq!(
        tables,
        BTreeSet::from([
            "delivery_outbox".to_owned(),
            "guardian_telemetry_fmans".to_owned(),
            "hook_idempotency_tombstones".to_owned(),
            "notification_events".to_owned(),
            "notification_hooks".to_owned(),
            "push_gateway_admission_locks".to_owned(),
            "push_registration_token_owners".to_owned(),
            "push_registrations".to_owned(),
        ])
    );
}

#[tokio::test]
async fn resolution_deadline_terminally_expires_every_active_outbox_state() {
    let (_tempdir, outbox) = sqlite_outbox().await;
    let now = crate::time::unix_timestamp();
    let overdue_created_at = now.saturating_sub(DELIVERY_RESOLUTION_DEADLINE_SECONDS);
    sqlx::query(
        "INSERT INTO delivery_outbox (
                outbox_id, event_id, recipient_id, installation_id, fcm_token, platform,
                notification_json, status, attempts, next_attempt_at, claim_id, created_at, updated_at
             ) VALUES
                ('overdue-pending', 'event', 'recipient', 'pending-installation', 'pending-token',
                 'android', '{}', 'pending', 0, 1, NULL, $1, $1),
                ('overdue-retrying', 'event', 'recipient', 'retrying-installation', 'retrying-token',
                 'android', '{}', 'retrying', 2, 1, NULL, $1, $1),
                ('overdue-claimed', 'event', 'recipient', 'claimed-installation', 'claimed-token',
                 'android', '{}', 'in_progress', 3, 1, 'old-claim', $1, $1),
                ('fresh-pending', 'event', 'recipient', 'fresh-installation', 'fresh-token',
                 'android', '{}', 'pending', 0, $2, NULL, $2, $2)",
    )
    .bind(overdue_created_at)
    .bind(now)
    .execute(&outbox.pool)
    .await
    .expect("insert active outbox rows");

    assert_eq!(
        outbox
            .expire_delivery_resolution_deadlines()
            .await
            .expect("expire deadlines"),
        3
    );

    let rows: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT outbox_id, status, last_error, attempts
         FROM delivery_outbox
         WHERE outbox_id LIKE 'overdue-%'
         ORDER BY outbox_id",
    )
    .fetch_all(&outbox.pool)
    .await
    .expect("read expired rows");
    assert_eq!(
        rows,
        vec![
            (
                "overdue-claimed".to_owned(),
                "dead_letter".to_owned(),
                Some("resolution_deadline_exceeded".to_owned()),
                3,
            ),
            (
                "overdue-pending".to_owned(),
                "dead_letter".to_owned(),
                Some("resolution_deadline_exceeded".to_owned()),
                0,
            ),
            (
                "overdue-retrying".to_owned(),
                "dead_letter".to_owned(),
                Some("resolution_deadline_exceeded".to_owned()),
                2,
            ),
        ]
    );
    assert_eq!(
        count_rows(
            &outbox,
            "delivery_outbox",
            "outbox_id = 'fresh-pending' AND status = 'pending'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn statement_time_deadline_overrides_provider_failure_retry() {
    let (_tempdir, outbox) = sqlite_outbox().await;
    let outbox = outbox
        .with_database_now_cte_for_test("WITH database_now(epoch) AS MATERIALIZED (SELECT 301)");
    let overdue_created_at = 1;
    sqlx::query(
        "INSERT INTO delivery_outbox (
                outbox_id, event_id, recipient_id, installation_id, fcm_token, platform,
                notification_json, status, attempts, next_attempt_at, created_at, updated_at
             ) VALUES (
                'deadline-claimed', 'event', 'recipient', 'deadline-installation', 'deadline-token',
                'android',
                '{\"recipient_id\":\"recipient\",\"notification_id\":\"notification\",\"kind\":\"kind\",\"title\":null,\"body\":null,\"data\":{}}',
                'pending', 0, 0, $1, $1
             )",
    )
    .bind(overdue_created_at)
    .execute(&outbox.pool)
    .await
    .expect("insert overdue outbox row");
    let ClaimDueOutcome::Claimed(claimed) = outbox.claim_due().await.expect("claim overdue row")
    else {
        panic!("expected claimed delivery");
    };

    assert_eq!(
        outbox
            .mark_failed(
                &claimed.outbox_id,
                &claimed.claim_id,
                &DeliveryOutboxFailure::transient("provider_timeout"),
            )
            .await
            .expect("mark failure"),
        MarkFailedOutcome::DeadLettered
    );
    let row: (String, Option<String>, i64) = sqlx::query_as(
        "SELECT status, last_error, attempts
         FROM delivery_outbox
         WHERE outbox_id = 'deadline-claimed'",
    )
    .fetch_one(&outbox.pool)
    .await
    .expect("read deadline row");
    assert_eq!(
        row,
        (
            "dead_letter".to_owned(),
            Some("resolution_deadline_exceeded".to_owned()),
            1,
        )
    );
}

#[tokio::test]
async fn statement_time_deadline_prevents_provider_success_from_bypassing_terminal_outcome() {
    let (_tempdir, outbox) = sqlite_outbox().await;
    let outbox = outbox
        .with_database_now_cte_for_test("WITH database_now(epoch) AS MATERIALIZED (SELECT 301)");
    let overdue_created_at = 1;
    sqlx::query(
        "INSERT INTO delivery_outbox (
                outbox_id, event_id, recipient_id, installation_id, fcm_token, platform,
                notification_json, status, attempts, next_attempt_at, created_at, updated_at
             ) VALUES (
                'deadline-success', 'event', 'recipient', 'deadline-success-installation',
                'deadline-success-token', 'android',
                '{\"recipient_id\":\"recipient\",\"notification_id\":\"notification\",\"kind\":\"kind\",\"title\":null,\"body\":null,\"data\":{}}',
                'pending', 0, 0, $1, $1
             )",
    )
    .bind(overdue_created_at)
    .execute(&outbox.pool)
    .await
    .expect("insert overdue outbox row");
    let ClaimDueOutcome::Claimed(claimed) = outbox.claim_due().await.expect("claim overdue row")
    else {
        panic!("expected claimed delivery");
    };

    assert!(
        !outbox
            .mark_succeeded(&claimed.outbox_id, &claimed.claim_id)
            .await
            .expect("mark success")
    );
    assert_eq!(
        outbox
            .expire_delivery_resolution_deadlines()
            .await
            .expect("expire deadline"),
        1
    );
    let row: (String, Option<String>) = sqlx::query_as(
        "SELECT status, last_error
         FROM delivery_outbox
         WHERE outbox_id = 'deadline-success'",
    )
    .fetch_one(&outbox.pool)
    .await
    .expect("read terminal row");
    assert_eq!(
        row,
        (
            "dead_letter".to_owned(),
            Some("resolution_deadline_exceeded".to_owned()),
        )
    );
}

async fn count_rows(outbox: &DeliveryOutboxRepository, table: &str, predicate: &str) -> i64 {
    let query = format!("SELECT COUNT(*) FROM {table} WHERE {predicate}");
    sqlx::query_scalar(&query)
        .fetch_one(&outbox.pool)
        .await
        .expect("count rows")
}

async fn sqlite_outbox() -> (tempfile::TempDir, DeliveryOutboxRepository) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let database_url = format!(
        "sqlite://{}?mode=rwc",
        tempdir.path().join("push.sqlite").display()
    );
    let database = crate::Database::connect(&database_url)
        .await
        .expect("database");
    let outbox = DeliveryOutboxRepository::new(database.pool().clone(), database.backend());
    outbox
        .pool
        .execute(
            "INSERT INTO notification_hooks (
                    hook_id, hook_secret_hash, recipient_id, open_behavior, privacy, data_json,
                    created_at, rate_limit_window_seconds, rate_limit_max_requests
                 ) VALUES ('hook', 'token-hash', 'recipient', 'open_app', 'display_text', '{}',
                    1, 3600, 2)",
        )
        .await
        .expect("insert hook");
    outbox
        .pool
        .execute(
            "INSERT INTO notification_events (
                    event_id, hook_id, recipient_id, notification_json, target_count, created_at
                 ) VALUES (
                    'event', 'hook', 'recipient',
                    '{\"title\":\"Sensitive notification\",\"body\":\"secret body\"}', 2, 1
                 )",
        )
        .await
        .expect("insert event");
    (tempdir, outbox)
}

async fn insert_dead_letter(
    outbox: &DeliveryOutboxRepository,
    outbox_id: &str,
    reason: &str,
    now: i64,
) {
    sqlx::query(
        "INSERT INTO delivery_outbox (
                outbox_id, event_id, recipient_id, installation_id, fcm_token, platform,
                notification_json, status, attempts, next_attempt_at, last_attempt_at,
                last_error, created_at, updated_at
             ) VALUES ($1, 'event', 'recipient', $2, 'secret-fcm-token', 'ios',
                '{\"title\":\"Sensitive notification\",\"body\":\"secret body\"}', 'dead_letter',
                5, $3, $3, $4, $3, $3)",
    )
    .bind(outbox_id)
    .bind(format!("installation-{outbox_id}"))
    .bind(now)
    .bind(reason)
    .execute(&outbox.pool)
    .await
    .expect("insert dead-letter row");
}
