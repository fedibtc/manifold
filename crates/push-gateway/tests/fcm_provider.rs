use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

use axum::{
    Form, Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use base64::{Engine, engine::general_purpose};
use fedi_decentralized_push_gateway::{
    AppId, AppState, Database, DeliveryOutboxRepository, DeliveryWorkerHandle, FcmProviderConfig,
    FcmPushProvider, FirebaseCredentials, PushGatewayConfig, PushProvider, PushProviderConfig,
    PushProviderErrorKind, app,
};
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::net::TcpListener;

#[tokio::test]
async fn fcm_provider_caches_oauth_token_and_sends_message_shape() {
    let server = FakeFcmServer::start().await;
    server.set_send_mode(SendMode::Success);
    let provider = provider_for(&server);
    let registration = test_registration("token-1");
    let notification = test_notification();

    provider
        .deliver(&registration, &notification)
        .await
        .expect("first send succeeds");
    provider
        .deliver(&registration, &notification)
        .await
        .expect("second send succeeds");

    assert_eq!(server.token_requests(), 1);
    assert_eq!(server.send_requests(), 2);
    assert!(!format!("{provider:?}").contains("access-token"));
    let sent = server.last_send_body().expect("send body");
    assert_eq!(sent["message"]["token"], "token-1");
    assert_eq!(sent["message"]["notification"]["title"], "Hello");
    assert_eq!(sent["message"]["notification"]["body"], "Body");
    assert_eq!(sent["message"]["data"]["pg.workflow"], "setup");
    assert_eq!(sent["message"]["data"]["attempt"], "7");
    assert_eq!(sent["message"]["data"]["urgent"], "true");
    assert_eq!(
        sent["message"]["data"]["context"],
        json!({"step": "dkg"}).to_string()
    );
    assert_eq!(sent["message"]["data"]["recipient_id"], "recipient");
    assert_eq!(sent["message"]["data"]["notification_id"], "notif-1");
    assert_eq!(sent["message"]["data"]["kind"], "hook");
}

#[tokio::test]
async fn fcm_registration_validation_uses_validate_only_without_notification() {
    let server = FakeFcmServer::start().await;
    let provider = provider_for(&server);

    provider
        .validate_registration(&fedi_decentralized_push_gateway::FcmRegistrationToken(
            "token-1".to_owned(),
        ))
        .await
        .expect("validation succeeds");

    assert_eq!(server.send_requests(), 1);
    let request = server.last_send_body().expect("validation request");
    assert_eq!(request["validate_only"], true);
    assert_eq!(request["message"]["token"], "token-1");
    assert!(request["message"]["notification"].is_null());
}

#[tokio::test]
async fn invalid_fcm_token_is_rejected_before_registration_is_persisted() {
    let server = FakeFcmServer::start().await;
    server.set_validation_mode(ValidationMode::Unregistered);
    let harness = Harness::new(&server).await;

    let response = signed_post_json(
        &harness,
        "/registrations",
        &json!({
            "installation_id": "device",
            "fcm_token": "wrong-project-token",
            "platform": "android"
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let registrations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_registrations")
        .fetch_one(harness.state.database().pool())
        .await
        .expect("count registrations");
    assert_eq!(registrations, 0);
}

#[tokio::test]
async fn ambiguous_fcm_validation_returns_503_without_registration_persistence() {
    let server = FakeFcmServer::start().await;
    server.set_validation_mode(ValidationMode::Unavailable);
    let harness = Harness::new(&server).await;

    let response = signed_post_json(
        &harness,
        "/registrations",
        &json!({
            "installation_id": "device",
            "fcm_token": "possibly-valid-token",
            "platform": "android"
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = response.json().await.expect("error json");
    assert_eq!(body["error"]["code"], "registration_validation_unavailable");
    let registrations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_registrations")
        .fetch_one(harness.state.database().pool())
        .await
        .expect("count registrations");
    assert_eq!(registrations, 0);
}

#[tokio::test]
async fn local_fcm_payload_validation_fails_before_oauth_or_send() {
    let server = FakeFcmServer::start().await;
    let provider = provider_for(&server);
    let registration = test_registration("token-1");
    let mut notification = test_notification();
    notification
        .data
        .insert("google.foo".to_owned(), Value::String("blocked".to_owned()));

    let err = provider
        .deliver(&registration, &notification)
        .await
        .expect_err("reserved FCM key rejected");

    assert_eq!(err.reason, "invalid_payload");
    assert_eq!(err.kind(), PushProviderErrorKind::InvalidPayload);
    assert_eq!(server.token_requests(), 0);
    assert_eq!(server.send_requests(), 0);
}

#[tokio::test]
async fn bare_fcm_not_found_is_transient_without_disabling_registration() {
    let server = FakeFcmServer::start().await;
    server.set_send_mode(SendMode::BareNotFound);
    let harness = Harness::new(&server).await;
    register_installation(&harness, "live-token").await;
    let url = create_hook(&harness).await;

    let response = test_http_client()
        .post(format!("{}{}", harness.base_url, url))
        .json(&json!({}))
        .send()
        .await
        .expect("invoke hook");
    assert_eq!(response.status(), StatusCode::OK);
    wait_for_status(&harness, "retrying", 1).await;

    let active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM push_registrations WHERE disabled_at IS NULL")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count active registrations");
    assert_eq!(active, 1);
}

#[tokio::test]
async fn invalid_fcm_token_is_classified_permanent_and_disabled_by_hook_invocation() {
    let server = FakeFcmServer::start().await;
    server.set_send_mode(SendMode::Unregistered);
    let harness = Harness::new(&server).await;
    register_installation(&harness, "dead-token").await;
    let url = create_hook(&harness).await;

    let response = test_http_client()
        .post(format!("{}{}", harness.base_url, url))
        .json(&json!({}))
        .send()
        .await
        .expect("invoke hook");
    assert_eq!(response.status(), StatusCode::OK);
    wait_for_status(&harness, "invalid_token", 1).await;

    let active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM push_registrations WHERE disabled_at IS NULL")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count active registrations");
    let reason: String = sqlx::query_scalar(
        "SELECT disabled_reason FROM push_registrations WHERE installation_id = 'device'",
    )
    .fetch_one(harness.state.database().pool())
    .await
    .expect("disabled reason");
    assert_eq!(active, 0);
    assert_eq!(reason, "invalid_token");
}

#[tokio::test]
async fn fcm_invalid_argument_with_fcm_error_disables_token() {
    let server = FakeFcmServer::start().await;
    server.set_send_mode(SendMode::TokenInvalidArgument);
    let harness = Harness::new(&server).await;
    register_installation(&harness, "bad-token-shape").await;
    let url = create_hook(&harness).await;

    let response = test_http_client()
        .post(format!("{}{}", harness.base_url, url))
        .json(&json!({}))
        .send()
        .await
        .expect("invoke hook");
    assert_eq!(response.status(), StatusCode::OK);
    wait_for_status(&harness, "invalid_token", 1).await;

    let active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM push_registrations WHERE disabled_at IS NULL")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count active registrations");
    assert_eq!(active, 0);
}

#[tokio::test]
async fn fcm_generic_bad_payload_invalid_argument_does_not_disable_token() {
    let server = FakeFcmServer::start().await;
    server.set_send_mode(SendMode::GenericBadPayloadInvalidArgument);
    let harness = Harness::new(&server).await;
    register_installation(&harness, "live-token").await;
    let url = create_hook(&harness).await;

    let response = test_http_client()
        .post(format!("{}{}", harness.base_url, url))
        .json(&json!({}))
        .send()
        .await
        .expect("invoke hook");
    assert_eq!(response.status(), StatusCode::OK);
    wait_for_status(&harness, "dead_letter", 1).await;

    let active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM push_registrations WHERE disabled_at IS NULL")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count active registrations");
    assert_eq!(active, 1);
}

#[tokio::test]
async fn transient_fcm_error_is_surfaced_without_disabling_registration() {
    let server = FakeFcmServer::start().await;
    server.set_send_mode(SendMode::Unavailable);
    let harness = Harness::new(&server).await;
    register_installation(&harness, "live-token").await;
    let url = create_hook(&harness).await;

    let response = test_http_client()
        .post(format!("{}{}", harness.base_url, url))
        .json(&json!({}))
        .send()
        .await
        .expect("invoke hook");
    assert_eq!(response.status(), StatusCode::OK);
    wait_for_status(&harness, "retrying", 1).await;

    let active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM push_registrations WHERE disabled_at IS NULL")
            .fetch_one(harness.state.database().pool())
            .await
            .expect("count active registrations");
    assert_eq!(active, 1);
}

#[test]
fn credential_and_config_debug_redacts_secret_material() {
    let credentials = credentials_for("http://127.0.0.1:9/token");
    let config = FcmProviderConfig::new(credentials.clone());

    let debug = format!("{credentials:?} {config:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("PRIVATE KEY"));
    assert!(!debug.contains("svc@example.test"));
}

struct Harness {
    state: AppState,
    base_url: String,
    recipient_keys: Keys,
    _tempdir: TempDir,
    _worker: DeliveryWorkerHandle,
}

impl Harness {
    async fn new(server: &FakeFcmServer) -> Self {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_url = format!(
            "sqlite://{}?mode=rwc",
            tempdir.path().join("push.sqlite").display()
        );
        let fcm_config = fcm_config_for(server);
        let config = PushGatewayConfig::new(Some(AppId("test-app".to_owned())), database_url, None)
            .with_provider(PushProviderConfig::Fcm(fcm_config.clone()))
            .try_with_local_test_public_base_url("http://127.0.0.1:3000")
            .expect("local test public base URL");
        let database = Database::connect(config.database_url())
            .await
            .expect("database");
        let provider = Arc::new(provider_for_config(&fcm_config));
        let state = AppState::with_push_provider(config, database, provider);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind app");
        let base_url = format!("http://{}", listener.local_addr().expect("addr"));
        let worker = state.start_delivery_worker();
        tokio::spawn(axum::serve(listener, app(state.clone())).into_future());
        Self {
            state,
            base_url,
            recipient_keys: Keys::generate(),
            _tempdir: tempdir,
            _worker: worker,
        }
    }
}

async fn wait_for_status(harness: &Harness, status: &str, count: i64) {
    for _ in 0..100 {
        let actual = DeliveryOutboxRepository::new(
            harness.state.database().pool().clone(),
            harness.state.database().backend(),
        )
        .count_by_status(status)
        .await
        .expect("count status");
        if actual >= count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {status}");
}

async fn register_installation(harness: &Harness, token: &str) {
    let response = signed_post_json(
        harness,
        "/registrations",
        &json!({
            "installation_id": "device",
            "fcm_token": token,
            "platform": "android"
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

async fn create_hook(harness: &Harness) -> String {
    let response = signed_post_json(
        harness,
        "/v1/hooks",
        &json!({
            "installation_id": "device",
            "notification": {
                "kind": "hook",
                "title": "Hello"
            },
            "open": {
                "workflow": "setup",
                "behavior": "open_workflow"
            },
            "policy": {"ttl_seconds": 3600}
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    response.json::<Value>().await.expect("create body")["invocation_url"]
        .as_str()
        .expect("invocation url")
        .strip_prefix("http://127.0.0.1:3000")
        .expect("configured public base")
        .to_owned()
}

async fn signed_post_json(harness: &Harness, path: &str, body: &Value) -> reqwest::Response {
    let body = body.to_string();
    test_http_client()
        .post(format!("{}{}", harness.base_url, path))
        .header("content-type", "application/json")
        .header(
            "authorization",
            nostr_authorization(&harness.recipient_keys, "POST", path, body.as_bytes()),
        )
        .body(body)
        .send()
        .await
        .expect("signed request")
}

fn nostr_authorization(keys: &Keys, method: &str, path: &str, body: &[u8]) -> String {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let payload = hex::encode(Sha256::digest(body));
    let event = EventBuilder::new(Kind::HttpAuth, "")
        .custom_created_at(Timestamp::now())
        .tag(Tag::parse(["u", &format!("http://127.0.0.1:3000{path}")]).expect("u"))
        .tag(Tag::parse(["method", method]).expect("method"))
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

#[derive(Clone)]
struct FakeFcmServer {
    address: String,
    state: FakeFcmState,
}

impl FakeFcmServer {
    async fn start() -> Self {
        let state = FakeFcmState::default();
        let router = Router::new()
            .route("/token", post(token_handler))
            .route(
                "/v1/projects/test-project/messages:send",
                post(send_handler),
            )
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fcm");
        let address = format!("http://{}", listener.local_addr().expect("addr"));
        tokio::spawn(axum::serve(listener, router).into_future());
        Self { address, state }
    }

    fn set_send_mode(&self, mode: SendMode) {
        *self.state.send_mode.lock().expect("send mode") = mode;
    }

    fn set_validation_mode(&self, mode: ValidationMode) {
        *self.state.validation_mode.lock().expect("validation mode") = mode;
    }

    fn token_requests(&self) -> usize {
        self.state.token_requests.load(Ordering::SeqCst)
    }

    fn send_requests(&self) -> usize {
        self.state.send_requests.load(Ordering::SeqCst)
    }

    fn last_send_body(&self) -> Option<Value> {
        self.state.last_send_body.lock().expect("send body").clone()
    }
}

#[derive(Clone, Default)]
struct FakeFcmState {
    token_requests: Arc<AtomicUsize>,
    send_requests: Arc<AtomicUsize>,
    last_send_body: Arc<Mutex<Option<Value>>>,
    send_mode: Arc<Mutex<SendMode>>,
    validation_mode: Arc<Mutex<ValidationMode>>,
}

#[derive(Clone, Copy, Default)]
enum SendMode {
    #[default]
    Success,
    Unregistered,
    TokenInvalidArgument,
    GenericBadPayloadInvalidArgument,
    Unavailable,
    BareNotFound,
}

#[derive(Clone, Copy, Default)]
enum ValidationMode {
    #[default]
    Success,
    Unregistered,
    Unavailable,
}

async fn token_handler(
    State(state): State<FakeFcmState>,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    state.token_requests.fetch_add(1, Ordering::SeqCst);
    assert_eq!(
        form.get("grant_type").map(String::as_str),
        Some("urn:ietf:params:oauth:grant-type:jwt-bearer")
    );
    let assertion = form.get("assertion").expect("assertion");
    let parts = assertion.split('.').collect::<Vec<_>>();
    assert_eq!(parts.len(), 3);
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .expect("decode jwt claims");
    let claims: Value = serde_json::from_slice(&claims).expect("claims json");
    assert_eq!(claims["iss"], "svc@example.test");
    let aud = claims["aud"].as_str().expect("aud");
    assert!(aud.starts_with("http://127.0.0.1:"));
    assert!(aud.ends_with("/token"));
    assert_eq!(
        claims["scope"],
        "https://www.googleapis.com/auth/firebase.messaging"
    );
    Json(json!({
        "access_token": "access-token",
        "token_type": "Bearer",
        "expires_in": 3600
    }))
}

async fn send_handler(
    State(state): State<FakeFcmState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    state.send_requests.fetch_add(1, Ordering::SeqCst);
    assert_eq!(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer access-token")
    );
    let validate_only = body["validate_only"] == true;
    *state.last_send_body.lock().expect("send body") = Some(body);
    if validate_only {
        return match *state.validation_mode.lock().expect("validation mode") {
            ValidationMode::Success => (StatusCode::OK, Json(json!({"name": "validated"}))),
            ValidationMode::Unregistered => (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": {
                        "status": "NOT_FOUND",
                        "details": [{
                            "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
                            "error_code": "UNREGISTERED"
                        }]
                    }
                })),
            ),
            ValidationMode::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": {"status": "UNAVAILABLE"}})),
            ),
        };
    }
    match *state.send_mode.lock().expect("send mode") {
        SendMode::Success => (StatusCode::OK, Json(json!({"name": "message-id"}))),
        SendMode::Unregistered => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "status": "NOT_FOUND",
                    "details": [{
                        "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
                        "error_code": "UNREGISTERED"
                    }]
                }
            })),
        ),
        SendMode::TokenInvalidArgument => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "status": "INVALID_ARGUMENT",
                    "details": [{
                        "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
                        "error_code": "INVALID_ARGUMENT"
                    }]
                }
            })),
        ),
        SendMode::GenericBadPayloadInvalidArgument => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "status": "INVALID_ARGUMENT",
                    "details": [{
                        "@type": "type.googleapis.com/google.rpc.BadRequest",
                        "fieldViolations": [{
                            "field": "message.data[0].value",
                            "description": "Invalid value"
                        }]
                    }]
                }
            })),
        ),
        SendMode::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": {"status": "UNAVAILABLE"}})),
        ),
        SendMode::BareNotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"status": "NOT_FOUND"}})),
        ),
    }
}

fn provider_for(server: &FakeFcmServer) -> FcmPushProvider {
    provider_for_config(&fcm_config_for(server))
}

fn provider_for_config(config: &FcmProviderConfig) -> FcmPushProvider {
    let client = test_http_client();
    FcmPushProvider::with_http_client(config, client)
}

fn test_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .tls_certs_only([])
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("test HTTP client")
}

fn fcm_config_for(server: &FakeFcmServer) -> FcmProviderConfig {
    FcmProviderConfig::new(credentials_for(&format!("{}/token", server.address)))
        .with_send_endpoint_base(&server.address)
        .with_max_concurrency(2)
}

fn credentials_for(token_uri: &str) -> FirebaseCredentials {
    FirebaseCredentials::from_json(
        &json!({
            "type": "service_account",
            "project_id": "test-project",
            "client_email": "svc@example.test",
            "private_key": TEST_PRIVATE_KEY,
            "token_uri": token_uri
        })
        .to_string(),
    )
    .expect("credentials")
}

fn test_registration(token: &str) -> fedi_decentralized_push_gateway::PushRegistration {
    fedi_decentralized_push_gateway::PushRegistration {
        recipient_id: fedi_decentralized_push_gateway::RecipientId("recipient".to_owned()),
        installation_id: fedi_decentralized_push_gateway::DeviceInstallationId("device".to_owned()),
        fcm_token: fedi_decentralized_push_gateway::FcmRegistrationToken(token.to_owned()),
        platform: None,
        created_at: 0,
        last_seen_at: 0,
        disabled_at: None,
        disabled_reason: None,
    }
}

fn test_notification() -> fedi_decentralized_push_gateway::Notification {
    fedi_decentralized_push_gateway::Notification {
        recipient_id: fedi_decentralized_push_gateway::RecipientId("recipient".to_owned()),
        notification_id: fedi_decentralized_push_gateway::NotificationId("notif-1".to_owned()),
        kind: fedi_decentralized_push_gateway::NotificationKind("hook".to_owned()),
        title: Some("Hello".to_owned()),
        body: Some("Body".to_owned()),
        data: serde_json::Map::from_iter([
            ("pg.workflow".to_owned(), Value::String("setup".to_owned())),
            ("attempt".to_owned(), Value::Number(7.into())),
            ("urgent".to_owned(), Value::Bool(true)),
            ("context".to_owned(), json!({"step": "dkg"})),
        ]),
    }
}

const TEST_PRIVATE_KEY: &str = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEArLgnsjAXR1TdzLd90QA8+PYAVFSvhe0O4r6m5gPzfVdopgU1
UmaQWfD3mN93dYdMamadYyuroTSob9kiGpRm6N+rPhotNTcDNk+x74mSnQDCqFxL
w6XaCrVBSfpgztc/ztJMYvxDVC83A5rusdzt3unDdlp33sbojeCkg7aY8c2HQ0Dd
o01pV0UWXunGf1OtPM6uxfprFgSh9jC+gcr7D6Z1aAPQAEX5HZmc0Zm08qTs3HXg
HA8E8MGVqBoLB47AghvATcAPlQ33TyjM7vtaeUc55SonElAGuWxnlD8oarnyq2pB
QS+pwwRJctJl0EuxtvxlZgWnIhX0069p9GWB3wIDAQABAoIBAE230bjm1dq1j9ZJ
rLYKTuVRwGUx9Acl2+BgnHX8yigY2FB4IH2zA/pMqQTjbPv4BQUNpn1UzbZMnQwz
HprqMwJPft0DZ1s+JVZfdvgLpeq6yFx8p2TicKIH3Fh+7uezyJT2YQPbcipj0nPv
V7+1410+P8M2QyD9zO/maPCRjfGjZH6EbJF/q9NcrSGbiCetrEdtkCP0LbiA3TBh
aXWy9qbMzwVIw1glMNL3y+FRAVsMZuyi6CRwcUGQt6tCRwYAU2+xFRxGMeA/n4PR
pKjOOi4pPFfBTjYLvjblEH2zZEyJk3tcBAl0TKz8KnGT0xNwUzOg9A3dFnXgKuuP
HbXObF0CgYEA78adoVnE9OPOWiN7kgKG/qm5cSA/yzao4qpCUOO4oQKrrXMkhs8s
WSNdtLSK+CNP2EETHZtwhSqbAnEy9yikYm004nSknV6yJTo6QCP5DckrBXl59J+E
Qt2w54ugpOKAE2wSa2lNzfVse6P1F+2g1qhtGRuu2KX6qyIqwhTLjPUCgYEAuGf9
jop5U0jmjB6Iv9nxxakzB32PGbsyW9BdRPEAopDMJeObgwRhG/1EWa/Q74P2D+6Q
lauiDoDoDnAbX8r42jY/+9MzkcpIrBpHKpflWkrIAdYSxU+857otfxSuuuIWzyGx
ZyxQzSa+lTSZiv00zQCtalQT9+OwFlfP6w1ujwMCgYEAqYxijmOx+BDWK7sHeBm9
Z3qQnMPXGFVQWudV+Wjtdz0yNHZFD+aTT3zImC1KT2h430w0vizaBfA4qCNvjIH6
q3bZfIBKntUFV3mzEwPc6rijaT2a1TWvCrFElJaRQ8a+Ff3HkJhn4gl3an5no0Hv
B5sVejmvC5dih3yji5W00bkCgYBKR+FULKVojgIISThuh30jUN+0Ubh19fj4EPux
DJ9j3I3PaVq4MOhpHOEOe4rfIDna+w8UqxlRXE2dmzz7nkgVpiqp5s5sGJ6jbMZj
+uGxOFROoQvYnSEL+uvet9cWgoILl5fdZnV53fSBJ7n9ybceKPqxzQJqJTZGGcMv
/K4fPwKBgDFNouRePI5zrhg3ErEoZsAT/se9SQl8xUDLay1DndnT8L81l9VgvK9s
YN2QFn/rYPGplcJet9vsE6cyrJSsIMuimQEt+psgVGWrnHNZn0dFOeS9cg+t1BPW
xpNMyzCNgTkwus9x1CXKT9oeAPDUEPwa7RmK9GwcCi1AKlytERl2
-----END RSA PRIVATE KEY-----"#;
