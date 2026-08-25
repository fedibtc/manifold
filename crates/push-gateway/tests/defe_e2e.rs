use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use base64::{Engine, engine::general_purpose};
use defe_api::{ResourceDescriptor, SharingMode};
use defe_client::AsyncDefeClient;
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::AnyPool;

const INSTALLATION_ID: &str = "installation-1";
const CALLER_IDEMPOTENCY_KEY: &str = "notification-1";

/// Runs the push gateway through `defe` and exercises the HTTP registration and
/// notification hook APIs in the default no-op provider mode. Ignored by
/// default because it needs `defe exec` or a persistent `defe serve` with
/// push-gateway resource support.
#[ignore = "requires a running defe server with push-gateway resource support"]
#[tokio::test]
async fn defe_managed_push_gateway_invokes_hook_without_real_fcm() {
    let mut client = AsyncDefeClient::connect_from_env()
        .await
        .expect("connect to defe from env");
    let lease = client
        .request_push_gateway(SharingMode::Exclusive)
        .await
        .expect("allocate push gateway");
    let ResourceDescriptor::PushGateway(info) = &lease.descriptor else {
        panic!(
            "expected push gateway descriptor, got {:?}",
            lease.descriptor
        );
    };
    let recipient_keys = Keys::generate();
    let recipient_id = recipient_keys.public_key().to_string();

    let registration = signed_post_json(
        "register installation",
        &info.url,
        "/registrations",
        json!({
            "installation_id": INSTALLATION_ID,
            "fcm_token": "fcm-token-1",
            "platform": "android",
        }),
        &recipient_keys,
    )
    .await;
    assert_eq!(registration["registered"], true);

    let create = signed_post_json(
        "create hook",
        &info.url,
        "/v1/hooks",
        json!({
            "installation_id": INSTALLATION_ID,
            "notification": {
                "kind": "test.notification",
                "title": "Default title",
                "body": "Test body"
            },
            "open": {
                "workflow": "test_workflow",
                "behavior": "open_workflow"
            },
            "data": { "hook_source": "defe-hook" },
            "policy": {"ttl_seconds": 3600},
        }),
        &recipient_keys,
    )
    .await;
    let invocation_url = create["invocation_url"]
        .as_str()
        .expect("hook create returns invocation_url");
    let hook_id = create["hook"]["hook_id"]
        .as_str()
        .expect("hook create returns hook_id");

    let invocation = post_json(
        "invoke hook",
        &info.url,
        invocation_url,
        json!({
            "idempotency_key": CALLER_IDEMPOTENCY_KEY,
            "data": { "caller_source": "defe-e2e" },
        }),
    )
    .await;
    assert_eq!(invocation["accepted"], true);
    assert_eq!(invocation["delivery_attempts"], 1);

    sqlx::any::install_default_drivers();
    let pool = AnyPool::connect(&format!(
        "sqlite://{}?mode=rw",
        info.database_path.display()
    ))
    .await
    .expect("connect to defe-managed push gateway database");
    let notification = wait_for_succeeded_outbox_notification(
        &pool,
        hook_id,
        CALLER_IDEMPOTENCY_KEY,
        &recipient_id,
        INSTALLATION_ID,
    )
    .await;
    assert_eq!(notification["recipient_id"], recipient_id);
    assert!(
        notification["notification_id"]
            .as_str()
            .expect("notification_id")
            .starts_with(&format!("hook:{hook_id}:"))
    );
    assert_eq!(notification["kind"], "test.notification");
    assert_eq!(notification["title"], "Default title");
    assert_eq!(notification["body"], "Test body");
    assert_eq!(notification["data"]["pg.workflow"], "test_workflow");
    assert_eq!(notification["data"]["hook_source"], "defe-hook");
    assert_eq!(notification["data"]["caller_source"], "defe-e2e");
}

async fn signed_post_json(
    label: &str,
    base_url: &str,
    path: &str,
    body: Value,
    keys: &Keys,
) -> Value {
    let body_string = body.to_string();
    let url = format!("{base_url}{path}");
    let response = reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .header(
            "authorization",
            nostr_authorization(keys, base_url, path, body_string.as_bytes()),
        )
        .body(body_string)
        .send()
        .await
        .unwrap_or_else(|err| panic!("{label} request failed: {}", err.without_url()));
    response_json(label, response).await
}

async fn post_json(label: &str, base_url: &str, path: &str, body: Value) -> Value {
    let url = if path.starts_with("http://") || path.starts_with("https://") {
        path.to_owned()
    } else {
        format!("{base_url}{path}")
    };
    let response = reqwest::Client::new()
        .post(url)
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|err| panic!("{label} request failed: {}", err.without_url()));
    response_json(label, response).await
}

async fn response_json(label: &str, response: reqwest::Response) -> Value {
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .unwrap_or_else(|err| panic!("{label} response body failed: {}", err.without_url()));
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes);
        panic!("{label} returned {status}: {body}");
    }

    serde_json::from_slice(&bytes).unwrap_or_else(|_| panic!("{label} response body is JSON"))
}

fn nostr_authorization(keys: &Keys, public_base_url: &str, path: &str, body: &[u8]) -> String {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let payload = hex::encode(Sha256::digest(body));
    let event = EventBuilder::new(Kind::HttpAuth, "")
        .custom_created_at(Timestamp::now())
        .tag(Tag::parse(["u", &format!("{public_base_url}{path}")]).expect("u"))
        .tag(Tag::parse(["method", "POST"]).expect("method"))
        .tag(Tag::parse(["payload", &payload]).expect("payload"))
        .tag(
            Tag::parse(["nonce", &NONCE.fetch_add(1, Ordering::Relaxed).to_string()])
                .expect("nonce"),
        )
        .sign_with_keys(keys)
        .expect("sign");
    format!(
        "Nostr {}",
        general_purpose::STANDARD.encode(serde_json::to_vec(&event).expect("event"))
    )
}

async fn wait_for_succeeded_outbox_notification(
    pool: &AnyPool,
    hook_id: &str,
    caller_idempotency_key: &str,
    recipient_id: &str,
    installation_id: &str,
) -> Value {
    for _ in 0..100 {
        let notification_json = sqlx::query_scalar::<_, String>(
            "SELECT d.notification_json
             FROM delivery_outbox d
             JOIN notification_events e ON e.event_id = d.event_id
             WHERE d.status = 'succeeded'
               AND e.hook_id = $1
               AND e.caller_idempotency_key = $2
               AND d.recipient_id = $3
               AND d.installation_id = $4
             LIMIT 1",
        )
        .bind(hook_id)
        .bind(caller_idempotency_key)
        .bind(recipient_id)
        .bind(installation_id)
        .fetch_optional(pool)
        .await
        .expect("query delivery outbox");
        if let Some(notification_json) = notification_json {
            return serde_json::from_str(&notification_json).expect("notification JSON");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "timed out waiting for no-op provider delivery through defe resource \
         (hook_id={hook_id}, idempotency_key={caller_idempotency_key}, recipient_id={recipient_id}, \
         installation_id={installation_id})"
    );
}
