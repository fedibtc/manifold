use std::env;
use std::ffi::OsStr;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt as _, StreamExt as _};
use secp256k1::{Keypair, Secp256k1, SecretKey, XOnlyPublicKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Error as WebSocketError, Message as WebSocketMessage},
};

type RelayWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const RUN_REAL_RELAY_TESTS_ENV: &str = "DEV_DEFE_RUN_REAL_NOSTR_RELAY_TESTS";
const REAL_RELAY_BIN_ENV: &str = "DEV_DEFE_NOSTR_RS_RELAY_BIN";
const RELAY_URL_ENV: &str = "DEV_DEFE_NOSTR_RELAY_URL";
const RELAY_PORT_ENV: &str = "DEV_DEFE_NOSTR_RELAY_PORT";
const RELAY_DATA_DIR_ENV: &str = "DEV_DEFE_NOSTR_RELAY_DATA_DIR";
const CHILD_PROBE_TEST: &str = "defe_cli_request_relay_child_probe_receives_usable_env";
const CHILD_PROBE_MARKER_ENV: &str = "DEV_DEFE_REAL_RELAY_WRAPPER_CHILD_PROBE";

#[tokio::test]
#[ignore = "opt-in real relay test; run in nix develop and set DEV_DEFE_RUN_REAL_NOSTR_RELAY_TESTS=1"]
async fn defe_cli_request_relay_through_real_defe_server() {
    if env::var_os(RUN_REAL_RELAY_TESTS_ENV).as_deref() != Some(OsStr::new("1")) {
        eprintln!(
            "skipping real defe-cli relay wrapper test; set {RUN_REAL_RELAY_TESTS_ENV}=1 to run"
        );
        return;
    }

    let mut command = tokio::process::Command::new("cargo");
    command.env(CHILD_PROBE_MARKER_ENV, "1");
    command.current_dir(workspace_root());
    command.args(["run", "-p", "defe", "--"]);
    if let Some(relay_bin) = env::var_os(REAL_RELAY_BIN_ENV) {
        command.arg("--nostr-rs-relay-bin").arg(relay_bin);
    }
    command
        .arg("exec")
        .arg(defe_cli_bin())
        .arg("--request-relay")
        .arg("--")
        .arg(current_test_binary())
        .args(["--exact", CHILD_PROBE_TEST, "--ignored", "--nocapture"]);

    let status = command
        .status()
        .await
        .expect("run defe real relay wrapper probe");
    assert!(
        status.success(),
        "defe-cli real relay wrapper probe failed with {status}"
    );
}

#[tokio::test]
#[ignore = "run only as a child of defe_cli_request_relay_through_real_defe_server"]
async fn defe_cli_request_relay_child_probe_receives_usable_env() {
    if env::var_os(CHILD_PROBE_MARKER_ENV).as_deref() != Some(OsStr::new("1")) {
        eprintln!(
            "skipping child relay env probe; run via defe_cli_request_relay_through_real_defe_server"
        );
        return;
    }

    let relay_url = env::var(RELAY_URL_ENV).expect("relay URL env is set");
    let relay_port = env::var(RELAY_PORT_ENV).expect("relay port env is set");
    let relay_data_dir =
        PathBuf::from(env::var_os(RELAY_DATA_DIR_ENV).expect("relay data dir env is set"));

    assert!(
        relay_data_dir.is_dir(),
        "relay data dir exists: {}",
        relay_data_dir.display()
    );

    let (host, port) = parse_ws_loopback_url(&relay_url);
    assert_eq!(
        relay_port,
        port.to_string(),
        "relay URL port matches {RELAY_PORT_ENV}"
    );

    let addr = SocketAddr::new(host.parse().expect("relay URL host is an IP address"), port);
    tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr))
        .await
        .expect("relay accepts TCP connections before timeout")
        .expect("relay accepts TCP connections");

    publish_and_read_back_event(&relay_url).await;
}

async fn publish_and_read_back_event(relay_url: &str) {
    let (mut websocket, _response) =
        tokio::time::timeout(Duration::from_secs(2), connect_async(relay_url))
            .await
            .expect("complete relay websocket handshake before timeout")
            .expect("complete relay websocket handshake");

    let event = make_test_event();
    let event_id = event["id"]
        .as_str()
        .expect("event id is a string")
        .to_owned();

    websocket
        .send(WebSocketMessage::text(json!(["EVENT", event]).to_string()))
        .await
        .expect("publish nostr event");
    expect_ok_for_event(&mut websocket, &event_id).await;

    let subscription_id = format!("defe-real-relay-probe-{event_id}");
    websocket
        .send(WebSocketMessage::text(
            json!(["REQ", subscription_id, { "ids": [event_id] }]).to_string(),
        ))
        .await
        .expect("request published nostr event by id");
    expect_event_for_subscription(&mut websocket, &subscription_id, &event_id).await;
}

fn make_test_event() -> Value {
    let secp = Secp256k1::new();
    let secret_key = SecretKey::from_byte_array(&[1; 32]).expect("test secret key is valid");
    let keypair = Keypair::from_secret_key(&secp, &secret_key);
    let (public_key, _parity) = XOnlyPublicKey::from_keypair(&keypair);
    let public_key = public_key.to_string();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after unix epoch");
    let created_at = now.as_secs();
    let kind = 1;
    let tags: Vec<Vec<String>> = Vec::new();
    let content = format!(
        "defe-cli real relay probe from pid {} at {}",
        std::process::id(),
        now.as_nanos()
    );

    let canonical_event = json!([0, public_key, created_at, kind, tags, content]);
    let canonical_event = serde_json::to_vec(&canonical_event).expect("serialize canonical event");
    let event_id = Sha256::digest(canonical_event);
    let signature = secp.sign_schnorr_no_aux_rand(&event_id, &keypair);

    json!({
        "id": hex::encode(event_id),
        "pubkey": public_key,
        "created_at": created_at,
        "kind": kind,
        "tags": tags,
        "content": content,
        "sig": signature.to_string(),
    })
}

async fn expect_ok_for_event(websocket: &mut RelayWebSocket, event_id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let value = read_json_message_before(websocket, deadline, "relay OK response").await;
        let Some(message) = value.as_array() else {
            continue;
        };
        if message.first().and_then(Value::as_str) != Some("OK") {
            continue;
        }
        if message.get(1).and_then(Value::as_str) != Some(event_id) {
            continue;
        }
        assert_eq!(
            message.get(2).and_then(Value::as_bool),
            Some(true),
            "relay accepted published event: {value}"
        );
        return;
    }
    panic!("timed out waiting for OK response for event {event_id}");
}

async fn expect_event_for_subscription(
    websocket: &mut RelayWebSocket,
    subscription_id: &str,
    event_id: &str,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let value = read_json_message_before(websocket, deadline, "relay EVENT response").await;
        let Some(message) = value.as_array() else {
            continue;
        };
        if message.first().and_then(Value::as_str) != Some("EVENT") {
            continue;
        }
        if message.get(1).and_then(Value::as_str) != Some(subscription_id) {
            continue;
        }
        assert_eq!(
            message
                .get(2)
                .and_then(|event| event.get("id"))
                .and_then(Value::as_str),
            Some(event_id),
            "relay returned the event requested by id: {value}"
        );
        return;
    }
    panic!("timed out waiting to read event {event_id} back from relay");
}

async fn read_json_message_before(
    websocket: &mut RelayWebSocket,
    deadline: tokio::time::Instant,
    context: &str,
) -> Value {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    tokio::time::timeout(remaining, read_json_message(websocket))
        .await
        .unwrap_or_else(|_| panic!("timed out reading {context}"))
        .unwrap_or_else(|err| panic!("read {context}: {err}"))
}

async fn read_json_message(websocket: &mut RelayWebSocket) -> Result<Value, WebSocketError> {
    loop {
        let Some(message) = websocket.next().await else {
            panic!("relay closed websocket before probe completed");
        };
        match message? {
            WebSocketMessage::Text(text) => {
                return Ok(serde_json::from_str(&text).expect("relay message is valid JSON"));
            }
            WebSocketMessage::Binary(bytes) => {
                return Ok(serde_json::from_slice(&bytes).expect("relay message is valid JSON"));
            }
            WebSocketMessage::Ping(payload) => {
                websocket.send(WebSocketMessage::Pong(payload)).await?
            }
            WebSocketMessage::Close(_) => panic!("relay closed websocket before probe completed"),
            WebSocketMessage::Pong(_) | WebSocketMessage::Frame(_) => {}
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("defe-client crate is under workspace/crates")
        .to_path_buf()
}

fn defe_cli_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_defe-cli"))
}

fn current_test_binary() -> PathBuf {
    env::current_exe().expect("current integration test binary path")
}

fn parse_ws_loopback_url(url: &str) -> (&str, u16) {
    let host_port = url
        .strip_prefix("ws://")
        .expect("relay URL uses ws:// scheme");
    let (host, port) = host_port
        .rsplit_once(':')
        .expect("relay URL contains host and port");
    assert_eq!(host, "127.0.0.1", "relay URL points at loopback");
    let port = port.parse::<u16>().expect("relay URL port is numeric");
    (host, port)
}
