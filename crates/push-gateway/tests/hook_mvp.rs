use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use axum::{
    body::{Body, to_bytes},
    extract::connect_info::ConnectInfo,
    http::{HeaderName, Request, StatusCode, header},
};
use base64::{Engine, engine::general_purpose};
use fedi_decentralized_push_gateway::{
    AppId, AppState, Database, DeliveryOutboxRepository, DeliveryWorkerHandle, FakePushProvider,
    HookToken, OperatorToken, PushGatewayConfig, RateLimitConfig, operator_app, public_app,
};
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::sync::{Barrier, Mutex, OwnedMutexGuard};
use tower::ServiceExt;

const TEST_RATE_LIMIT_WINDOW_SECONDS: i64 = 60;
const TEST_RATE_LIMIT_MAX_REQUESTS: i64 = 30;

#[tokio::test]
async fn signed_registration_and_management_share_admission_profile() {
    let open = Harness::new_with_config(|config| {
        config
            .with_open_self_registration_enabled(true)
            .with_admission_allowed_recipients(Vec::<String>::new())
    })
    .await;
    let registration = open
        .json(
            "POST",
            "/registrations",
            json!({"installation_id": "device-1", "fcm_token": "token-1"}),
        )
        .await;
    assert_eq!(registration.status, StatusCode::OK);
    let hook = open
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "installation_id": "device-1",
                "policy": {"ttl_seconds": 3600}
            }),
        )
        .await;
    assert_eq!(hook.status, StatusCode::OK);
    drop(open);

    let restricted = Harness::new_with_config(|config| {
        config
            .with_open_self_registration_enabled(false)
            .with_admission_allowed_recipients(["00".repeat(32)])
    })
    .await;
    for (uri, body) in [
        (
            "/registrations",
            json!({"installation_id": "device-1", "fcm_token": "token-1"}),
        ),
        (
            "/v1/hooks",
            json!({
                "installation_id": "device-1",
                "policy": {"ttl_seconds": 3600}
            }),
        ),
    ] {
        let response = restricted.json("POST", uri, body).await;
        assert_eq!(response.status, StatusCode::FORBIDDEN);
        assert_eq!(response.body["error"]["code"], "recipient_not_admitted");
    }
}

#[tokio::test]
async fn hook_happy_path_records_fake_delivery() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;

    let create = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "installation_id": "device-1",
                "label": "ci hook",
                "notification": {
                    "kind": "ci",
                    "title": "default title",
                    "body": "default body"
                },
                "policy": { "ttl_seconds": 3600, "max_uses": 2 }
            }),
        )
        .await;
    assert_eq!(create.status, StatusCode::OK);
    assert_eq!(create.cache_control.as_deref(), Some("no-store"));
    assert_eq!(create.pragma.as_deref(), Some("no-cache"));
    let invocation_url = create.body["invocation_url"]
        .as_str()
        .expect("invocation_url");
    let hook_id = create.body["hook"]["hook_id"].as_str().expect("hook_id");
    let hook_secret = create.body["hook_secret"].as_str().expect("hook_secret");
    assert_eq!(
        invocation_url,
        format!("http://127.0.0.1:3000/hooks/{hook_id}/{hook_secret}")
    );
    assert!(create.body.get("secret").is_none());
    assert!(create.body.get("hook_token").is_none());

    let invoke = harness
        .json(
            "POST",
            invocation_url,
            json!({
                "idempotency_key": "event-1",
                "data": { "severity": "info" }
            }),
        )
        .await;
    assert_eq!(invoke.status, StatusCode::OK);
    assert_eq!(invoke.body["accepted"], true);
    assert_eq!(invoke.body["delivery_attempts"], 1);

    let deliveries = wait_for_deliveries(&harness, 1).await;
    assert_eq!(deliveries.len(), 1);
    assert_eq!(
        deliveries[0].registration.recipient_id.0,
        harness.recipient_keys.public_key().to_string()
    );
    assert!(
        deliveries[0]
            .notification
            .notification_id
            .0
            .starts_with(&format!("hook:{hook_id}:"))
    );
    assert_eq!(deliveries[0].notification.kind.0, "ci");
    assert_eq!(
        deliveries[0].notification.title.as_deref(),
        Some("default title")
    );
    assert_eq!(
        deliveries[0].notification.body.as_deref(),
        Some("default body")
    );
}

#[tokio::test]
async fn installation_scoped_hook_delivers_only_to_initiating_installation() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let second = harness
        .json(
            "POST",
            "/registrations",
            json!({
                "installation_id": "device-2",
                "fcm_token": "fcm-token-2",
                "platform": "ios"
            }),
        )
        .await;
    assert_eq!(second.status, StatusCode::OK);

    let create = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "installation_id": "device-1",
                "notification": {"kind": "formation_update"},
                "policy": {"ttl_seconds": 3600}
            }),
        )
        .await;
    assert_eq!(create.status, StatusCode::OK);
    assert_eq!(create.body["hook"]["installation_id"], "device-1");

    let invoke = harness
        .json(
            "POST",
            create.body["invocation_url"].as_str().expect("url"),
            json!({"idempotency_key": "shared-dkg-completion"}),
        )
        .await;
    assert_eq!(invoke.status, StatusCode::OK);
    assert_eq!(invoke.body["delivery_attempts"], 1);

    let deliveries = wait_for_deliveries(&harness, 1).await;
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].registration.installation_id.0, "device-1");
}

#[tokio::test]
async fn accepted_key_remains_idempotent_after_sensitive_delivery_retention_cleanup() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let create = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "installation_id": "device-1",
                "notification": {"kind": "formation_update"},
                "policy": {"ttl_seconds": 2_592_000}
            }),
        )
        .await;
    assert_eq!(create.status, StatusCode::OK);
    let url = create.body["invocation_url"].as_str().expect("url");

    let accepted = harness
        .json(
            "POST",
            url,
            json!({"idempotency_key": "shared-dkg-completion"}),
        )
        .await;
    assert_eq!(accepted.status, StatusCode::OK);
    assert_eq!(accepted.body["delivery_attempts"], 1);
    assert_eq!(wait_for_deliveries(&harness, 1).await.len(), 1);

    sqlx::query(
        "UPDATE delivery_outbox
         SET status = 'succeeded', updated_at = 1
         WHERE event_id IN (
             SELECT event_id FROM notification_events
             WHERE caller_idempotency_key = 'shared-dkg-completion'
         )",
    )
    .execute(harness.state.database().pool())
    .await
    .expect("make delivery eligible for retention purge");
    sqlx::query(
        "UPDATE notification_events SET created_at = 1
         WHERE caller_idempotency_key = 'shared-dkg-completion'",
    )
    .execute(harness.state.database().pool())
    .await
    .expect("make event eligible for retention purge");
    let purged = DeliveryOutboxRepository::new(
        harness.state.database().pool().clone(),
        harness.state.database().backend(),
    )
    .purge_retained_sensitive_data(2, 2)
    .await
    .expect("purge sensitive event and outbox data");
    assert_eq!(purged.delivery_outbox_rows, 1);
    assert_eq!(purged.notification_event_rows, 1);
    assert_eq!(purged.idempotency_tombstone_rows, 0);

    let replay = harness
        .json(
            "POST",
            url,
            json!({"idempotency_key": "shared-dkg-completion"}),
        )
        .await;
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(replay.body["delivery_attempts"], 1);
    assert_eq!(harness.provider.deliveries().len(), 1);
    let (uses, events, outbox, tombstones): (i64, i64, i64, i64) = (
        sqlx::query_scalar("SELECT SUM(use_count) FROM notification_hooks")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count uses"),
        sqlx::query_scalar("SELECT COUNT(*) FROM notification_events")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count events"),
        sqlx::query_scalar("SELECT COUNT(*) FROM delivery_outbox")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count outbox rows"),
        sqlx::query_scalar("SELECT COUNT(*) FROM hook_idempotency_tombstones")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count tombstones"),
    );
    assert_eq!((uses, events, outbox, tombstones), (1, 0, 0, 1));
}

#[tokio::test]
async fn idempotency_marker_capacity_fails_closed_before_mutating_hook_state() {
    let limits = RateLimitConfig {
        max_hook_rows_global: 1,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;
    let create = create_targeted_hook(&harness, "device-1").await;
    let url = create.body["invocation_url"].as_str().expect("url");

    assert_eq!(
        harness
            .json("POST", url, json!({"idempotency_key": "first"}))
            .await
            .status,
        StatusCode::OK
    );
    let rejected = harness
        .json("POST", url, json!({"idempotency_key": "second"}))
        .await;
    assert_eq!(rejected.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        rejected.body["error"]["code"],
        "idempotency_capacity_exceeded"
    );
    let (uses, events, outbox, tombstones): (i64, i64, i64, i64) = (
        sqlx::query_scalar("SELECT SUM(use_count) FROM notification_hooks")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count uses"),
        sqlx::query_scalar("SELECT COUNT(*) FROM notification_events")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count events"),
        sqlx::query_scalar("SELECT COUNT(*) FROM delivery_outbox")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count outbox rows"),
        sqlx::query_scalar("SELECT COUNT(*) FROM hook_idempotency_tombstones")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count tombstones"),
    );
    assert_eq!((uses, events, outbox, tombstones), (1, 1, 1, 1));
}

#[tokio::test]
async fn missing_invocation_target_is_retryable_with_same_idempotency_key() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let hook = create_targeted_hook(&harness, "device-1").await;
    assert_eq!(hook.status, StatusCode::OK);
    assert_eq!(
        harness
            .empty("DELETE", "/registrations/device-1")
            .await
            .status,
        StatusCode::OK
    );
    assert_unavailable_then_refreshes_once(&harness, &hook, "missing-target").await;
}

#[tokio::test]
async fn disabled_invocation_target_is_retryable_with_same_idempotency_key() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let hook = create_targeted_hook(&harness, "device-1").await;
    assert_eq!(hook.status, StatusCode::OK);
    assert_eq!(
        harness
            .empty("POST", "/registrations/device-1/disable")
            .await
            .status,
        StatusCode::OK
    );
    assert_unavailable_then_refreshes_once(&harness, &hook, "disabled-target").await;
}

#[tokio::test]
async fn stale_invocation_target_is_retryable_with_same_idempotency_key() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let hook = create_targeted_hook(&harness, "device-1").await;
    assert_eq!(hook.status, StatusCode::OK);
    sqlx::query("UPDATE push_registrations SET last_seen_at = 0")
        .execute(harness.state.database().pool())
        .await
        .unwrap();
    assert_unavailable_then_refreshes_once(&harness, &hook, "stale-target").await;
}

#[tokio::test]
async fn hook_target_must_be_an_active_owned_registration() {
    let harness = Harness::new().await;
    let create = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "installation_id": "not-registered",
                "policy": {"ttl_seconds": 3600}
            }),
        )
        .await;

    assert_eq!(create.status, StatusCode::NOT_FOUND);
    assert_eq!(create.body["error"]["code"], "registration_not_found");
    assert_eq!(hook_row_count(&harness).await, 0);
}

#[tokio::test]
async fn hook_target_owned_by_another_signer_is_rejected_without_persistence() {
    let harness = Harness::new().await;
    let other = Keys::generate();
    let body = json!({"installation_id": "shared-device", "fcm_token": "other-token"}).to_string();
    let registered = request_with_state_signed(
        harness.state.clone(),
        &other,
        "POST",
        "/registrations",
        Body::from(body),
    )
    .await;
    assert_eq!(registered.status, StatusCode::OK);

    let create = create_targeted_hook(&harness, "shared-device").await;
    assert_eq!(create.status, StatusCode::NOT_FOUND);
    assert_eq!(create.body["error"]["code"], "registration_not_found");
    assert_eq!(hook_row_count(&harness).await, 0);
}

#[tokio::test]
async fn disabled_hook_target_is_rejected_without_persistence() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    assert_eq!(
        harness
            .empty("POST", "/registrations/device-1/disable")
            .await
            .status,
        StatusCode::OK
    );
    let create = create_targeted_hook(&harness, "device-1").await;
    assert_eq!(create.status, StatusCode::NOT_FOUND);
    assert_eq!(hook_row_count(&harness).await, 0);
}

#[tokio::test]
async fn stale_hook_target_is_rejected_without_persistence() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    sqlx::query("UPDATE push_registrations SET last_seen_at = 0")
        .execute(harness.state.database().pool())
        .await
        .unwrap();
    let create = create_targeted_hook(&harness, "device-1").await;
    assert_eq!(create.status, StatusCode::NOT_FOUND);
    assert_eq!(hook_row_count(&harness).await, 0);
}

#[tokio::test]
async fn signed_refresh_restores_valid_hook_target_boundary() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    sqlx::query("UPDATE push_registrations SET last_seen_at = 0")
        .execute(harness.state.database().pool())
        .await
        .unwrap();
    register_installation(&harness, "recipient").await;
    let create = create_targeted_hook(&harness, "device-1").await;
    assert_eq!(create.status, StatusCode::OK);
    assert_eq!(hook_row_count(&harness).await, 1);
}

#[tokio::test]
async fn cross_recipient_token_reassignment_is_refused_without_delete_or_spam_hook() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let attacker = Keys::generate();
    let body =
        json!({"installation_id": "attacker-device", "fcm_token": "fcm-token-1"}).to_string();
    let stolen = request_with_state_signed(
        harness.state.clone(),
        &attacker,
        "POST",
        "/registrations",
        Body::from(body),
    )
    .await;
    assert_eq!(stolen.status, StatusCode::CONFLICT);
    assert_eq!(
        stolen.body["error"]["code"],
        "fcm_token_bound_to_different_installation"
    );
    let owner: String = sqlx::query_scalar(
        "SELECT recipient_id FROM push_registrations WHERE fcm_token = 'fcm-token-1'",
    )
    .fetch_one(harness.state.database().pool())
    .await
    .unwrap();
    assert_eq!(owner, harness.recipient_keys.public_key().to_string());

    let attacker_hook = request_with_state_signed(
        harness.state.clone(),
        &attacker,
        "POST",
        "/v1/hooks",
        Body::from(
            json!({
                "installation_id": "device-1",
                "policy": {"ttl_seconds": 3600}
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(attacker_hook.status, StatusCode::NOT_FOUND);
    assert_eq!(hook_row_count(&harness).await, 0);
}

#[tokio::test]
async fn account_switch_reassigns_only_the_same_installation_and_strands_old_hooks() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let old_hook = create_targeted_hook(&harness, "device-1").await;
    assert_eq!(old_hook.status, StatusCode::OK);

    let new_recipient = Keys::generate();
    let body = json!({
        "installation_id": "device-1",
        "fcm_token": "fcm-token-1",
        "platform": "android"
    })
    .to_string();
    let moved = request_with_state_signed(
        harness.state.clone(),
        &new_recipient,
        "POST",
        "/registrations",
        Body::from(body),
    )
    .await;
    assert_eq!(moved.status, StatusCode::OK);

    let owner: String = sqlx::query_scalar(
        "SELECT recipient_id FROM push_registrations WHERE fcm_token = 'fcm-token-1'",
    )
    .fetch_one(harness.state.database().pool())
    .await
    .unwrap();
    assert_eq!(owner, new_recipient.public_key().to_string());

    let old_url = old_hook.body["invocation_url"]
        .as_str()
        .expect("old invocation URL");
    let old_delivery = harness
        .json(
            "POST",
            old_url,
            json!({"idempotency_key": "after-account-switch"}),
        )
        .await;
    assert_eq!(old_delivery.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        old_delivery.body["error"]["code"],
        "target_installation_unavailable"
    );

    let new_hook = request_with_state_signed(
        harness.state.clone(),
        &new_recipient,
        "POST",
        "/v1/hooks",
        Body::from(
            json!({
                "installation_id": "device-1",
                "policy": {"ttl_seconds": 3600}
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(new_hook.status, StatusCode::OK);
}

#[tokio::test]
async fn latest_valid_exact_pair_registration_wins_in_both_directions_atomically() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let first_recipient = harness.recipient_keys.public_key().to_string();
    let second_keys = Keys::generate();
    let second_recipient = second_keys.public_key().to_string();
    let exact_pair = json!({
        "installation_id": "device-1",
        "fcm_token": "fcm-token-1",
        "platform": "android"
    })
    .to_string();

    // The first clone prepares a still-valid refresh, but the second clone's
    // switch reaches the serialized mutation boundary first.
    let delayed_first_refresh = exact_pair.clone();
    let second_takes_over = request_with_state_signed(
        harness.state.clone(),
        &second_keys,
        "POST",
        "/registrations",
        Body::from(exact_pair.clone()),
    )
    .await;
    assert_eq!(second_takes_over.status, StatusCode::OK);
    assert_exact_pair_owner(&harness, &second_recipient).await;

    // The delayed valid refresh commits later and therefore takes ownership
    // back without leaving either durable table on the previous recipient.
    let first_takes_back = request_with_state_signed(
        harness.state.clone(),
        &harness.recipient_keys,
        "POST",
        "/registrations",
        Body::from(delayed_first_refresh),
    )
    .await;
    assert_eq!(first_takes_back.status, StatusCode::OK);
    assert_exact_pair_owner(&harness, &first_recipient).await;

    // The same rule applies in the opposite direction: clones can oscillate
    // until only one continues submitting valid registrations.
    let second_takes_back = request_with_state_signed(
        harness.state.clone(),
        &second_keys,
        "POST",
        "/registrations",
        Body::from(exact_pair),
    )
    .await;
    assert_eq!(second_takes_back.status, StatusCode::OK);
    assert_exact_pair_owner(&harness, &second_recipient).await;
}

#[tokio::test]
async fn hook_app_open_context_is_owned_by_created_hook() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;

    let create = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "notification": {
                    "kind": "federation.setup",
                    "privacy": "display_text",
                    "title": "Federation update"
                },
                "open": {
                    "workflow": "federation_setup",
                    "action": "review_guardians",
                    "deep_link": "fedi://workflows/federation-setup/review",
                    "behavior": "open_deep_link"
                },
                "data": { "federation_id": "fed-1" }
            }),
        )
        .await;
    assert_eq!(create.status, StatusCode::OK);
    assert_eq!(create.body["hook"]["open"]["workflow"], "federation_setup");
    assert_eq!(create.body["hook"]["open"]["action"], "review_guardians");
    assert_eq!(create.body["hook"]["open"]["behavior"], "open_deep_link");

    let invoke = harness
        .json(
            "POST",
            create.body["invocation_url"].as_str().expect("url"),
            json!({
                "idempotency_key": "event-context",
                "data": { "caller_reference": "abc" }
            }),
        )
        .await;
    assert_eq!(invoke.status, StatusCode::OK);

    let deliveries = wait_for_deliveries(&harness, 1).await;
    let notification = &deliveries[0].notification;
    assert_eq!(notification.kind.0, "federation.setup");
    assert_eq!(notification.title.as_deref(), Some("Federation update"));
    assert_eq!(notification.data["pg.workflow"], "federation_setup");
    assert_eq!(notification.data["pg.action"], "review_guardians");
    assert_eq!(
        notification.data["pg.deep_link"],
        "fedi://workflows/federation-setup/review"
    );
    assert_eq!(notification.data["pg.open_behavior"], "open_deep_link");
    assert_eq!(notification.data["federation_id"], "fed-1");
    assert_eq!(notification.data["caller_reference"], "abc");
}

#[tokio::test]
async fn caller_cannot_inject_reserved_app_open_context() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let create = create_hook(&harness, None, None).await;

    let reserved = harness
        .json(
            "POST",
            create.body["invocation_url"].as_str().expect("url"),
            json!({ "data": { "pg.deep_link": "fedi://evil" } }),
        )
        .await;
    assert_eq!(reserved.status, StatusCode::BAD_REQUEST);
    assert_eq!(reserved.body["error"]["code"], "data_key_reserved");
    assert!(harness.provider.deliveries().is_empty());

    for key in ["event_id", "recipient_id", "deep_link"] {
        let data = serde_json::Map::from_iter([(key.to_owned(), json!("bad"))]);
        let reserved = harness
            .json(
                "POST",
                create.body["invocation_url"].as_str().expect("url"),
                json!({ "data": data }),
            )
            .await;
        assert_eq!(reserved.status, StatusCode::BAD_REQUEST);
        assert_eq!(reserved.body["error"]["code"], "data_key_reserved");
    }

    let create_reserved = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "data": { "pg.workflow": "x" }
            }),
        )
        .await;
    assert_eq!(create_reserved.status, StatusCode::BAD_REQUEST);
    assert_eq!(create_reserved.body["error"]["code"], "data_key_reserved");
}

#[tokio::test]
async fn old_flat_hook_api_fields_are_rejected() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;

    let old_create = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "kind": "legacy-flat",
                "title": "caller controlled",
                "open_behavior": "open_app"
            }),
        )
        .await;
    assert_eq!(old_create.status, StatusCode::BAD_REQUEST);
    assert_eq!(old_create.body["error"]["code"], "invalid_json");

    let create = create_hook(&harness, None, None).await;
    assert_eq!(create.status, StatusCode::OK);
    let url = create.body["invocation_url"].as_str().expect("url");
    for payload in [
        json!({ "event_id": "old-event" }),
        json!({ "title": "caller title" }),
        json!({ "body": "caller body" }),
    ] {
        let response = harness.json("POST", url, payload).await;
        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(response.body["error"]["code"], "invalid_json");
    }
}

#[tokio::test]
async fn app_open_contract_validation_rejects_invalid_hook_context() {
    let harness = Harness::new().await;

    let empty_kind = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "notification": { "kind": "" }
            }),
        )
        .await;
    assert_eq!(empty_kind.status, StatusCode::BAD_REQUEST);
    assert_eq!(empty_kind.body["error"]["code"], "field_required");

    let missing_workflow = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "open": { "behavior": "open_workflow" }
            }),
        )
        .await;
    assert_eq!(missing_workflow.status, StatusCode::BAD_REQUEST);
    assert_eq!(missing_workflow.body["error"]["code"], "workflow_required");

    for deep_link in [
        "https://example.test/path",
        "//example.test/path",
        "fedi://",
    ] {
        let invalid = harness
            .json(
                "POST",
                "/v1/hooks",
                json!({
                    "open": {
                        "deep_link": deep_link,
                        "behavior": "open_deep_link"
                    }
                }),
            )
            .await;
        assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
        assert_eq!(invalid.body["error"]["code"], "deep_link_invalid");
    }
}

#[tokio::test]
async fn data_only_privacy_strips_display_text_from_payloads() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let create = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "notification": {
                    "privacy": "data_only",
                    "title": "stored title",
                    "body": "stored body"
                }
            }),
        )
        .await;
    assert_eq!(create.status, StatusCode::OK);

    let invoke = harness
        .json(
            "POST",
            create.body["invocation_url"].as_str().expect("url"),
            json!({ "data": { "reference": "caller" } }),
        )
        .await;
    assert_eq!(invoke.status, StatusCode::OK);

    let deliveries = wait_for_deliveries(&harness, 1).await;
    assert_eq!(deliveries[0].notification.title, None);
    assert_eq!(deliveries[0].notification.body, None);
    assert_eq!(deliveries[0].notification.data["pg.privacy"], "data_only");
}

#[tokio::test]
async fn hook_secret_is_not_listed_and_wrong_token_is_not_found() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let create = create_hook(&harness, None, None).await;
    let hook_id = create.body["hook"]["hook_id"].as_str().expect("hook_id");

    let list = harness.empty("GET", "/v1/hooks").await;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(list.body["hooks"][0]["hook_id"], hook_id);
    assert!(list.body["hooks"][0].get("secret").is_none());
    assert!(list.body["hooks"][0].get("hook_secret").is_none());
    assert!(list.body["hooks"][0].get("hook_secret_hash").is_none());

    let wrong = harness
        .json("POST", "/hooks/wrong-id/wrong-secret", json!({}))
        .await;
    assert_eq!(wrong.status, StatusCode::NOT_FOUND);
    assert_eq!(wrong.body["error"]["code"], "hook_not_found");
    assert!(harness.provider.deliveries().is_empty());
}

#[tokio::test]
async fn stored_hook_secret_hash_is_invoked_with_public_hook_id_and_secret() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let hook_secret = "url-secret";
    let hook_secret_hash = HookToken::from_path_segment(hook_secret.to_owned()).hash_hex();
    sqlx::query(
        "INSERT INTO notification_hooks (
             hook_id, hook_secret_hash, recipient_id, installation_id, open_behavior,
             privacy, data_json, created_at, expires_at, rate_limit_window_seconds,
             rate_limit_max_requests
          ) VALUES ($1, $2, $3, 'device-1', 'open_app', 'display_text', '{}',
                    1, 4000000000, 60, 30)",
    )
    .bind("manual-hook-id")
    .bind(hook_secret_hash)
    .bind(harness.recipient_keys.public_key().to_string())
    .execute(harness.state.database().pool())
    .await
    .expect("insert manual hook row");

    let response = harness
        .json(
            "POST",
            &format!("/hooks/manual-hook-id/{hook_secret}"),
            json!({}),
        )
        .await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body["accepted"], true);
    assert_eq!(response.body["delivery_attempts"], 1);
}

#[tokio::test]
async fn revoked_expired_and_max_use_hooks_are_rejected() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;

    let revoked = create_hook(&harness, None, None).await;
    let revoked_hook_id = revoked.body["hook"]["hook_id"].as_str().expect("hook_id");
    let revoke = harness
        .empty("DELETE", &format!("/v1/hooks/{revoked_hook_id}"))
        .await;
    assert_eq!(revoke.status, StatusCode::OK);
    let revoked_invoke = harness
        .json(
            "POST",
            revoked.body["invocation_url"].as_str().expect("url"),
            json!({}),
        )
        .await;
    assert_eq!(revoked_invoke.status, StatusCode::GONE);
    assert_eq!(revoked_invoke.body["error"]["code"], "hook_revoked");

    let expired = create_hook(&harness, Some(60), None).await;
    let expired_hook_id = expired.body["hook"]["hook_id"].as_str().expect("hook_id");
    sqlx::query("UPDATE notification_hooks SET expires_at = 0 WHERE hook_id = $1")
        .bind(expired_hook_id)
        .execute(harness.state.database().pool())
        .await
        .expect("expire hook");
    let expired_invoke = harness
        .json(
            "POST",
            expired.body["invocation_url"].as_str().expect("url"),
            json!({}),
        )
        .await;
    assert_eq!(expired_invoke.status, StatusCode::GONE);
    assert_eq!(expired_invoke.body["error"]["code"], "hook_expired");

    let one_use = create_hook(&harness, None, Some(1)).await;
    let one_use_url = one_use.body["invocation_url"].as_str().expect("url");
    let first = harness.json("POST", one_use_url, json!({})).await;
    assert_eq!(first.status, StatusCode::OK);
    let second = harness.json("POST", one_use_url, json!({})).await;
    assert_eq!(second.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(second.body["error"]["code"], "hook_max_uses_exceeded");
}

#[tokio::test]
async fn revocation_rejects_replay_of_a_previously_accepted_key() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let hook = create_hook(&harness, Some(3600), None).await;
    let url = hook.body["invocation_url"].as_str().expect("url");
    let accepted = harness
        .json("POST", url, json!({"idempotency_key": "accepted-key"}))
        .await;
    assert_eq!(accepted.status, StatusCode::OK);

    let hook_id = hook.body["hook"]["hook_id"].as_str().expect("hook id");
    let revoked = harness
        .empty("DELETE", &format!("/v1/hooks/{hook_id}"))
        .await;
    assert_eq!(revoked.status, StatusCode::OK);
    let replay = harness
        .json("POST", url, json!({"idempotency_key": "accepted-key"}))
        .await;
    assert_eq!(replay.status, StatusCode::GONE);
    assert_eq!(replay.body["error"]["code"], "hook_revoked");
    assert_eq!(harness.provider.deliveries().len(), 1);
}

#[tokio::test]
async fn concurrent_max_use_hook_only_delivers_once() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let one_use = create_hook(&harness, None, Some(1)).await;
    let one_use_url = one_use.body["invocation_url"]
        .as_str()
        .expect("url")
        .to_owned();
    let barrier = Arc::new(Barrier::new(3));

    let first_state = harness.state.clone();
    let first_url = one_use_url.clone();
    let first_barrier = barrier.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        request_with_state(first_state, "POST", &first_url, Body::from("{}")).await
    });

    let second_state = harness.state.clone();
    let second_barrier = barrier.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        request_with_state(second_state, "POST", &one_use_url, Body::from("{}")).await
    });

    barrier.wait().await;
    let first = first.await.expect("first invocation task");
    let second = second.await.expect("second invocation task");
    let success_count = [first.status, second.status]
        .into_iter()
        .filter(|status| *status == StatusCode::OK)
        .count();
    let limited_count = [first.status, second.status]
        .into_iter()
        .filter(|status| *status == StatusCode::TOO_MANY_REQUESTS)
        .count();

    assert_eq!(success_count, 1);
    assert_eq!(limited_count, 1);
    assert_eq!(wait_for_deliveries(&harness, 1).await.len(), 1);
}

#[tokio::test]
async fn concurrent_same_idempotency_key_consumes_hook_once() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let one_use = create_hook(&harness, None, Some(1)).await;
    let hook_id = one_use.body["hook"]["hook_id"]
        .as_str()
        .expect("hook_id")
        .to_owned();
    let url = one_use.body["invocation_url"]
        .as_str()
        .expect("url")
        .to_owned();
    let barrier = Arc::new(Barrier::new(3));

    let first_state = harness.state.clone();
    let first_url = url.clone();
    let first_barrier = barrier.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        request_with_state(
            first_state,
            "POST",
            &first_url,
            Body::from(json!({"idempotency_key":"same"}).to_string()),
        )
        .await
    });
    let second_state = harness.state.clone();
    let second_barrier = barrier.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        request_with_state(
            second_state,
            "POST",
            &url,
            Body::from(json!({"idempotency_key":"same"}).to_string()),
        )
        .await
    });

    barrier.wait().await;
    let first = first.await.expect("first");
    let second = second.await.expect("second");
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(second.status, StatusCode::OK);
    let use_count: i64 =
        sqlx::query_scalar("SELECT use_count FROM notification_hooks WHERE hook_id = $1")
            .bind(&hook_id)
            .fetch_one(harness.state.database().pool())
            .await
            .expect("use count");
    let event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM notification_events WHERE hook_id = $1")
            .bind(&hook_id)
            .fetch_one(harness.state.database().pool())
            .await
            .expect("event count");
    assert_eq!(use_count, 1);
    assert_eq!(event_count, 1);
}

#[tokio::test]
async fn concurrent_distinct_idempotency_keys_still_respect_max_use() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let one_use = create_hook(&harness, None, Some(1)).await;
    let one_use_url = one_use.body["invocation_url"]
        .as_str()
        .expect("url")
        .to_owned();
    let barrier = Arc::new(Barrier::new(3));

    let first_state = harness.state.clone();
    let first_url = one_use_url.clone();
    let first_barrier = barrier.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        request_with_state(
            first_state,
            "POST",
            &first_url,
            Body::from(json!({"idempotency_key":"first"}).to_string()),
        )
        .await
    });

    let second_state = harness.state.clone();
    let second_barrier = barrier.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        request_with_state(
            second_state,
            "POST",
            &one_use_url,
            Body::from(json!({"idempotency_key":"second"}).to_string()),
        )
        .await
    });

    barrier.wait().await;
    let first = first.await.expect("first invocation task");
    let second = second.await.expect("second invocation task");
    let success_count = [first.status, second.status]
        .into_iter()
        .filter(|status| *status == StatusCode::OK)
        .count();
    let limited_count = [first.status, second.status]
        .into_iter()
        .filter(|status| *status == StatusCode::TOO_MANY_REQUESTS)
        .count();

    assert_eq!(success_count, 1);
    assert_eq!(limited_count, 1);
    assert_eq!(wait_for_deliveries(&harness, 1).await.len(), 1);
}

#[tokio::test]
async fn hook_rate_limit_rejects_excess_invocations_in_fixed_window() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let create = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "policy": { "rate_limit": { "window_seconds": 60, "max_requests": 2 } }
            }),
        )
        .await;
    assert_eq!(create.status, StatusCode::OK);
    assert_eq!(
        create.body["hook"]["policy"]["rate_limit"]["window_seconds"],
        60
    );
    assert_eq!(
        create.body["hook"]["policy"]["rate_limit"]["max_requests"],
        2
    );
    let url = create.body["invocation_url"].as_str().expect("url");

    assert_eq!(
        harness
            .json("POST", url, json!({"idempotency_key": "first"}))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        harness
            .json("POST", url, json!({"idempotency_key": "second"}))
            .await
            .status,
        StatusCode::OK
    );
    let limited = harness
        .json("POST", url, json!({"idempotency_key": "third"}))
        .await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.body["error"]["code"], "hook_rate_limited");
    let metrics = harness
        .operator_text_with_header("GET", "/metrics", None)
        .await;
    assert_eq!(metrics.status, StatusCode::OK);
    assert!(
        metrics
            .text
            .contains("push_gateway_rate_limit_rejections_total 1")
    );
    assert_eq!(wait_for_deliveries(&harness, 2).await.len(), 2);

    // The fake provider records a delivery before the worker commits its
    // corresponding outbox update. Wait for those transactions to finish
    // before directly editing the hook below.
    for _ in 0..50 {
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_outbox
             WHERE status IN ('pending', 'retrying', 'in_progress')",
        )
        .fetch_one(harness.state.database().pool())
        .await
        .expect("count unsettled deliveries");
        if pending == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_outbox
         WHERE status IN ('pending', 'retrying', 'in_progress')",
    )
    .fetch_one(harness.state.database().pool())
    .await
    .expect("count unsettled deliveries");
    assert_eq!(pending, 0, "delivery worker did not settle the outbox");

    let hook_id = create.body["hook"]["hook_id"].as_str().expect("hook_id");
    sqlx::query(
        "UPDATE notification_hooks
         SET rate_limit_window_started_at = rate_limit_window_started_at - 61
         WHERE hook_id = $1",
    )
    .bind(hook_id)
    .execute(harness.state.database().pool())
    .await
    .expect("move fixed window into the past");

    let fourth = harness
        .json("POST", url, json!({"idempotency_key": "fourth"}))
        .await;
    assert_eq!(fourth.status, StatusCode::OK, "{:?}", fourth.body);
    assert_eq!(wait_for_deliveries(&harness, 3).await.len(), 3);
}

#[tokio::test]
async fn concurrent_rate_limited_hook_only_delivers_once() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let one_per_window = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "policy": { "rate_limit": { "window_seconds": 60, "max_requests": 1 } }
            }),
        )
        .await;
    assert_eq!(one_per_window.status, StatusCode::OK);
    let one_per_window_url = one_per_window.body["invocation_url"]
        .as_str()
        .expect("url")
        .to_owned();
    let barrier = Arc::new(Barrier::new(3));

    let first_state = harness.state.clone();
    let first_url = one_per_window_url.clone();
    let first_barrier = barrier.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        request_with_state(first_state, "POST", &first_url, Body::from("{}")).await
    });

    let second_state = harness.state.clone();
    let second_barrier = barrier.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        request_with_state(second_state, "POST", &one_per_window_url, Body::from("{}")).await
    });

    barrier.wait().await;
    let first = first.await.expect("first invocation task");
    let second = second.await.expect("second invocation task");
    let success_count = [&first, &second]
        .into_iter()
        .filter(|response| response.status == StatusCode::OK)
        .count();
    let limited: Vec<_> = [first, second]
        .into_iter()
        .filter(|response| response.status == StatusCode::TOO_MANY_REQUESTS)
        .collect();

    assert_eq!(success_count, 1);
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].body["error"]["code"], "hook_rate_limited");
    assert_eq!(wait_for_deliveries(&harness, 1).await.len(), 1);
}

#[tokio::test]
async fn default_rate_limit_is_applied_to_new_hooks() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let create = harness.json("POST", "/v1/hooks", json!({})).await;

    assert_eq!(create.status, StatusCode::OK);
    assert_eq!(
        create.body["hook"]["policy"]["rate_limit"]["window_seconds"],
        3600
    );
    assert_eq!(
        create.body["hook"]["policy"]["rate_limit"]["max_requests"],
        2
    );
}

#[tokio::test]
async fn production_mode_rejects_per_hook_high_rate_exceptions() {
    let harness = Harness::new_with_config(|config| {
        config
            .with_production_mode(true)
            .with_open_self_registration_enabled(true)
    })
    .await;
    register_installation(&harness, "recipient").await;
    let high_rate = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "installation_id": "device-1",
                "policy": { "ttl_seconds": 3600, "rate_limit": { "window_seconds": 60, "max_requests": 30 } }
            }),
        )
        .await;
    assert_eq!(high_rate.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        high_rate.body["error"]["code"],
        "production_hook_rate_limit_too_permissive"
    );
    let high_max_only = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "installation_id": "device-1",
                "policy": { "ttl_seconds": 3600, "rate_limit": { "window_seconds": 3600, "max_requests": 3 } }
            }),
        )
        .await;
    assert_eq!(high_max_only.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        high_max_only.body["error"]["code"],
        "production_hook_rate_limit_too_permissive"
    );
    let short_window_only = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "installation_id": "device-1",
                "policy": { "ttl_seconds": 3600, "rate_limit": { "window_seconds": 3599, "max_requests": 2 } }
            }),
        )
        .await;
    assert_eq!(short_window_only.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        short_window_only.body["error"]["code"],
        "production_hook_rate_limit_too_permissive"
    );
    let stricter = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "installation_id": "device-1",
                "policy": { "ttl_seconds": 3600, "rate_limit": { "window_seconds": 7200, "max_requests": 1 } }
            }),
        )
        .await;
    assert_eq!(stricter.status, StatusCode::OK);

    let default_rate = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({"installation_id": "device-1", "policy": {"ttl_seconds": 3600}}),
        )
        .await;
    assert_eq!(default_rate.status, StatusCode::OK);
    assert_eq!(
        default_rate.body["hook"]["policy"]["rate_limit"]["window_seconds"],
        3600
    );
    assert_eq!(
        default_rate.body["hook"]["policy"]["rate_limit"]["max_requests"],
        2
    );

    let invocation_url = default_rate.body["invocation_url"].as_str().expect("url");
    let hook_id = default_rate.body["hook"]["hook_id"]
        .as_str()
        .expect("hook_id");
    sqlx::query(
        "UPDATE notification_hooks
         SET rate_limit_window_seconds = 60,
             rate_limit_max_requests = 30
         WHERE hook_id = $1",
    )
    .bind(hook_id)
    .execute(harness.state.database().pool())
    .await
    .expect("simulate pre-existing high-rate hook");
    assert_eq!(
        harness
            .json("POST", invocation_url, json!({"idempotency_key": "first"}))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        harness
            .json("POST", invocation_url, json!({"idempotency_key": "second"}))
            .await
            .status,
        StatusCode::OK
    );
    let capped = harness
        .json("POST", invocation_url, json!({"idempotency_key": "third"}))
        .await;
    assert_eq!(capped.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(capped.body["error"]["code"], "hook_rate_limited");
}

#[tokio::test]
async fn hooks_require_owned_installation_and_finite_ttl_in_every_mode() {
    let production = Harness::new_with_config(|config| {
        config
            .with_production_mode(true)
            .with_open_self_registration_enabled(true)
    })
    .await;
    assert_hook_target_and_ttl_are_required(&production).await;
    drop(production);

    let local = Harness::new().await;
    assert_hook_target_and_ttl_are_required(&local).await;
}

async fn assert_hook_target_and_ttl_are_required(harness: &Harness) {
    register_installation(harness, "recipient").await;

    let missing_installation = harness
        .exact_json(
            "POST",
            "/v1/hooks",
            json!({"policy": {"ttl_seconds": 3600}}),
        )
        .await;
    assert_eq!(missing_installation.status, StatusCode::BAD_REQUEST);
    assert_eq!(missing_installation.body["error"]["code"], "invalid_json");

    let missing_ttl = harness
        .exact_json("POST", "/v1/hooks", json!({"installation_id": "device-1"}))
        .await;
    assert_eq!(missing_ttl.status, StatusCode::BAD_REQUEST);
    assert_eq!(missing_ttl.body["error"]["code"], "ttl_seconds_required");
}

#[tokio::test]
async fn auth_and_payload_validation_reject_bad_requests() {
    let harness = Harness::new().await;
    let missing_auth = harness.unsigned_json("POST", "/v1/hooks", json!({})).await;
    assert_eq!(missing_auth.status, StatusCode::UNAUTHORIZED);
    assert_eq!(missing_auth.body["error"]["code"], "invalid_nostr_auth");

    let bad_auth = harness
        .request_with_header(
            "POST",
            "/v1/hooks",
            Body::from("{}"),
            "authorization",
            "Nostr not-base64",
        )
        .await;
    assert_eq!(bad_auth.status, StatusCode::UNAUTHORIZED);

    let too_large = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "label": "x".repeat(200)
            }),
        )
        .await;
    assert_eq!(too_large.status, StatusCode::BAD_REQUEST);
    assert_eq!(too_large.body["error"]["code"], "field_too_long");

    let invalid_json = harness
        .request("POST", "/registrations", Body::from("{"))
        .await;
    assert_eq!(invalid_json.status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid_json.body["error"]["code"], "invalid_json");

    let bad_rate_limit = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "policy": { "rate_limit": { "window_seconds": 0 } }
            }),
        )
        .await;
    assert_eq!(bad_rate_limit.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        bad_rate_limit.body["error"]["code"],
        "rate_limit_window_seconds_out_of_range"
    );

    let bad_rate_limit_max = harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "policy": { "rate_limit": { "max_requests": 0 } }
            }),
        )
        .await;
    assert_eq!(bad_rate_limit_max.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        bad_rate_limit_max.body["error"]["code"],
        "rate_limit_max_requests_out_of_range"
    );

    let too_many_data_keys = harness
        .json(
            "POST",
            "/hooks/not-found-id/not-found-secret",
            json!({
                "data": (0..33).map(|i| (format!("key-{i}"), json!(i))).collect::<serde_json::Map<String, Value>>()
            }),
        )
        .await;
    assert_eq!(too_many_data_keys.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        too_many_data_keys.body["error"]["code"],
        "data_too_many_keys"
    );

    let invalid_data_key = harness
        .json(
            "POST",
            "/hooks/not-found-id/not-found-secret",
            json!({ "data": { "": "value" } }),
        )
        .await;
    assert_eq!(invalid_data_key.status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid_data_key.body["error"]["code"], "data_key_invalid");

    let too_long_data_key = harness
        .json(
            "POST",
            "/hooks/not-found-id/not-found-secret",
            json!({ "data": { "x".repeat(65): "value" } }),
        )
        .await;
    assert_eq!(too_long_data_key.status, StatusCode::BAD_REQUEST);
    assert_eq!(too_long_data_key.body["error"]["code"], "data_key_invalid");
}

#[tokio::test]
async fn nostr_auth_binds_method_url_query_body_timestamp_and_replay() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;

    let wrong_method = nostr_authorization_custom(
        &harness.recipient_keys,
        harness.state.config().public_base_url(),
        "GET",
        "/v1/hooks",
        b"{}",
        Some(Timestamp::now()),
    );
    let response = harness
        .request_with_header(
            "POST",
            "/v1/hooks",
            Body::from("{}"),
            "authorization",
            &wrong_method,
        )
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    let wrong_query = nostr_authorization_custom(
        &harness.recipient_keys,
        harness.state.config().public_base_url(),
        "GET",
        "/v1/hooks",
        b"",
        Some(Timestamp::now()),
    );
    let response = harness
        .request_with_header(
            "GET",
            "/v1/hooks?cursor=1",
            Body::empty(),
            "authorization",
            &wrong_query,
        )
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    let wrong_body = nostr_authorization_custom(
        &harness.recipient_keys,
        harness.state.config().public_base_url(),
        "POST",
        "/v1/hooks",
        br#"{"label":"signed"}"#,
        Some(Timestamp::now()),
    );
    let response = harness
        .request_with_header(
            "POST",
            "/v1/hooks",
            Body::from(r#"{"label":"sent"}"#),
            "authorization",
            &wrong_body,
        )
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    let disable_uri = "/registrations/device-1/disable?reason=invalid_token";
    let missing_payload = nostr_authorization_with_payload(
        &harness.recipient_keys,
        harness.state.config().public_base_url(),
        "POST",
        disable_uri,
        b"",
        None,
        PayloadTag::Omit,
    );
    let response = harness
        .request_with_header(
            "POST",
            disable_uri,
            Body::empty(),
            "authorization",
            &missing_payload,
        )
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    let wrong_empty_payload = nostr_authorization_with_payload(
        &harness.recipient_keys,
        harness.state.config().public_base_url(),
        "POST",
        disable_uri,
        b"",
        None,
        PayloadTag::Value("not-the-empty-body-hash"),
    );
    let response = harness
        .request_with_header(
            "POST",
            disable_uri,
            Body::empty(),
            "authorization",
            &wrong_empty_payload,
        )
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    let stale = nostr_authorization_custom(
        &harness.recipient_keys,
        harness.state.config().public_base_url(),
        "POST",
        "/v1/hooks",
        b"{}",
        Some(Timestamp::from_secs((crate_unix_timestamp() - 120) as u64)),
    );
    let response = harness
        .request_with_header(
            "POST",
            "/v1/hooks",
            Body::from("{}"),
            "authorization",
            &stale,
        )
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    let future = nostr_authorization_custom(
        &harness.recipient_keys,
        harness.state.config().public_base_url(),
        "POST",
        "/v1/hooks",
        b"{}",
        Some(Timestamp::from_secs((crate_unix_timestamp() + 60) as u64)),
    );
    let response = harness
        .request_with_header(
            "POST",
            "/v1/hooks",
            Body::from("{}"),
            "authorization",
            &future,
        )
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    let replay_body = br#"{"installation_id":"device-1","policy":{"ttl_seconds":3600}}"#;
    let replayed = nostr_authorization_custom(
        &harness.recipient_keys,
        harness.state.config().public_base_url(),
        "POST",
        "/v1/hooks",
        replay_body,
        Some(Timestamp::now()),
    );
    let first = harness
        .request_with_header(
            "POST",
            "/v1/hooks",
            Body::from(replay_body.as_slice()),
            "authorization",
            &replayed,
        )
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let second = harness
        .request_with_header(
            "POST",
            "/v1/hooks",
            Body::from(replay_body.as_slice()),
            "authorization",
            &replayed,
        )
        .await;
    assert_eq!(second.status, StatusCode::UNAUTHORIZED);
    assert_eq!(second.body["error"]["code"], "auth_replay");
}

#[tokio::test]
async fn recipient_id_in_body_is_rejected_as_spoofing_attempt() {
    let harness = Harness::new().await;
    let response = harness
        .json("POST", "/v1/hooks", json!({"recipient_id": "spoofed"}))
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.body["error"]["code"], "invalid_json");
}

#[tokio::test]
async fn legacy_notification_hook_derives_recipient_from_nostr_auth() {
    let harness = Harness::new().await;
    let accepted = harness
        .json(
            "POST",
            "/hooks/notification",
            json!({
                "notification_id": "legacy-1",
                "kind": "legacy",
                "title": "Legacy",
                "body": "direct notification",
                "data": { "ok": true }
            }),
        )
        .await;
    assert_eq!(accepted.status, StatusCode::OK);
    assert_eq!(accepted.body["accepted"], true);
    assert_eq!(
        accepted.body["notification"]["recipient_id"],
        harness.recipient_keys.public_key().to_string()
    );

    let spoof = harness
        .json(
            "POST",
            "/hooks/notification",
            json!({
                "recipient_id": "attacker",
                "app_id": "attacker-app",
                "notification_id": "legacy-2",
                "kind": "legacy"
            }),
        )
        .await;
    assert_eq!(spoof.status, StatusCode::BAD_REQUEST);
    assert_eq!(spoof.body["error"]["code"], "invalid_json");
}

#[tokio::test]
async fn hook_revoke_rejects_cross_recipient_action() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let create = create_hook(&harness, None, None).await;
    let hook_id = create.body["hook"]["hook_id"].as_str().expect("hook id");

    let cross_recipient = harness
        .empty_with_keys(&Keys::generate(), "DELETE", &format!("/v1/hooks/{hook_id}"))
        .await;
    assert_eq!(cross_recipient.status, StatusCode::NOT_FOUND);
    assert_eq!(cross_recipient.body["error"]["code"], "hook_not_found");

    let invoke = harness
        .json(
            "POST",
            create.body["invocation_url"].as_str().expect("url"),
            json!({ "idempotency_key": "still-owned" }),
        )
        .await;
    assert_eq!(invoke.status, StatusCode::OK);
}

#[tokio::test]
async fn default_active_installation_limit_rejects_ninth_registration() {
    let harness = Harness::new().await;
    for i in 0..8 {
        let response = harness
            .json(
                "POST",
                "/registrations",
                json!({
                    "installation_id": format!("device-{i}"),
                    "fcm_token": format!("fcm-token-{i}"),
                }),
            )
            .await;
        assert_eq!(response.status, StatusCode::OK);
    }

    let response = harness
        .json(
            "POST",
            "/registrations",
            json!({
                "installation_id": "device-8",
                "fcm_token": "fcm-token-8",
            }),
        )
        .await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.body["error"]["code"],
        "max_active_installations_exceeded"
    );
}

#[tokio::test]
async fn hook_invocation_source_limit_can_be_overridden() {
    let limits = RateLimitConfig {
        hook_invocations_per_source_prefix: 1,
        hook_invocations_per_hook: 100,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;
    register_installation(&harness, "recipient").await;
    let create = create_hook(&harness, None, None).await;
    let url = create.body["invocation_url"].as_str().expect("url");

    let first = harness
        .json("POST", url, json!({ "idempotency_key": "one" }))
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let second = harness
        .json("POST", url, json!({ "idempotency_key": "two" }))
        .await;
    assert_eq!(second.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(second.body["error"]["code"], "source_rate_limited");
}

#[tokio::test]
async fn hook_invocation_per_hook_limit_can_be_overridden() {
    let limits = RateLimitConfig {
        hook_invocations_per_source_prefix: 100,
        hook_invocations_per_hook: 1,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;
    register_installation(&harness, "recipient").await;
    let create = create_hook(&harness, None, None).await;
    let url = create.body["invocation_url"].as_str().expect("url");

    assert_eq!(
        harness
            .json("POST", url, json!({ "idempotency_key": "one" }))
            .await
            .status,
        StatusCode::OK
    );
    let second = harness
        .json("POST", url, json!({ "idempotency_key": "two" }))
        .await;
    assert_eq!(second.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(second.body["error"]["code"], "hook_rate_limited");
}

#[tokio::test]
async fn hook_creation_rate_limit_and_active_cap_are_enforced() {
    let limits = RateLimitConfig {
        hook_creations_per_recipient: 1,
        max_active_hooks_per_recipient: 10,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;
    assert_eq!(
        create_hook(&harness, None, None).await.status,
        StatusCode::OK
    );
    let limited = create_hook(&harness, None, None).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.body["error"]["code"], "hook_creation_rate_limited");
    drop(harness);

    let limits = RateLimitConfig {
        hook_creations_per_recipient: 100,
        max_active_hooks_per_recipient: 1,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;
    assert_eq!(
        create_hook(&harness, None, None).await.status,
        StatusCode::OK
    );
    let capped = create_hook(&harness, None, None).await;
    assert_eq!(capped.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(capped.body["error"]["code"], "max_active_hooks_exceeded");
}

#[tokio::test]
async fn registration_write_limit_includes_unregister() {
    let limits = RateLimitConfig {
        registration_changes_per_recipient_source: 1,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;

    let limited = harness.empty("DELETE", "/registrations/device-1").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.body["error"]["code"], "registration_rate_limited");
}

#[tokio::test]
async fn registration_source_limit_cannot_be_evaded_with_new_recipient_keys() {
    let limits = RateLimitConfig {
        registration_changes_per_recipient_source: 100,
        registration_changes_per_source_prefix: 1,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;

    let body = json!({
        "installation_id": "other-device",
        "fcm_token": "other-token"
    })
    .to_string();
    let response = request_with_state_signed(
        harness.state.clone(),
        &Keys::generate(),
        "POST",
        "/registrations",
        Body::from(body),
    )
    .await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.body["error"]["code"],
        "registration_source_rate_limited"
    );
}

#[tokio::test]
async fn authentication_source_budget_precedes_replay_cache_and_preserves_other_sources() {
    let limits = RateLimitConfig {
        auth_events_per_source_prefix: 1,
        auth_event_window_seconds: 60,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;

    let first = request_with_state_signed_from_peer(
        harness.state.clone(),
        &harness.recipient_keys,
        "GET",
        "/v1/hooks",
        Body::empty(),
        "198.51.100.10:1234".parse().unwrap(),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK);

    let throttled = request_with_state_signed_from_peer(
        harness.state.clone(),
        &harness.recipient_keys,
        "GET",
        "/v1/hooks",
        Body::empty(),
        "198.51.100.10:5678".parse().unwrap(),
    )
    .await;
    assert_eq!(throttled.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(throttled.body["error"]["code"], "auth_source_rate_limited");

    let other_source = request_with_state_signed_from_peer(
        harness.state.clone(),
        &harness.recipient_keys,
        "GET",
        "/v1/hooks",
        Body::empty(),
        "203.0.113.20:1234".parse().unwrap(),
    )
    .await;
    assert_eq!(other_source.status, StatusCode::OK);
}

#[tokio::test]
async fn global_installation_capacity_fails_closed() {
    let limits = RateLimitConfig {
        max_active_installations_global: 1,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;

    let response = harness
        .json(
            "POST",
            "/registrations",
            json!({"installation_id": "device-2", "fcm_token": "fcm-token-2"}),
        )
        .await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.body["error"]["code"],
        "global_installation_capacity_exceeded"
    );
}

#[tokio::test]
async fn global_hook_capacity_fails_closed() {
    let limits = RateLimitConfig {
        hook_creations_per_recipient: 100,
        max_active_hooks_global: 1,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;
    assert_eq!(
        create_hook(&harness, None, None).await.status,
        StatusCode::OK
    );

    let response = create_hook(&harness, None, None).await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.body["error"]["code"],
        "global_hook_capacity_exceeded"
    );
}

#[tokio::test]
async fn admission_reclaims_terminal_rows_without_restart_and_enforces_physical_ceilings() {
    let limits = RateLimitConfig {
        hook_creations_per_recipient: 100,
        registration_changes_per_recipient_source: 100,
        registration_changes_per_source_prefix: 100,
        max_active_hooks_per_recipient: 100,
        max_active_installations_per_recipient: 100,
        max_active_hooks_global: 100,
        max_active_installations_global: 100,
        max_hook_rows_global: 1,
        // One reclaimed owner tombstone plus the replacement registration and owner.
        max_registration_rows_global: 3,
        admission_gc_batch_size: 1,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;
    sqlx::query("UPDATE push_registrations SET last_seen_at = 0")
        .execute(harness.state.database().pool())
        .await
        .unwrap();
    let reclaimed_registration = harness
        .json(
            "POST",
            "/registrations",
            json!({"installation_id": "device-2", "fcm_token": "fcm-token-2"}),
        )
        .await;
    assert_eq!(reclaimed_registration.status, StatusCode::OK);
    let registration_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_registrations")
        .fetch_one(harness.state.database().pool())
        .await
        .unwrap();
    assert_eq!(registration_rows, 1);

    let registration_full = harness
        .json(
            "POST",
            "/registrations",
            json!({"installation_id": "device-3", "fcm_token": "fcm-token-3"}),
        )
        .await;
    assert_eq!(registration_full.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        registration_full.body["error"]["code"],
        "registration_row_capacity_exceeded"
    );

    let first_hook = create_targeted_hook(&harness, "device-2").await;
    assert_eq!(first_hook.status, StatusCode::OK);
    sqlx::query("UPDATE notification_hooks SET expires_at = 0")
        .execute(harness.state.database().pool())
        .await
        .unwrap();
    let reclaimed_hook = create_targeted_hook(&harness, "device-2").await;
    assert_eq!(reclaimed_hook.status, StatusCode::OK);
    assert_eq!(hook_row_count(&harness).await, 1);

    let hook_full = create_targeted_hook(&harness, "device-2").await;
    assert_eq!(hook_full.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        hook_full.body["error"]["code"],
        "hook_row_capacity_exceeded"
    );
}

#[tokio::test]
async fn terminal_keyed_hooks_release_hook_capacity_after_events_are_gone() {
    let limits = RateLimitConfig {
        hook_creations_per_recipient: 100,
        max_active_hooks_per_recipient: 100,
        max_active_hooks_global: 100,
        max_hook_rows_global: 1,
        admission_gc_batch_size: 1,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;

    let revoked = create_targeted_hook(&harness, "device-1").await;
    assert_eq!(revoked.status, StatusCode::OK);
    let revoked_url = revoked.body["invocation_url"].as_str().expect("url");
    assert_eq!(
        harness
            .json(
                "POST",
                revoked_url,
                json!({"idempotency_key": "revoked-key"})
            )
            .await
            .status,
        StatusCode::OK
    );
    delete_sensitive_delivery_state(&harness).await;
    let revoked_id = revoked.body["hook"]["hook_id"].as_str().expect("hook id");
    assert_eq!(
        harness
            .empty("DELETE", &format!("/v1/hooks/{revoked_id}"))
            .await
            .status,
        StatusCode::OK
    );

    let expired = create_targeted_hook(&harness, "device-1").await;
    assert_eq!(expired.status, StatusCode::OK);
    assert_eq!(hook_row_count(&harness).await, 1);
    assert_eq!(idempotency_row_count(&harness).await, 0);
    let expired_url = expired.body["invocation_url"].as_str().expect("url");
    assert_eq!(
        harness
            .json(
                "POST",
                expired_url,
                json!({"idempotency_key": "expired-key"})
            )
            .await
            .status,
        StatusCode::OK
    );
    delete_sensitive_delivery_state(&harness).await;
    sqlx::query("UPDATE notification_hooks SET expires_at = 0")
        .execute(harness.state.database().pool())
        .await
        .expect("expire keyed hook");

    let replacement = create_targeted_hook(&harness, "device-1").await;
    assert_eq!(replacement.status, StatusCode::OK);
    assert_eq!(hook_row_count(&harness).await, 1);
    assert_eq!(idempotency_row_count(&harness).await, 0);
}

#[tokio::test]
async fn undeclared_attestation_shape_is_rejected_without_persistence() {
    let harness = Harness::new().await;
    let response = harness
        .json(
            "POST",
            "/registrations",
            json!({
                "installation_id": "device",
                "fcm_token": "token",
                "attestation": {"version": 1, "evidence": {"proof": "opaque"}}
            }),
        )
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.body["error"]["code"], "invalid_json");
}

#[tokio::test]
async fn global_outbox_backlog_cap_returns_unavailable() {
    let limits = RateLimitConfig {
        max_global_outbox_backlog: 1,
        hook_invocations_per_source_prefix: 100,
        hook_invocations_per_hook: 100,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;
    let create = create_hook(&harness, None, None).await;
    seed_future_pending_outbox(&harness, &create).await;
    let url = create.body["invocation_url"].as_str().expect("url");

    let response = harness
        .json("POST", url, json!({ "idempotency_key": "one" }))
        .await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.body["error"]["code"], "outbox_backlog_full");
}

#[tokio::test]
async fn recipient_outbox_backlog_cap_returns_too_many_requests() {
    let limits = RateLimitConfig {
        max_recipient_outbox_backlog: 1,
        hook_invocations_per_source_prefix: 100,
        hook_invocations_per_hook: 100,
        ..RateLimitConfig::default()
    };
    let harness = Harness::new_with_limits(limits).await;
    register_installation(&harness, "recipient").await;
    let create = create_hook(&harness, None, None).await;
    seed_future_pending_outbox(&harness, &create).await;
    let response = harness
        .json(
            "POST",
            create.body["invocation_url"].as_str().expect("url"),
            json!({ "idempotency_key": "one" }),
        )
        .await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.body["error"]["code"],
        "recipient_outbox_backlog_full"
    );
}

#[tokio::test]
async fn readiness_and_registration_management_are_available() {
    let harness = Harness::new().await;
    let ready = harness
        .operator_text_with_header("GET", "/ready", None)
        .await;
    assert_eq!(ready.status, StatusCode::OK);
    let body: Value = serde_json::from_str(&ready.text).expect("readiness json");
    assert_eq!(body["ok"], true);
    assert_eq!(body["check"], "readiness");
    assert_eq!(body["provider_mode"], "noop");
    assert_eq!(body["database_ready"], true);
    assert_eq!(body["outbox_worker_running"], true);
    assert_eq!(body["outbox"]["pending"], 0);

    register_installation(&harness, "recipient").await;
    let create = create_hook(&harness, None, None).await;
    let invoke = harness
        .json(
            "POST",
            create.body["invocation_url"].as_str().expect("url"),
            json!({}),
        )
        .await;
    assert_eq!(invoke.status, StatusCode::OK);
    assert_eq!(invoke.body["delivery_attempts"], 1);

    let disabled = harness
        .empty(
            "POST",
            "/registrations/device-1/disable?reason=invalid_token",
        )
        .await;
    assert_eq!(disabled.status, StatusCode::OK);
    assert_eq!(disabled.body["disabled"], true);

    let invoke = harness
        .json(
            "POST",
            create.body["invocation_url"].as_str().expect("url"),
            json!({ "idempotency_key": "target-refresh" }),
        )
        .await;
    assert_eq!(invoke.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        invoke.body["error"]["code"],
        "target_installation_unavailable"
    );

    register_installation(&harness, "recipient").await;
    let retried = harness
        .json(
            "POST",
            create.body["invocation_url"].as_str().expect("url"),
            json!({ "idempotency_key": "target-refresh" }),
        )
        .await;
    assert_eq!(retried.status, StatusCode::OK);
    assert_eq!(retried.body["delivery_attempts"], 1);

    register_installation(&harness, "recipient").await;
    let deleted = harness.empty("DELETE", "/registrations/device-1").await;
    assert_eq!(deleted.status, StatusCode::OK);
    assert_eq!(deleted.body["unregistered"], true);
}

#[tokio::test]
async fn public_listener_hides_operator_endpoints_by_default() {
    let harness = Harness::new().await;

    let live = harness.empty("GET", "/live").await;
    assert_eq!(live.status, StatusCode::OK);

    let ready = harness.empty("GET", "/ready").await;
    assert_eq!(ready.status, StatusCode::NOT_FOUND);

    let metrics = harness.text("GET", "/metrics").await;
    assert_eq!(metrics.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn public_metrics_requires_explicit_enablement() {
    let harness = Harness::new_with_config(|config| config.with_public_metrics_enabled(true)).await;

    let metrics = harness.text("GET", "/metrics").await;
    assert_eq!(metrics.status, StatusCode::OK);
    assert!(metrics.text.contains("push_gateway_http_requests_total"));
    assert!(
        metrics
            .text
            .contains("push_gateway_hook_rows{state=\"total\"}")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_registration_rows{state=\"total\"}")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_registration_rows{state=\"token_owners\"}")
    );

    let ready = harness.empty("GET", "/ready").await;
    assert_eq!(ready.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn operator_routes_require_configured_bearer_token() {
    let harness = Harness::new_with_config(|config| {
        config
            .with_operator_bind(Some("127.0.0.1:9100".parse().expect("operator bind")))
            .with_operator_token(OperatorToken::new("operator-secret"))
    })
    .await;

    let public_ready = harness.empty("GET", "/ready").await;
    assert_eq!(public_ready.status, StatusCode::NOT_FOUND);
    let public_metrics = harness.text("GET", "/metrics").await;
    assert_eq!(public_metrics.status, StatusCode::NOT_FOUND);

    for header in [None, Some(("authorization", "Bearer wrong"))] {
        let metrics = harness
            .operator_text_with_header("GET", "/metrics", header)
            .await;
        assert_eq!(metrics.status, StatusCode::UNAUTHORIZED);
    }

    let metrics = harness
        .operator_text_with_header(
            "GET",
            "/metrics",
            Some(("authorization", "Bearer operator-secret")),
        )
        .await;
    assert_eq!(metrics.status, StatusCode::OK);
    assert!(metrics.text.contains("push_gateway_http_requests_total"));

    let ready = harness
        .operator_text_with_header(
            "GET",
            "/ready",
            Some(("authorization", "Bearer operator-secret")),
        )
        .await;
    assert_eq!(ready.status, StatusCode::OK);
}

#[tokio::test]
async fn token_only_operator_routes_are_protected_on_public_listener() {
    let harness = Harness::new_with_config(|config| {
        config.with_operator_token(OperatorToken::new("operator-secret"))
    })
    .await;

    let missing = harness.text("GET", "/metrics").await;
    assert_eq!(missing.status, StatusCode::UNAUTHORIZED);
    let target_path = format!(
        "/v1/telemetry/fmans/{}/seats/{}/metrics",
        "aa".repeat(32),
        "bb".repeat(32)
    );
    let missing_target = harness.text("GET", &target_path).await;
    assert_eq!(missing_target.status, StatusCode::UNAUTHORIZED);

    let wrong = harness
        .public_text_with_header("GET", "/ready", Some(("authorization", "Bearer wrong")))
        .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);

    let metrics = harness
        .public_text_with_header(
            "GET",
            "/metrics",
            Some(("authorization", "Bearer operator-secret")),
        )
        .await;
    assert_eq!(metrics.status, StatusCode::OK);
    assert!(metrics.text.contains("push_gateway_http_requests_total"));

    let target = harness
        .public_text_with_header(
            "GET",
            &target_path,
            Some(("authorization", "Bearer operator-secret")),
        )
        .await;
    assert_eq!(target.status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn metrics_and_request_id_are_available_without_token_values() {
    let harness = Harness::new().await;
    register_installation(&harness, "recipient").await;
    let create = create_hook(&harness, None, None).await;
    let invocation_url = create.body["invocation_url"].as_str().expect("url");
    assert!(invocation_url.starts_with("http://127.0.0.1:3000/hooks/"));

    let response = harness
        .request_with_header(
            "POST",
            invocation_url,
            Body::from("{}"),
            "x-request-id",
            "test-request-1",
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.request_id.is_some());
    assert_ne!(response.request_id.as_deref(), Some("test-request-1"));
    let hook_secret = invocation_url
        .rsplit('/')
        .next()
        .expect("token path segment");
    let token_response = harness
        .request_with_header("GET", "/live", Body::empty(), "x-request-id", hook_secret)
        .await;
    assert_eq!(token_response.status, StatusCode::OK);
    assert_ne!(token_response.request_id.as_deref(), Some(hook_secret));

    let metrics = harness
        .operator_text_with_header("GET", "/metrics", None)
        .await;
    assert_eq!(metrics.status, StatusCode::OK);
    assert!(metrics.text.contains("push_gateway_http_requests_total"));
    assert!(
        metrics
            .text
            .contains("push_gateway_provider_mode_info{mode=\"noop\"} 1")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_outbox_rows{status=\"pending\"}")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_outbox_claim_queries_total")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_metrics_scrape_db_error 0")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_outbox_oldest_due_age_seconds")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_outbox_oldest_pending_age_seconds")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_outbox_retrying_oldest_age_seconds")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_outbox_dead_letter_rows")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_provider_outcomes_total{reason_class=\"auth\"}")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_invalid_token_cleanup_failures_total")
    );
    assert!(
        metrics
            .text
            .contains("push_gateway_rate_limit_rejections_total")
    );
    assert!(!metrics.text.contains(hook_secret));
    assert!(!metrics.text.contains(invocation_url));
}

#[tokio::test]
async fn metrics_returns_explicit_error_when_outbox_query_fails() {
    let harness = Harness::new().await;
    sqlx::query("DROP TABLE delivery_outbox")
        .execute(harness.state.database().pool())
        .await
        .expect("drop outbox table");

    let metrics = harness
        .operator_text_with_header("GET", "/metrics", None)
        .await;
    assert_eq!(metrics.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        metrics
            .text
            .contains("push_gateway_metrics_scrape_db_error 1")
    );
    assert!(
        !metrics
            .text
            .contains("push_gateway_outbox_rows{status=\"pending\"} 0"),
        "db query failures must not be silently reported as zero queue depth"
    );
}

#[tokio::test]
async fn app_state_debug_redacts_runtime_config_secrets() {
    let harness = Harness::new().await;
    let debug = format!("{:?}", harness.state);

    assert!(debug.contains("AppState"));
    assert!(debug.contains("<config>"));
    assert!(debug.contains("<database>"));
    assert!(!debug.contains("test-app"));
    assert!(!debug.contains("push.sqlite"));
}

async fn wait_for_deliveries(
    harness: &Harness,
    count: usize,
) -> Vec<fedi_decentralized_push_gateway::FakeDelivery> {
    for _ in 0..50 {
        let deliveries = harness.provider.deliveries();
        if deliveries.len() >= count {
            return deliveries;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    harness.provider.deliveries()
}

async fn create_hook(
    harness: &Harness,
    expires_in_seconds: Option<i64>,
    max_uses: Option<i64>,
) -> Response {
    // Most hook MVP tests are not about the production-conservative default; this
    // helper sets an explicit fast test/dev policy. Tests for default policy should
    // inline the create request.
    let mut body = json!({
        "installation_id": "device-1",
        "policy": {
            "ttl_seconds": 3600,
            "rate_limit": {
                "window_seconds": TEST_RATE_LIMIT_WINDOW_SECONDS,
                "max_requests": TEST_RATE_LIMIT_MAX_REQUESTS
            }
        }
    });
    if let Some(expires_in_seconds) = expires_in_seconds {
        body["policy"]["ttl_seconds"] = json!(expires_in_seconds);
    }
    if let Some(max_uses) = max_uses {
        body["policy"]["max_uses"] = json!(max_uses);
    }
    harness.json("POST", "/v1/hooks", body).await
}

async fn create_targeted_hook(harness: &Harness, installation_id: &str) -> Response {
    harness
        .json(
            "POST",
            "/v1/hooks",
            json!({
                "installation_id": installation_id,
                "policy": {
                    "ttl_seconds": 3600,
                    "rate_limit": {
                        "window_seconds": TEST_RATE_LIMIT_WINDOW_SECONDS,
                        "max_requests": TEST_RATE_LIMIT_MAX_REQUESTS
                    }
                }
            }),
        )
        .await
}

async fn hook_row_count(harness: &Harness) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM notification_hooks")
        .fetch_one(harness.state.database().pool())
        .await
        .expect("count hook rows")
}

async fn idempotency_row_count(harness: &Harness) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM hook_idempotency_tombstones")
        .fetch_one(harness.state.database().pool())
        .await
        .expect("count idempotency rows")
}

async fn delete_sensitive_delivery_state(harness: &Harness) {
    sqlx::query("DELETE FROM delivery_outbox")
        .execute(harness.state.database().pool())
        .await
        .expect("delete outbox rows");
    sqlx::query("DELETE FROM notification_events")
        .execute(harness.state.database().pool())
        .await
        .expect("delete event rows");
}

async fn assert_exact_pair_owner(harness: &Harness, expected_recipient: &str) {
    let registrations: Vec<(String, String, String)> =
        sqlx::query_as("SELECT recipient_id, installation_id, fcm_token FROM push_registrations")
            .fetch_all(harness.state.database().pool())
            .await
            .expect("read registration route");
    assert_eq!(
        registrations,
        vec![(
            expected_recipient.to_owned(),
            "device-1".to_owned(),
            "fcm-token-1".to_owned(),
        )]
    );
    let owners: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT recipient_id, installation_id, fcm_token
         FROM push_registration_token_owners",
    )
    .fetch_all(harness.state.database().pool())
    .await
    .expect("read token owner");
    assert_eq!(
        owners,
        vec![(
            expected_recipient.to_owned(),
            "device-1".to_owned(),
            "fcm-token-1".to_owned(),
        )]
    );
}

async fn seed_future_pending_outbox(harness: &Harness, hook: &Response) {
    let hook_id = hook.body["hook"]["hook_id"].as_str().expect("hook id");
    let recipient_id = harness.recipient_keys.public_key().to_string();
    sqlx::query(
        "INSERT INTO notification_events (
             event_id, hook_id, recipient_id, notification_json, target_count, created_at
         ) VALUES ('existing-event', $1, $2, '{}', 1, 4000000000)",
    )
    .bind(hook_id)
    .bind(&recipient_id)
    .execute(harness.state.database().pool())
    .await
    .expect("seed event");
    sqlx::query(
        "INSERT INTO delivery_outbox (
             outbox_id, event_id, recipient_id, installation_id, fcm_token,
             notification_json, status, attempts, next_attempt_at, created_at, updated_at
         ) VALUES (
             'existing-outbox', 'existing-event', $1, 'device-1', 'fcm-token-1',
             '{}', 'retrying', 0, 4000000000, 4000000000, 4000000000
         )",
    )
    .bind(recipient_id)
    .execute(harness.state.database().pool())
    .await
    .expect("seed future pending outbox");
}

async fn assert_unavailable_then_refreshes_once(
    harness: &Harness,
    hook: &Response,
    idempotency_key: &str,
) {
    let url = hook.body["invocation_url"].as_str().expect("hook URL");
    let unavailable = harness
        .json("POST", url, json!({"idempotency_key": idempotency_key}))
        .await;
    assert_eq!(unavailable.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        unavailable.body["error"]["code"],
        "target_installation_unavailable"
    );
    let use_count: i64 = sqlx::query_scalar("SELECT SUM(use_count) FROM notification_hooks")
        .fetch_one(harness.state.database().pool())
        .await
        .unwrap_or(0);
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_events")
        .fetch_one(harness.state.database().pool())
        .await
        .unwrap();
    assert_eq!(use_count, 0);
    assert_eq!(events, 0);

    register_installation(harness, "recipient").await;
    let accepted = harness
        .json("POST", url, json!({"idempotency_key": idempotency_key}))
        .await;
    assert_eq!(accepted.status, StatusCode::OK);
    assert_eq!(accepted.body["delivery_attempts"], 1);
    let replay = harness
        .json("POST", url, json!({"idempotency_key": idempotency_key}))
        .await;
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(replay.body["delivery_attempts"], 1);
    let use_count: i64 = sqlx::query_scalar("SELECT SUM(use_count) FROM notification_hooks")
        .fetch_one(harness.state.database().pool())
        .await
        .unwrap_or(0);
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_events")
        .fetch_one(harness.state.database().pool())
        .await
        .unwrap();
    let outbox: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delivery_outbox")
        .fetch_one(harness.state.database().pool())
        .await
        .unwrap();
    assert_eq!(use_count, 1);
    assert_eq!(events, 1);
    assert_eq!(outbox, 1);
}

async fn register_installation(harness: &Harness, _recipient_id: &str) {
    let response = harness
        .json(
            "POST",
            "/registrations",
            json!({
                "installation_id": "device-1",
                "fcm_token": "fcm-token-1",
                "platform": "android"
            }),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK);
}

struct Harness {
    _test_guard: OwnedMutexGuard<()>,
    _temp_dir: TempDir,
    state: AppState,
    provider: FakePushProvider,
    recipient_keys: Keys,
    _worker: DeliveryWorkerHandle,
}

impl Harness {
    async fn new() -> Self {
        Self::new_with_auth(Some(AppId("test-app".to_owned())), false).await
    }

    async fn new_with_auth(app_id: Option<AppId>, unsafe_allow_any_app_id_for_tests: bool) -> Self {
        Self::new_with_auth_and_limits(
            app_id,
            unsafe_allow_any_app_id_for_tests,
            RateLimitConfig::default(),
        )
        .await
    }

    async fn new_with_limits(rate_limits: RateLimitConfig) -> Self {
        Self::new_with_auth_and_limits(Some(AppId("test-app".to_owned())), false, rate_limits).await
    }

    async fn new_with_config(
        configure: impl FnOnce(PushGatewayConfig) -> PushGatewayConfig,
    ) -> Self {
        let test_guard = hook_mvp_test_mutex().lock_owned().await;
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let database_path = temp_dir.path().join("push.sqlite");
        let config = PushGatewayConfig::new(
            Some(AppId("test-app".to_owned())),
            format!("sqlite://{}?mode=rwc", database_path.display()),
            None,
        )
        .try_with_local_test_public_base_url("http://127.0.0.1:3000")
        .expect("local test public base URL");
        let config = configure(config);
        let database = Database::connect(config.database_url())
            .await
            .expect("connect database");
        let provider = FakePushProvider::default();
        let state = AppState::with_push_provider(config, database, Arc::new(provider.clone()));
        let worker = state.start_delivery_worker();

        Self {
            _test_guard: test_guard,
            _temp_dir: temp_dir,
            state,
            provider,
            recipient_keys: Keys::generate(),
            _worker: worker,
        }
    }

    async fn new_with_auth_and_limits(
        app_id: Option<AppId>,
        unsafe_allow_any_app_id_for_tests: bool,
        rate_limits: RateLimitConfig,
    ) -> Self {
        let test_guard = hook_mvp_test_mutex().lock_owned().await;
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let database_path = temp_dir.path().join("push.sqlite");
        let config = PushGatewayConfig::new(
            app_id,
            format!("sqlite://{}?mode=rwc", database_path.display()),
            None,
        )
        .with_unsafe_allow_any_app_id_for_tests(unsafe_allow_any_app_id_for_tests)
        .with_rate_limits(rate_limits)
        .try_with_local_test_public_base_url("http://127.0.0.1:3000")
        .expect("local test public base URL");
        let database = Database::connect(config.database_url())
            .await
            .expect("connect database");
        let provider = FakePushProvider::default();
        let state = AppState::with_push_provider(config, database, Arc::new(provider.clone()));
        let worker = state.start_delivery_worker();

        Self {
            _test_guard: test_guard,
            _temp_dir: temp_dir,
            state,
            provider,
            recipient_keys: Keys::generate(),
            _worker: worker,
        }
    }

    async fn empty(&self, method: &str, uri: &str) -> Response {
        self.request(method, uri, Body::empty()).await
    }

    async fn empty_with_keys(&self, keys: &Keys, method: &str, uri: &str) -> Response {
        request_with_state_signed(self.state.clone(), keys, method, uri, Body::empty()).await
    }

    async fn json(&self, method: &str, uri: &str, mut body: Value) -> Response {
        // Most tests exercise behavior after hook admission. Keep their fixtures
        // on the required targeted, finite contract; boundary tests use
        // `exact_json` when omission itself is under test.
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
        self.exact_json(method, uri, body).await
    }

    async fn exact_json(&self, method: &str, uri: &str, body: Value) -> Response {
        self.request(method, uri, Body::from(body.to_string()))
            .await
    }

    async fn unsigned_json(&self, method: &str, uri: &str, body: Value) -> Response {
        request_with_state_and_header(
            self.state.clone(),
            &self.recipient_keys,
            method,
            uri,
            Body::from(body.to_string()),
            Some(("authorization", "")),
        )
        .await
    }

    async fn request(&self, method: &str, uri: &str, body: Body) -> Response {
        request_with_state_signed(self.state.clone(), &self.recipient_keys, method, uri, body).await
    }

    async fn request_with_header(
        &self,
        method: &str,
        uri: &str,
        body: Body,
        header_name: &str,
        header_value: &str,
    ) -> Response {
        request_with_state_and_header(
            self.state.clone(),
            &self.recipient_keys,
            method,
            uri,
            body,
            Some((header_name, header_value)),
        )
        .await
    }

    async fn text(&self, method: &str, uri: &str) -> TextResponse {
        self.text_with_header(method, uri, None, true).await
    }

    async fn public_text_with_header(
        &self,
        method: &str,
        uri: &str,
        header: Option<(&str, &str)>,
    ) -> TextResponse {
        self.text_with_header(method, uri, header, true).await
    }

    async fn operator_text_with_header(
        &self,
        method: &str,
        uri: &str,
        header: Option<(&str, &str)>,
    ) -> TextResponse {
        self.text_with_header(method, uri, header, false).await
    }

    async fn text_with_header(
        &self,
        method: &str,
        uri: &str,
        header: Option<(&str, &str)>,
        public: bool,
    ) -> TextResponse {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .expect("request");
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:12345".parse::<std::net::SocketAddr>().unwrap(),
        ));
        if let Some((name, value)) = header {
            request.headers_mut().insert(
                HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                value.parse().expect("header value"),
            );
        }
        let router = if public {
            public_app(self.state.clone())
        } else {
            operator_app(self.state.clone())
        };
        let response = router.oneshot(request).await.expect("response");
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

async fn request_with_state(state: AppState, method: &str, uri: &str, body: Body) -> Response {
    request_with_state_and_header(state, &Keys::generate(), method, uri, body, None).await
}

async fn request_with_state_signed(
    state: AppState,
    recipient_keys: &Keys,
    method: &str,
    uri: &str,
    body: Body,
) -> Response {
    request_with_state_and_header(state, recipient_keys, method, uri, body, None).await
}

async fn request_with_state_signed_from_peer(
    state: AppState,
    recipient_keys: &Keys,
    method: &str,
    uri: &str,
    body: Body,
    peer: std::net::SocketAddr,
) -> Response {
    request_with_state_and_header_from_peer(state, recipient_keys, method, uri, body, None, peer)
        .await
}

async fn request_with_state_and_header(
    state: AppState,
    recipient_keys: &Keys,
    method: &str,
    uri: &str,
    body: Body,
    header: Option<(&str, &str)>,
) -> Response {
    request_with_state_and_header_from_peer(
        state,
        recipient_keys,
        method,
        uri,
        body,
        header,
        "127.0.0.1:12345".parse().unwrap(),
    )
    .await
}

async fn request_with_state_and_header_from_peer(
    state: AppState,
    recipient_keys: &Keys,
    method: &str,
    uri: &str,
    body: Body,
    header: Option<(&str, &str)>,
    peer: std::net::SocketAddr,
) -> Response {
    let body_bytes = to_bytes(body, 1024 * 1024).await.expect("request body");
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body_bytes.clone()));
    let mut request = request.expect("request");
    request.extensions_mut().insert(ConnectInfo(peer));
    if let Some((name, value)) = header {
        request.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes()).expect("header name"),
            value.parse().expect("header value"),
        );
    }
    if is_management_request(method, uri) && !request.headers().contains_key(header::AUTHORIZATION)
    {
        let authorization = nostr_authorization(
            recipient_keys,
            state.config().public_base_url(),
            method,
            uri,
            &body_bytes,
        );
        request.headers_mut().insert(
            header::AUTHORIZATION,
            authorization.parse().expect("authorization header"),
        );
    }
    let response = public_app(state).oneshot(request).await.expect("response");
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let cache_control = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let pragma = response
        .headers()
        .get(header::PRAGMA)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json response")
    };

    Response {
        status,
        body,
        request_id,
        cache_control,
        pragma,
    }
}

fn is_management_request(method: &str, uri: &str) -> bool {
    let path = uri
        .strip_prefix("http://127.0.0.1:3000")
        .unwrap_or(uri)
        .split_once('?')
        .map_or(uri, |(path, _)| path);
    path == "/registrations"
        || path.starts_with("/registrations/")
        || path == "/v1/hooks"
        || (method == "DELETE" && path.starts_with("/v1/hooks/"))
        || path == "/hooks/notification"
}

fn nostr_authorization(
    keys: &Keys,
    public_base_url: &str,
    method: &str,
    uri: &str,
    body: &[u8],
) -> String {
    nostr_authorization_custom(keys, public_base_url, method, uri, body, None)
}

fn nostr_authorization_custom(
    keys: &Keys,
    public_base_url: &str,
    method: &str,
    uri: &str,
    body: &[u8],
    timestamp: Option<Timestamp>,
) -> String {
    nostr_authorization_with_payload(
        keys,
        public_base_url,
        method,
        uri,
        body,
        timestamp,
        PayloadTag::DefaultForMethod,
    )
}

enum PayloadTag<'a> {
    DefaultForMethod,
    Omit,
    Value(&'a str),
}

fn nostr_authorization_with_payload(
    keys: &Keys,
    public_base_url: &str,
    method: &str,
    uri: &str,
    body: &[u8],
    timestamp: Option<Timestamp>,
    payload_tag: PayloadTag<'_>,
) -> String {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let path_and_query = uri.strip_prefix(public_base_url).unwrap_or(uri);
    let mut builder = EventBuilder::new(Kind::HttpAuth, "")
        .custom_created_at(timestamp.unwrap_or_else(Timestamp::now))
        .tag(Tag::parse(["u", &format!("{public_base_url}{path_and_query}")]).expect("u tag"))
        .tag(Tag::parse(["method", method]).expect("method tag"))
        .tag(
            Tag::parse(["nonce", &NONCE.fetch_add(1, Ordering::Relaxed).to_string()])
                .expect("nonce tag"),
        );
    match payload_tag {
        PayloadTag::DefaultForMethod if !matches!(method, "GET" | "DELETE") => {
            let payload = hex::encode(Sha256::digest(body));
            builder = builder.tag(Tag::parse(["payload", &payload]).expect("payload tag"));
        }
        PayloadTag::DefaultForMethod | PayloadTag::Omit => {}
        PayloadTag::Value(payload) => {
            builder = builder.tag(Tag::parse(["payload", payload]).expect("payload tag"));
        }
    }
    let event = builder.sign_with_keys(keys).expect("sign auth event");
    format!(
        "Nostr {}",
        general_purpose::STANDARD.encode(serde_json::to_vec(&event).expect("event json"))
    )
}

fn crate_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_secs() as i64
}

struct Response {
    status: StatusCode,
    body: Value,
    request_id: Option<String>,
    cache_control: Option<String>,
    pragma: Option<String>,
}

struct TextResponse {
    status: StatusCode,
    text: String,
}

fn hook_mvp_test_mutex() -> Arc<Mutex<()>> {
    static HOOK_MVP_TEST_MUTEX: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    HOOK_MVP_TEST_MUTEX
        .get_or_init(|| Arc::new(Mutex::new(())))
        .clone()
}
