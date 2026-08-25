use std::{
    collections::VecDeque,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use base64::{Engine, engine::general_purpose};
use fedi_decentralized_push_gateway::{
    AppId, AppState, ClaimDueOutcome, DELIVERY_RESOLUTION_DEADLINE_SECONDS, Database,
    DeliveryOutboxFailure, DeliveryOutboxFailureKind, DeliveryOutboxRepository, FakePushProvider,
    PushGatewayConfig, PushProvider, PushProviderError, operator_app, public_app,
};
use fedi_decentralized_push_gateway_storage::DEFAULT_DATABASE_WRITE_REQUEST_ADMISSION;
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tower::ServiceExt;

#[tokio::test]
async fn invocation_durably_enqueues_before_delivery() {
    let harness = Harness::new(Arc::new(FakePushProvider::default())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;

    let invoke = harness
        .json("POST", &url, json!({"idempotency_key":"evt-1"}))
        .await;
    assert_eq!(invoke.status, StatusCode::OK);
    assert_eq!(invoke.body["delivery_attempts"], 1);
    assert_eq!(outbox_count(&harness, "pending").await, 1);
}

#[tokio::test]
async fn metrics_reports_oldest_outbox_ages() {
    let harness = Harness::new(Arc::new(FakePushProvider::default())).await;
    register(&harness, "device-1", "token-1").await;
    register(&harness, "device-2", "token-2").await;
    let first_url = create_hook_for(&harness, "device-1").await;
    let second_url = create_hook_for(&harness, "device-2").await;
    assert_eq!(
        harness.json("POST", &first_url, json!({})).await.status,
        StatusCode::OK
    );
    assert_eq!(
        harness.json("POST", &second_url, json!({})).await.status,
        StatusCode::OK
    );
    let now = unix_timestamp_for_test();
    sqlx::query(
        "UPDATE delivery_outbox
         SET next_attempt_at = $1, created_at = $2, updated_at = $2
         WHERE installation_id = 'device-1'",
    )
    .bind(now - 31)
    .bind(now - 67)
    .execute(harness.state.database().pool())
    .await
    .expect("age pending row");
    sqlx::query(
        "UPDATE delivery_outbox
         SET status = 'retrying', next_attempt_at = $1, created_at = $2, updated_at = $3
         WHERE installation_id = 'device-2'",
    )
    .bind(now - 29)
    .bind(now - 83)
    .bind(now - 43)
    .execute(harness.state.database().pool())
    .await
    .expect("age retrying row");

    let metrics = harness.text("GET", "/metrics").await;
    assert_eq!(metrics.status, StatusCode::OK);
    assert_metric_between(
        &metrics.text,
        "push_gateway_outbox_oldest_due_age_seconds",
        31,
        40,
    );
    assert_metric_between(
        &metrics.text,
        "push_gateway_outbox_oldest_pending_age_seconds",
        67,
        76,
    );
    assert_metric_between(
        &metrics.text,
        "push_gateway_outbox_retrying_oldest_age_seconds",
        43,
        52,
    );
}

#[tokio::test]
async fn worker_delivers_pending_row_after_startup() {
    let provider = FakePushProvider::default();
    let harness = Harness::new(Arc::new(provider.clone())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;
    assert_eq!(
        harness.json("POST", &url, json!({})).await.status,
        StatusCode::OK
    );

    let worker = harness.state.start_delivery_worker();
    wait_for_status(&harness, "succeeded", 1).await;
    worker.shutdown().await;
    assert_eq!(provider.deliveries().len(), 1);
}

#[tokio::test]
async fn worker_dead_letters_overdue_delivery_without_calling_provider() {
    let provider = FakePushProvider::default();
    let harness = Harness::new(Arc::new(provider.clone())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;
    assert_eq!(
        harness.json("POST", &url, json!({})).await.status,
        StatusCode::OK
    );
    sqlx::query("UPDATE delivery_outbox SET created_at = $1")
        .bind(unix_timestamp_for_test().saturating_sub(DELIVERY_RESOLUTION_DEADLINE_SECONDS))
        .execute(harness.state.database().pool())
        .await
        .expect("expire accepted delivery");

    let worker = harness.state.start_delivery_worker();
    wait_for_status(&harness, "dead_letter", 1).await;
    worker.shutdown().await;

    assert!(provider.deliveries().is_empty());
    let reason: Option<String> =
        sqlx::query_scalar("SELECT last_error FROM delivery_outbox LIMIT 1")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("read terminal reason");
    assert_eq!(reason.as_deref(), Some("resolution_deadline_exceeded"));
}

#[tokio::test]
async fn worker_database_writes_wait_for_request_write_lock() {
    let provider = FakePushProvider::default();
    let harness = Harness::new(Arc::new(provider.clone())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;
    assert_eq!(
        harness.json("POST", &url, json!({})).await.status,
        StatusCode::OK
    );

    let write_lock = harness.state.database().write_lock();
    let guard = write_lock.acquire_worker().await;
    let acquisition = write_lock.observe_next_acquisition().await;
    let worker = harness.state.start_delivery_worker();
    acquisition
        .await
        .expect("worker reached write-lock boundary");
    assert_eq!(outbox_count(&harness, "pending").await, 1);
    assert!(provider.deliveries().is_empty());

    drop(guard);
    wait_for_status(&harness, "succeeded", 1).await;
    worker.shutdown().await;
    assert_eq!(provider.deliveries().len(), 1);
}

#[tokio::test]
async fn hook_acceptance_waits_for_write_lock() {
    let provider = FakePushProvider::default();
    let harness = Harness::new(Arc::new(provider.clone())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;

    let write_lock = harness.state.database().write_lock();
    let guard = write_lock.acquire_worker().await;
    let acquisition = write_lock.observe_next_acquisition().await;
    let state = harness.state.clone();
    let response = tokio::spawn(async move {
        public_app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(url)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("hook invocation request"),
            )
            .await
            .expect("hook invocation response")
            .status()
    });
    acquisition
        .await
        .expect("hook acceptance reached write-lock boundary");
    assert!(
        !response.is_finished(),
        "hook acceptance completed while the database-write lock was held"
    );

    drop(guard);
    assert_eq!(
        response.await.expect("hook invocation task"),
        StatusCode::OK
    );

    // This test covers request-side lock waiting. Start the worker after acceptance so its
    // priority over queued public writes cannot affect the asserted request ordering.
    let worker = harness.state.start_delivery_worker();
    wait_for_status(&harness, "succeeded", 1).await;
    worker.shutdown().await;
    assert_eq!(provider.deliveries().len(), 1);
}

#[tokio::test]
async fn management_database_writes_wait_for_shared_write_lock() {
    let harness = Harness::new(Arc::new(FakePushProvider::default())).await;
    register(&harness, "device-1", "token-1").await;
    assert_management_request_waits_for_write_lock(
        &harness,
        harness.state.clone(),
        management_json_request(&harness, "POST", "/v1/hooks", json!({ "label": "locked" })),
    )
    .await;
    assert_management_request_waits_for_write_lock(
        &harness,
        harness.state.clone(),
        management_json_request(
            &harness,
            "POST",
            "/registrations",
            json!({
                "installation_id": "device-upsert",
                "fcm_token": "token-upsert",
                "platform": "android",
            }),
        ),
    )
    .await;

    register(&harness, "device-1", "token-1").await;
    assert_management_request_waits_for_write_lock(
        &harness,
        harness.state.clone(),
        management_empty_request(&harness, "DELETE", "/registrations/device-1"),
    )
    .await;

    register(&harness, "device-2", "token-2").await;
    assert_management_request_waits_for_write_lock(
        &harness,
        harness.state.clone(),
        management_empty_request(
            &harness,
            "POST",
            "/registrations/device-2/disable?reason=invalid_token",
        ),
    )
    .await;

    register(&harness, "device-1", "token-1").await;
    let hook_id = create_hook(&harness)
        .await
        .rsplit('/')
        .nth(1)
        .expect("hook id in invocation URL")
        .to_owned();
    assert_management_request_waits_for_write_lock(
        &harness,
        harness.state.clone(),
        management_empty_request(&harness, "DELETE", &format!("/v1/hooks/{hook_id}")),
    )
    .await;
}

#[tokio::test]
async fn app_states_from_one_database_share_the_write_coordinator() {
    let harness = Harness::new(Arc::new(FakePushProvider::default())).await;
    let second_state = AppState::with_push_provider(
        harness.state.config().clone(),
        harness.state.database().clone(),
        Arc::new(FakePushProvider::default()),
    );

    assert_management_request_waits_for_write_lock(
        &harness,
        second_state,
        management_json_request(
            &harness,
            "POST",
            "/registrations",
            json!({
                "installation_id": "second-state-device",
                "fcm_token": "second-state-token",
                "platform": "android",
            }),
        ),
    )
    .await;
}

#[tokio::test]
async fn database_write_admission_returns_sanitized_503_at_default_capacity() {
    let harness = Harness::new(Arc::new(FakePushProvider::default())).await;
    register(&harness, "device-1", "token-1").await;
    let write_lock = harness.state.database().write_lock();
    let guard = write_lock.acquire_worker().await;
    let mut admitted = Vec::with_capacity(DEFAULT_DATABASE_WRITE_REQUEST_ADMISSION.get());

    for request_number in 0..DEFAULT_DATABASE_WRITE_REQUEST_ADMISSION.get() {
        let acquisition = write_lock.observe_next_acquisition().await;
        let state = harness.state.clone();
        let request = management_json_request(
            &harness,
            "POST",
            "/v1/hooks",
            json!({ "label": format!("admitted-{request_number}") }),
        );
        admitted.push(tokio::spawn(async move {
            public_app(state)
                .oneshot(request)
                .await
                .expect("admitted management response")
                .status()
        }));
        acquisition
            .await
            .expect("admitted request reached the request queue boundary");
    }

    let overflow = public_app(harness.state.clone())
        .oneshot(management_json_request(
            &harness,
            "POST",
            "/v1/hooks",
            json!({ "label": "overflow" }),
        ))
        .await
        .expect("overflow management response");
    assert_eq!(overflow.status(), StatusCode::SERVICE_UNAVAILABLE);
    let overflow_body = to_bytes(overflow.into_body(), 1024 * 1024)
        .await
        .expect("overflow response body");
    assert_eq!(
        serde_json::from_slice::<Value>(&overflow_body).expect("overflow JSON")["error"]["code"],
        "database_write_queue_full"
    );

    drop(guard);
    for request in admitted {
        assert_eq!(
            request.await.expect("admitted management task"),
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn idle_worker_does_not_continuously_claim_when_nothing_is_due() {
    let provider = FakePushProvider::default();
    let harness = Harness::new(Arc::new(provider.clone())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;

    let worker = harness.state.start_delivery_worker();
    tokio::time::sleep(Duration::from_millis(550)).await;
    let idle_claim_queries = harness
        .state
        .observability()
        .snapshot()
        .outbox_claim_queries_total;
    assert!(
        idle_claim_queries <= 2,
        "idle worker should not busy-poll claim_due, saw {idle_claim_queries} claim queries"
    );

    assert_eq!(
        harness.json("POST", &url, json!({})).await.status,
        StatusCode::OK
    );
    wait_for_status(&harness, "succeeded", 1).await;
    worker.shutdown().await;
    assert_eq!(provider.deliveries().len(), 1);
}

#[tokio::test]
async fn duplicate_idempotency_key_is_idempotent_for_outbox_rows() {
    let provider = FakePushProvider::default();
    let harness = Harness::new(Arc::new(provider.clone())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;

    assert_eq!(
        harness
            .json("POST", &url, json!({"idempotency_key":"same"}))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        harness
            .json("POST", &url, json!({"idempotency_key":"same"}))
            .await
            .status,
        StatusCode::OK
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delivery_outbox")
        .fetch_one(harness.state.database().pool())
        .await
        .expect("count rows");
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn duplicate_idempotency_key_replays_after_max_use_is_exhausted() {
    let provider = FakePushProvider::default();
    let harness = Harness::new(Arc::new(provider)).await;
    register(&harness, "device-1", "token-1").await;
    let create = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({ "policy": { "max_uses": 1 }
            }),
        )
        .await;
    assert_eq!(create.status, StatusCode::OK);
    let url = create.body["invocation_url"].as_str().expect("url");

    assert_eq!(
        harness
            .json("POST", url, json!({"idempotency_key":"same"}))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        harness
            .json("POST", url, json!({"idempotency_key":"same"}))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        harness
            .json("POST", url, json!({"idempotency_key":"other"}))
            .await
            .status,
        StatusCode::TOO_MANY_REQUESTS
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delivery_outbox")
        .fetch_one(harness.state.database().pool())
        .await
        .expect("count rows");
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn fcm_reserved_data_key_is_rejected_before_enqueue() {
    let harness = Harness::new(Arc::new(FakePushProvider::default())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;

    let response = harness
        .json("POST", &url, json!({"data": {"google.foo": "blocked"}}))
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.body["error"]["code"], "data_key_reserved");
    assert_eq!(
        response.body["error"]["message"],
        "data payload uses a reserved key"
    );
    assert_eq!(outbox_count(&harness, "pending").await, 0);
}

#[tokio::test]
async fn final_fcm_data_size_is_rejected_before_enqueue() {
    let harness = Harness::new(Arc::new(FakePushProvider::default())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;

    let response = harness
        .json("POST", &url, json!({"data": {"fill": "x".repeat(4050)}}))
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.body["error"]["code"], "data_too_large");
    assert_eq!(outbox_count(&harness, "pending").await, 0);
}

#[tokio::test]
async fn portable_constraints_reject_invalid_outbox_values() {
    let harness = Harness::new(Arc::new(FakePushProvider::default())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;
    assert_eq!(
        harness
            .json("POST", &url, json!({"idempotency_key":"evt-1"}))
            .await
            .status,
        StatusCode::OK
    );

    assert_constraint_error(
        sqlx::query("UPDATE delivery_outbox SET status = 'mystery'")
            .execute(harness.state.database().pool())
            .await,
    );
    assert_constraint_error(
        sqlx::query("UPDATE delivery_outbox SET attempts = -1")
            .execute(harness.state.database().pool())
            .await,
    );
    assert_constraint_error(
        sqlx::query("UPDATE delivery_outbox SET next_attempt_at = -1")
            .execute(harness.state.database().pool())
            .await,
    );
    assert_constraint_error(
        sqlx::query("UPDATE delivery_outbox SET last_attempt_at = -1")
            .execute(harness.state.database().pool())
            .await,
    );
    assert_constraint_error(
        sqlx::query("UPDATE push_registrations SET created_at = -1")
            .execute(harness.state.database().pool())
            .await,
    );
    assert_constraint_error(
        sqlx::query("UPDATE push_registrations SET disabled_at = -1")
            .execute(harness.state.database().pool())
            .await,
    );
    assert_constraint_error(
        sqlx::query("UPDATE notification_events SET target_count = -1")
            .execute(harness.state.database().pool())
            .await,
    );
    assert_constraint_error(
        sqlx::query("UPDATE notification_events SET created_at = -1")
            .execute(harness.state.database().pool())
            .await,
    );
    assert_constraint_error(
        sqlx::query("UPDATE notification_hooks SET use_count = -1")
            .execute(harness.state.database().pool())
            .await,
    );
    assert_constraint_error(
        sqlx::query("UPDATE notification_hooks SET rate_limit_count = -1")
            .execute(harness.state.database().pool())
            .await,
    );
    assert_constraint_error(
        sqlx::query("UPDATE notification_hooks SET rate_limit_window_seconds = 0")
            .execute(harness.state.database().pool())
            .await,
    );
    assert_constraint_error(
        sqlx::query("UPDATE notification_hooks SET rate_limit_max_requests = 0")
            .execute(harness.state.database().pool())
            .await,
    );
    assert_constraint_error(
        sqlx::query("UPDATE notification_hooks SET max_uses = 0")
            .execute(harness.state.database().pool())
            .await,
    );

    let (event_id, recipient_id, installation_id, fcm_token, notification_json): (
        String,
        String,
        String,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT event_id, recipient_id, installation_id, fcm_token, notification_json
         FROM delivery_outbox",
    )
    .fetch_one(harness.state.database().pool())
    .await
    .expect("existing outbox row");
    assert_constraint_error(
        sqlx::query(
            "INSERT INTO delivery_outbox (
                 outbox_id, event_id, recipient_id, installation_id, fcm_token,
                 notification_json, status, attempts, next_attempt_at, created_at, updated_at
             ) VALUES ('duplicate-outbox', $1, $2, $3, $4, $5, 'pending', 0, 0, 0, 0)",
        )
        .bind(event_id)
        .bind(recipient_id)
        .bind(installation_id)
        .bind(fcm_token)
        .bind(notification_json)
        .execute(harness.state.database().pool())
        .await,
    );
}

#[tokio::test]
async fn corrupted_hook_data_json_returns_sanitized_internal_error() {
    let harness = Harness::new(Arc::new(FakePushProvider::default())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;
    sqlx::query("UPDATE notification_hooks SET data_json = '{not-json'")
        .execute(harness.state.database().pool())
        .await
        .expect("corrupt hook data");

    let response = harness.json("POST", &url, json!({})).await;

    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.body["error"]["code"], "internal_error");
    assert_eq!(response.body["error"]["message"], "internal server error");
    assert_eq!(outbox_count(&harness, "pending").await, 0);
}

#[tokio::test]
async fn corrupted_outbox_notification_is_dead_lettered_without_fallback_delivery() {
    let provider = FakePushProvider::default();
    let harness = Harness::new(Arc::new(provider.clone())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;
    assert_eq!(
        harness.json("POST", &url, json!({})).await.status,
        StatusCode::OK
    );
    sqlx::query("UPDATE delivery_outbox SET notification_json = '{not-json'")
        .execute(harness.state.database().pool())
        .await
        .expect("corrupt outbox notification");

    let worker = harness.state.start_delivery_worker();
    wait_for_status(&harness, "dead_letter", 1).await;
    worker.shutdown().await;

    assert!(provider.deliveries().is_empty());
    assert_eq!(outbox_count(&harness, "pending").await, 0);
    let metrics = harness.text("GET", "/metrics").await;
    assert_eq!(metrics.status, StatusCode::OK);
    assert!(
        metrics
            .text
            .contains("push_gateway_outbox_dead_letter_rows 1")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_outbox_dead_letter_total 1")
    );
    let last_error: String = sqlx::query_scalar("SELECT last_error FROM delivery_outbox")
        .fetch_one(harness.state.database().pool())
        .await
        .expect("last error");
    assert_eq!(last_error, "notification_json_invalid");
    assert_eq!(
        harness
            .state
            .observability()
            .snapshot()
            .outbox_delivery_failure_total,
        1
    );
}

#[tokio::test]
async fn invalid_token_disables_registration_and_stops_retrying() {
    let provider =
        ScriptedProvider::new(vec![Err(PushProviderError::invalid_token("invalid_token"))]);
    let harness = Harness::new(Arc::new(provider)).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;
    assert_eq!(
        harness.json("POST", &url, json!({})).await.status,
        StatusCode::OK
    );

    let worker = harness.state.start_delivery_worker();
    wait_for_status(&harness, "invalid_token", 1).await;
    worker.shutdown().await;
    assert_eq!(outbox_count(&harness, "retrying").await, 0);
    let disabled_at: Option<i64> = sqlx::query_scalar(
        "SELECT disabled_at FROM push_registrations WHERE installation_id = 'device-1'",
    )
    .fetch_one(harness.state.database().pool())
    .await
    .expect("disabled_at");
    assert!(disabled_at.is_some());
}

#[test]
fn outbox_failure_inputs_are_non_token_failures() {
    assert_eq!(
        DeliveryOutboxFailure::permanent_payload("invalid_payload"),
        DeliveryOutboxFailure::new(
            "invalid_payload",
            DeliveryOutboxFailureKind::PermanentPayload
        )
    );
    assert_eq!(
        DeliveryOutboxFailure::transient("provider_unavailable"),
        DeliveryOutboxFailure::new("provider_unavailable", DeliveryOutboxFailureKind::Transient)
    );
}

#[tokio::test]
async fn transient_failure_retries_and_then_succeeds() {
    let provider = ScriptedProvider::new(vec![
        Err(PushProviderError::unavailable("provider_unavailable")),
        Ok(()),
    ]);
    let harness = Harness::new(Arc::new(provider)).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;
    assert_eq!(
        harness.json("POST", &url, json!({})).await.status,
        StatusCode::OK
    );

    let worker = harness.state.start_delivery_worker();
    harness.state.notify_delivery_worker();
    wait_for_status(&harness, "retrying", 1).await;
    sqlx::query("UPDATE delivery_outbox SET next_attempt_at = 0 WHERE status = 'retrying'")
        .execute(harness.state.database().pool())
        .await
        .expect("force due");
    harness.state.notify_delivery_worker();
    wait_for_status(&harness, "succeeded", 1).await;
    worker.shutdown().await;
}

#[tokio::test]
async fn retrying_row_waits_for_next_attempt_deadline_without_notify() {
    let provider = ScriptedProvider::new(vec![
        Err(PushProviderError::unavailable("provider_unavailable")),
        Ok(()),
    ]);
    let harness = Harness::new(Arc::new(provider)).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;
    assert_eq!(
        harness.json("POST", &url, json!({})).await.status,
        StatusCode::OK
    );

    let worker = harness.state.start_delivery_worker();
    harness.state.notify_delivery_worker();
    wait_for_status(&harness, "retrying", 1).await;
    let future_deadline = unix_timestamp_for_test() + 2;
    sqlx::query("UPDATE delivery_outbox SET next_attempt_at = $1 WHERE status = 'retrying'")
        .bind(future_deadline)
        .execute(harness.state.database().pool())
        .await
        .expect("move retry deadline into the future");

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(outbox_count(&harness, "succeeded").await, 0);
    wait_for_status(&harness, "succeeded", 1).await;
    worker.shutdown().await;
}

#[tokio::test]
async fn transient_retry_exhaustion_dead_letters() {
    let provider = ScriptedProvider::new(vec![
        Err(PushProviderError::unavailable(
            "provider_unavailable"
        ));
        8
    ]);
    let harness = Harness::new(Arc::new(provider)).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;
    assert_eq!(
        harness.json("POST", &url, json!({})).await.status,
        StatusCode::OK
    );

    let worker = harness.state.start_delivery_worker();
    for expected_attempts in 1..5 {
        wait_for_attempt_status(&harness, expected_attempts, "retrying").await;
        sqlx::query("UPDATE delivery_outbox SET next_attempt_at = 0 WHERE status = 'retrying'")
            .execute(harness.state.database().pool())
            .await
            .expect("force retry due");
        harness.state.notify_delivery_worker();
    }
    wait_for_attempt_status(&harness, 5, "dead_letter").await;
    worker.shutdown().await;
    assert_eq!(outbox_count(&harness, "dead_letter").await, 1);
    let metrics = harness.text("GET", "/metrics").await;
    assert_eq!(metrics.status, StatusCode::OK);
    assert!(
        metrics
            .text
            .contains("push_gateway_provider_outcomes_total{reason_class=\"transient\"} 5")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_outbox_dead_letter_rows 1")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_outbox_dead_letter_total 1")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_outbox_dead_letter_retained_total 1")
    );
}

#[tokio::test]
async fn invalid_old_token_does_not_disable_rotated_registration() {
    let provider =
        ScriptedProvider::new(vec![Err(PushProviderError::invalid_token("invalid_token"))]);
    let harness = Harness::new(Arc::new(provider)).await;
    register(&harness, "device-1", "old-token").await;
    let url = create_hook(&harness).await;
    assert_eq!(
        harness.json("POST", &url, json!({})).await.status,
        StatusCode::OK
    );
    register(&harness, "device-1", "new-token").await;

    let worker = harness.state.start_delivery_worker();
    wait_for_status(&harness, "invalid_token", 1).await;
    worker.shutdown().await;

    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM push_registrations WHERE fcm_token = 'new-token' AND disabled_at IS NULL",
    )
    .fetch_one(harness.state.database().pool())
    .await
    .expect("active new token");
    assert_eq!(active, 1);
}

#[tokio::test]
async fn startup_recovers_interrupted_in_progress_rows() {
    let provider = FakePushProvider::default();
    let harness = Harness::new(Arc::new(provider.clone())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;
    assert_eq!(
        harness.json("POST", &url, json!({})).await.status,
        StatusCode::OK
    );
    sqlx::query("UPDATE delivery_outbox SET status = 'in_progress'")
        .execute(harness.state.database().pool())
        .await
        .expect("simulate interrupted row");

    let worker = harness.state.start_delivery_worker();
    wait_for_status(&harness, "succeeded", 1).await;
    worker.shutdown().await;
    assert_eq!(provider.deliveries().len(), 1);
}

#[tokio::test]
async fn worker_shutdown_completes_without_pending_work() {
    let harness = Harness::new(Arc::new(FakePushProvider::default())).await;
    let worker = harness.state.start_delivery_worker();
    tokio::time::timeout(Duration::from_secs(1), worker.shutdown())
        .await
        .expect("shutdown should not hang");
}

#[tokio::test]
async fn stale_claim_failure_does_not_disable_after_newer_success() {
    let harness = Harness::new(Arc::new(FakePushProvider::default())).await;
    register(&harness, "device-1", "token-1").await;
    let url = create_hook(&harness).await;
    assert_eq!(
        harness.json("POST", &url, json!({})).await.status,
        StatusCode::OK
    );
    let outbox = DeliveryOutboxRepository::new(
        harness.state.database().pool().clone(),
        harness.state.database().backend(),
    );
    let ClaimDueOutcome::Claimed(stale) = outbox.claim_due().await.expect("claim stale") else {
        panic!("expected stale row");
    };
    sqlx::query("UPDATE delivery_outbox SET next_attempt_at = 0 WHERE outbox_id = $1")
        .bind(&stale.outbox_id)
        .execute(harness.state.database().pool())
        .await
        .expect("expire lease");
    let ClaimDueOutcome::Claimed(current) = outbox.claim_due().await.expect("claim current") else {
        panic!("expected current row");
    };
    assert_ne!(stale.claim_id, current.claim_id);
    assert!(
        outbox
            .mark_succeeded(&current.outbox_id, &current.claim_id)
            .await
            .expect("current success")
    );
    let stale_updated = outbox
        .mark_invalid_token_and_disable_registration(&stale, "invalid_token")
        .await
        .expect("stale failure fenced");
    assert!(!stale_updated);

    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM push_registrations WHERE installation_id = 'device-1' AND disabled_at IS NULL",
    )
    .fetch_one(harness.state.database().pool())
    .await
    .expect("active registration");
    assert_eq!(active, 1);
    assert_eq!(outbox_count(&harness, "succeeded").await, 1);
}

#[tokio::test]
async fn partial_failures_finish_independently() {
    let provider = TokenProvider;
    let harness = Harness::new(Arc::new(provider)).await;
    register(&harness, "device-1", "good-token").await;
    register(&harness, "device-2", "bad-token").await;
    let good_url = create_hook_for(&harness, "device-1").await;
    let bad_url = create_hook_for(&harness, "device-2").await;
    assert_eq!(
        harness.json("POST", &good_url, json!({})).await.status,
        StatusCode::OK
    );
    assert_eq!(
        harness.json("POST", &bad_url, json!({})).await.status,
        StatusCode::OK
    );

    let worker = harness.state.start_delivery_worker();
    wait_for_status(&harness, "succeeded", 1).await;
    wait_for_status(&harness, "invalid_token", 1).await;
    worker.shutdown().await;
}

async fn create_hook(harness: &Harness) -> String {
    create_hook_for(harness, "device-1").await
}

async fn create_hook_for(harness: &Harness, installation_id: &str) -> String {
    let create = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "installation_id": installation_id,
                "label":"outbox"
            }),
        )
        .await;
    assert_eq!(create.status, StatusCode::OK);
    create.body["invocation_url"]
        .as_str()
        .expect("url")
        .to_owned()
}

async fn register(harness: &Harness, installation_id: &str, token: &str) {
    let response = harness
        .json(
            "POST",
            "/registrations",
            json!({
                "installation_id": installation_id, "fcm_token": token, "platform":"android"
            }),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
}

async fn assert_management_request_waits_for_write_lock(
    harness: &Harness,
    state: AppState,
    request: Request<Body>,
) {
    let write_lock = harness.state.database().write_lock();
    let guard = write_lock.acquire_worker().await;
    let acquisition = write_lock.observe_next_acquisition().await;
    let response = tokio::spawn(async move {
        public_app(state)
            .oneshot(request)
            .await
            .expect("management response")
            .status()
    });
    acquisition
        .await
        .expect("request reached write-lock boundary");
    assert!(
        !response.is_finished(),
        "management write completed while the database-write lock was held"
    );

    drop(guard);
    assert_eq!(response.await.expect("management task"), StatusCode::OK);
}

fn management_empty_request(harness: &Harness, method: &str, uri: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .expect("request");
    request.headers_mut().insert(
        header::AUTHORIZATION,
        nostr_authorization(
            &harness.recipient_keys,
            harness.state.config().public_base_url(),
            method,
            uri,
            b"",
        )
        .parse()
        .expect("auth header"),
    );
    request
}

fn management_json_request(
    harness: &Harness,
    method: &str,
    uri: &str,
    body: Value,
) -> Request<Body> {
    let body = complete_hook_fixture(method, uri, body);
    let body = body.to_string();
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.clone()))
        .expect("request");
    request.headers_mut().insert(
        header::AUTHORIZATION,
        nostr_authorization(
            &harness.recipient_keys,
            harness.state.config().public_base_url(),
            method,
            uri,
            body.as_bytes(),
        )
        .parse()
        .expect("auth header"),
    );
    request
}

fn complete_hook_fixture(method: &str, uri: &str, mut body: Value) -> Value {
    if method == "POST"
        && uri == "/v1/hooks"
        && let Some(object) = body.as_object_mut()
    {
        object
            .entry("installation_id")
            .or_insert_with(|| json!("device-1"));
        let policy = object.entry("policy").or_insert_with(|| json!({}));
        if let Some(policy) = policy.as_object_mut() {
            policy.entry("ttl_seconds").or_insert_with(|| json!(3600));
        }
    }
    body
}

fn is_management_request(uri: &str) -> bool {
    let path = uri
        .strip_prefix("http://127.0.0.1:3000")
        .unwrap_or(uri)
        .split_once('?')
        .map_or(uri, |(path, _)| path);
    path == "/registrations" || path.starts_with("/registrations/") || path == "/v1/hooks"
}

fn nostr_authorization(
    keys: &Keys,
    public_base_url: &str,
    method: &str,
    uri: &str,
    body: &[u8],
) -> String {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let path_and_query = uri.strip_prefix(public_base_url).unwrap_or(uri);
    let mut builder = EventBuilder::new(Kind::HttpAuth, "")
        .custom_created_at(Timestamp::now())
        .tag(Tag::parse(["u", &format!("{public_base_url}{path_and_query}")]).expect("u tag"))
        .tag(Tag::parse(["method", method]).expect("method tag"))
        .tag(
            Tag::parse(["nonce", &NONCE.fetch_add(1, Ordering::Relaxed).to_string()])
                .expect("nonce"),
        );
    if !matches!(method, "GET" | "DELETE") {
        builder = builder
            .tag(Tag::parse(["payload", &hex::encode(Sha256::digest(body))]).expect("payload"));
    }
    let event = builder.sign_with_keys(keys).expect("sign");
    format!(
        "Nostr {}",
        general_purpose::STANDARD.encode(serde_json::to_vec(&event).expect("json"))
    )
}

async fn outbox_count(harness: &Harness, status: &str) -> i64 {
    DeliveryOutboxRepository::new(
        harness.state.database().pool().clone(),
        harness.state.database().backend(),
    )
    .count_by_status(status)
    .await
    .expect("count status")
}

async fn wait_for_status(harness: &Harness, status: &str, count: i64) {
    assert!(
        wait_for_status_maybe(harness, status, count).await,
        "timed out waiting for {status}"
    );
}

async fn wait_for_status_maybe(harness: &Harness, status: &str, count: i64) -> bool {
    const MAX_STATUS_POLLS: usize = 400;
    const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(50);

    for _ in 0..MAX_STATUS_POLLS {
        if outbox_count(harness, status).await >= count {
            return true;
        }
        tokio::time::sleep(STATUS_POLL_INTERVAL).await;
    }
    false
}

async fn wait_for_attempt_status(harness: &Harness, attempts: i64, status: &str) {
    const MAX_STATUS_POLLS: usize = 400;
    const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(50);

    for _ in 0..MAX_STATUS_POLLS {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_outbox
             WHERE attempts = $1 AND status = $2",
        )
        .bind(attempts)
        .bind(status)
        .fetch_one(harness.state.database().pool())
        .await
        .expect("read attempt status");
        if count == 1 {
            return;
        }
        tokio::time::sleep(STATUS_POLL_INTERVAL).await;
    }
    panic!("timed out waiting for attempt {attempts} status {status}");
}

fn assert_constraint_error(result: Result<sqlx::any::AnyQueryResult, sqlx::Error>) {
    match result {
        Ok(_) => panic!("expected constraint error"),
        Err(sqlx::Error::Database(error)) => {
            let message = error.message().to_ascii_lowercase();
            assert!(
                message.contains("constraint") || message.contains("unique"),
                "unexpected database error: {message}"
            );
        }
        Err(error) => panic!("unexpected error type: {error}"),
    }
}

fn unix_timestamp_for_test() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn metric_value(metrics: &str, name: &str) -> i64 {
    metrics
        .lines()
        .find_map(|line| {
            let (metric_name, value) = line.split_once(' ')?;
            (metric_name == name).then(|| value.parse().expect("integer metric"))
        })
        .unwrap_or_else(|| panic!("missing metric {name}"))
}

fn assert_metric_between(metrics: &str, name: &str, min: i64, max: i64) {
    let value = metric_value(metrics, name);
    assert!(
        (min..=max).contains(&value),
        "{name}={value} outside expected range {min}..={max}"
    );
}

#[derive(Debug)]
struct ScriptedProvider {
    outcomes: Mutex<VecDeque<Result<(), PushProviderError>>>,
}

impl ScriptedProvider {
    fn new(outcomes: Vec<Result<(), PushProviderError>>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into()),
        }
    }
}

impl PushProvider for ScriptedProvider {
    fn validate_registration<'a>(
        &'a self,
        _token: &'a fedi_decentralized_push_gateway::FcmRegistrationToken,
    ) -> fedi_decentralized_push_gateway::ProviderFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn deliver<'a>(
        &'a self,
        _registration: &'a fedi_decentralized_push_gateway::PushRegistration,
        _notification: &'a fedi_decentralized_push_gateway::Notification,
    ) -> fedi_decentralized_push_gateway::ProviderFuture<'a> {
        Box::pin(async move { self.outcomes.lock().await.pop_front().unwrap_or(Ok(())) })
    }
}

#[derive(Debug)]
struct TokenProvider;

impl PushProvider for TokenProvider {
    fn validate_registration<'a>(
        &'a self,
        _token: &'a fedi_decentralized_push_gateway::FcmRegistrationToken,
    ) -> fedi_decentralized_push_gateway::ProviderFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn deliver<'a>(
        &'a self,
        registration: &'a fedi_decentralized_push_gateway::PushRegistration,
        _notification: &'a fedi_decentralized_push_gateway::Notification,
    ) -> fedi_decentralized_push_gateway::ProviderFuture<'a> {
        Box::pin(async move {
            if registration.fcm_token.0 == "bad-token" {
                Err(PushProviderError::invalid_token("invalid_token"))
            } else {
                Ok(())
            }
        })
    }
}

struct Harness {
    _test_guard: OwnedMutexGuard<()>,
    _temp_dir: TempDir,
    state: AppState,
    recipient_keys: Keys,
}

impl Harness {
    async fn new(provider: Arc<dyn PushProvider>) -> Self {
        let test_guard = outbox_test_mutex().lock_owned().await;
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let database_path = temp_dir.path().join("push.sqlite");
        let config = PushGatewayConfig::new(
            Some(AppId("test-app".to_owned())),
            format!("sqlite://{}?mode=rwc", database_path.display()),
            None,
        )
        .try_with_local_test_public_base_url("http://127.0.0.1:3000")
        .expect("local test public base URL")
        .with_rate_limits(fedi_decentralized_push_gateway::RateLimitConfig {
            hook_creations_per_recipient: 0,
            max_active_hooks_per_recipient: 0,
            ..Default::default()
        });
        let database = Database::connect(config.database_url())
            .await
            .expect("connect database");
        let state = AppState::with_push_provider(config, database, provider);
        Self {
            _test_guard: test_guard,
            _temp_dir: temp_dir,
            state,
            recipient_keys: Keys::generate(),
        }
    }

    async fn json(&self, method: &str, uri: &str, body: Value) -> Response {
        let body = complete_hook_fixture(method, uri, body);
        let body_string = body.to_string();
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body_string.clone()))
            .expect("request");
        let mut request = request;
        if is_management_request(uri) {
            request.headers_mut().insert(
                header::AUTHORIZATION,
                nostr_authorization(
                    &self.recipient_keys,
                    self.state.config().public_base_url(),
                    method,
                    uri,
                    body_string.as_bytes(),
                )
                .parse()
                .expect("auth header"),
            );
        }
        let response = public_app(self.state.clone())
            .oneshot(request)
            .await
            .expect("response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let body = serde_json::from_slice(&bytes).expect("json response");
        Response { status, body }
    }

    async fn text(&self, method: &str, uri: &str) -> TextResponse {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .expect("request");
        let response = operator_app(self.state.clone())
            .oneshot(request)
            .await
            .expect("response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        TextResponse {
            status,
            text: String::from_utf8(bytes.to_vec()).expect("utf8 body"),
        }
    }
}

fn outbox_test_mutex() -> Arc<Mutex<()>> {
    // Keep this serialization local to the outbox worker/retry integration
    // tests. Cargo's in-binary parallel test mode can otherwise create
    // unrelated SQLite lock noise while multiple background workers poll,
    // claim, and manually edit retry deadlines at the same time, even though
    // each test uses its own temporary database.
    static OUTBOX_TEST_MUTEX: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    OUTBOX_TEST_MUTEX
        .get_or_init(|| Arc::new(Mutex::new(())))
        .clone()
}

struct Response {
    status: StatusCode,
    body: Value,
}

struct TextResponse {
    status: StatusCode,
    text: String,
}
