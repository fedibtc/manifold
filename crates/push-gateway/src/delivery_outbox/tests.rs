use sqlx::Executor;

use super::*;

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
    insert_dead_letter(&outbox, "outbox-a", "provider_unavailable", 10).await;
    insert_dead_letter(&outbox, "outbox-b", "provider_unavailable", 11).await;

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

async fn sqlite_outbox() -> (tempfile::TempDir, DeliveryOutboxRepository) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let database_url = format!(
        "sqlite://{}?mode=rwc",
        tempdir.path().join("push.sqlite").display()
    );
    let config = crate::PushGatewayConfig::new(None, database_url, None);
    let database = crate::Database::connect(&config).await.expect("database");
    let outbox = DeliveryOutboxRepository::new(database.pool().clone(), database.backend());
    outbox
        .pool
        .execute(
            "INSERT INTO notification_hooks (
                    hook_id, secret_hash, recipient_id, open_behavior, privacy, data_json,
                    created_at, rate_limit_window_seconds, rate_limit_max_requests
                 ) VALUES ('hook', 'secret-hash', 'recipient', 'open_app', 'display_text', '{}',
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
