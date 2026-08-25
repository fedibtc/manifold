use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use defe_api::{ResourceDescriptor, SharingMode};
use defe_client::AsyncDefeClient;
use futures_util::{SinkExt as _, StreamExt as _};
use secp256k1::{Keypair, Secp256k1, SecretKey, XOnlyPublicKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Error as WebSocketError, Message as WebSocketMessage},
};

type RelayWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[tokio::test]
async fn defe_client_connect_from_env_allocates_usable_relay() {
    let mut client = AsyncDefeClient::connect_from_env()
        .await
        .expect("connect to defe from env");
    let lease = client
        .request_nostr_relay(SharingMode::Shared)
        .await
        .expect("allocate Nostr relay");
    let ResourceDescriptor::NostrRelay(info) = &lease.descriptor else {
        panic!(
            "expected Nostr relay descriptor, got {:?}",
            lease.descriptor
        );
    };

    assert!(
        info.data_dir.is_dir(),
        "relay data dir exists: {}",
        info.data_dir.display()
    );
    let (host, port) = parse_ws_loopback_url(&info.url);
    assert_eq!(info.port, port, "relay URL port matches descriptor port");

    let addr = SocketAddr::new(host.parse().expect("relay URL host is an IP address"), port);
    tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr))
        .await
        .expect("relay accepts TCP connections before timeout")
        .expect("relay accepts TCP connections");
    publish_and_read_back_event(&info.url).await;

    client
        .release(lease.handle_id)
        .await
        .expect("release relay");
}

#[tokio::test]
async fn defe_client_connect_from_env_allocates_usable_push_gateway() {
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

    assert!(
        info.database_path
            .parent()
            .is_some_and(std::path::Path::is_dir),
        "push gateway database parent exists: {}",
        info.database_path.display()
    );
    assert_eq!(
        read_http_health(&info.host, info.port).await,
        r#"{"ok":true}"#
    );

    client
        .release(lease.handle_id)
        .await
        .expect("release push gateway");
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

    let subscription_id = format!("tests-e2e-relay-probe-{event_id}");
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
        "tests-e2e real relay probe from pid {} at {}",
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

async fn read_http_health(host: &str, port: u16) -> String {
    assert_eq!(host, "127.0.0.1", "push gateway listens on loopback");
    let addr = SocketAddr::new(host.parse().expect("gateway host is an IP address"), port);
    let mut stream =
        tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr))
            .await
            .expect("connect to push gateway TCP socket before timeout")
            .expect("connect to push gateway TCP socket");
    stream
        .write_all(
            b"GET /health HTTP/1.1
Host: 127.0.0.1
Connection: close

",
        )
        .await
        .expect("write health request");

    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_string(&mut response))
        .await
        .expect("read health response before timeout")
        .expect("read health response");
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .or_else(|| response.split_once("\n\n"))
        .expect("HTTP response has headers and body");
    assert!(
        headers.starts_with("HTTP/1.1 200 OK") || headers.starts_with("HTTP/1.0 200 OK"),
        "health response is successful: {headers}"
    );
    body.to_owned()
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
