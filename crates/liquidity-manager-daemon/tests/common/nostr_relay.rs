use std::time::Duration;

use anyhow::Context;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Error as WebSocketError, Message as WebSocketMessage},
};

use super::defe::DefeResources;

type RelayWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub struct NostrRelayFixture {
    ws_url: String,
    resources: DefeResources,
}

impl NostrRelayFixture {
    pub async fn start() -> anyhow::Result<Self> {
        let resources = DefeResources::relay_only().await?;
        let this = Self {
            ws_url: resources.relay().url.clone(),
            resources,
        };
        this.wait_for_tcp().await?;
        Ok(this)
    }

    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    pub async fn close(self) -> anyhow::Result<()> {
        self.resources.release().await
    }

    async fn wait_for_tcp(&self) -> anyhow::Result<()> {
        let (host, port) = parse_ws_url(&self.ws_url)?;
        tokio::time::timeout(
            Duration::from_secs(10),
            tokio::net::TcpStream::connect((host.as_str(), port)),
        )
        .await
        .context("timed out waiting for nostr-rs-relay TCP listener")?
        .context("nostr-rs-relay TCP listener did not accept connections")?;
        Ok(())
    }
}

pub async fn query_advertisement_event(
    relay_url: &str,
    provider_pubkey: &str,
    kind: u16,
    d_tag: &str,
    hashtag: &str,
) -> anyhow::Result<Value> {
    let (mut websocket, _response) =
        tokio::time::timeout(Duration::from_secs(10), connect_async(relay_url))
            .await
            .context("timed out connecting to nostr-rs-relay websocket")?
            .context("failed to connect to nostr-rs-relay websocket")?;

    let subscription_id = format!("flip-advertisement-{}", unique_suffix());
    websocket
        .send(WebSocketMessage::text(
            json!([
                "REQ",
                subscription_id,
                {
                    "kinds": [kind],
                    "authors": [provider_pubkey],
                    "#d": [d_tag],
                    "#t": [hashtag]
                }
            ])
            .to_string(),
        ))
        .await
        .context("failed to send Nostr REQ")?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let value = read_json_message_before(&mut websocket, deadline, "relay EVENT response")
            .await
            .context("failed to read relay message")?;
        let Some(message) = value.as_array() else {
            continue;
        };
        if message.first().and_then(Value::as_str) != Some("EVENT") {
            continue;
        }
        if message.get(1).and_then(Value::as_str) != Some(subscription_id.as_str()) {
            continue;
        }
        let Some(event) = message.get(2) else {
            continue;
        };
        if event.get("pubkey").and_then(Value::as_str) != Some(provider_pubkey) {
            continue;
        }
        if event.get("kind").and_then(Value::as_u64) != Some(u64::from(kind)) {
            continue;
        }
        if !event_has_tag(event, "d", d_tag) || !event_has_tag(event, "t", hashtag) {
            continue;
        }
        return Ok(event.clone());
    }

    anyhow::bail!("timed out waiting to read FLIP advertisement event back from relay")
}

/// Reads whatever advertisement the relay serves for this provider right now.
///
/// Unlike `query_advertisement_event`, this returns at the relay's
/// end-of-stored-events marker rather than waiting out a deadline, so `None` is
/// the relay answering that it holds nothing — the assertion a withdrawal needs
/// — and not merely a publication that has yet to arrive.
pub async fn find_advertisement_event(
    relay_url: &str,
    provider_pubkey: &str,
    kind: u16,
    d_tag: &str,
    hashtag: &str,
) -> anyhow::Result<Option<Value>> {
    let (mut websocket, _response) =
        tokio::time::timeout(Duration::from_secs(10), connect_async(relay_url))
            .await
            .context("timed out connecting to nostr-rs-relay websocket")?
            .context("failed to connect to nostr-rs-relay websocket")?;

    let subscription_id = format!("flip-advertisement-probe-{}", unique_suffix());
    websocket
        .send(WebSocketMessage::text(
            json!([
                "REQ",
                subscription_id,
                {
                    "kinds": [kind],
                    "authors": [provider_pubkey],
                    "#d": [d_tag],
                    "#t": [hashtag]
                }
            ])
            .to_string(),
        ))
        .await
        .context("failed to send Nostr REQ")?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let value = read_json_message_before(&mut websocket, deadline, "relay EVENT response")
            .await
            .context("failed to read relay message")?;
        let Some(message) = value.as_array() else {
            continue;
        };
        if message.get(1).and_then(Value::as_str) != Some(subscription_id.as_str()) {
            continue;
        }
        match message.first().and_then(Value::as_str) {
            Some("EVENT") => {
                let Some(event) = message.get(2) else {
                    continue;
                };
                if event.get("pubkey").and_then(Value::as_str) != Some(provider_pubkey) {
                    continue;
                }
                if event.get("kind").and_then(Value::as_u64) != Some(u64::from(kind)) {
                    continue;
                }
                if !event_has_tag(event, "d", d_tag) || !event_has_tag(event, "t", hashtag) {
                    continue;
                }
                return Ok(Some(event.clone()));
            }
            Some("EOSE") => return Ok(None),
            _ => continue,
        }
    }

    anyhow::bail!("relay sent neither a matching advertisement nor EOSE before the deadline")
}

async fn read_json_message_before(
    websocket: &mut RelayWebSocket,
    deadline: tokio::time::Instant,
    context: &str,
) -> anyhow::Result<Value> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    tokio::time::timeout(remaining, read_json_message(websocket))
        .await
        .with_context(|| format!("timed out reading {context}"))?
        .with_context(|| format!("read {context}"))
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

fn event_has_tag(event: &Value, key: &str, expected: &str) -> bool {
    event
        .get("tags")
        .and_then(Value::as_array)
        .is_some_and(|tags| {
            tags.iter().any(|tag| {
                let Some(values) = tag.as_array() else {
                    return false;
                };
                values.first().and_then(Value::as_str) == Some(key)
                    && values.get(1).and_then(Value::as_str) == Some(expected)
            })
        })
}

fn parse_ws_url(url: &str) -> anyhow::Result<(String, u16)> {
    let host_port = url
        .strip_prefix("ws://")
        .context("relay URL must use ws:// scheme")?;
    let (host, port) = host_port
        .rsplit_once(':')
        .context("relay URL must contain host and port")?;
    let port = port.parse::<u16>().context("relay URL port is numeric")?;
    Ok((host.to_owned(), port))
}

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_nanos();
    format!("{}_{}", std::process::id(), nanos)
}
