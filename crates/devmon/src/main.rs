//! Read-only development monitor for the decentralized-federations environment.
//!
//! Subscribes to the Nostr relay(s) the components advertise on and streams every event to a
//! browser dashboard over SSE. It never publishes events and never calls a mutating RPC: it is
//! an observer, so it can watch a live environment without perturbing it.
//!
//! Development tool. Not part of CI.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use axum::Router;
use axum::extract::{Json, State};
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use clap::Parser;
use fedi_decentralized_nostr::fman::{
    FMAN_ADVERTISEMENT_EVENT_KIND, HOLDER_AUTHORIZATION_EVENT_KIND,
};
use fedi_decentralized_nostr::setup_payment_federations::SETUP_PAYMENT_FEDERATIONS_EVENT_KIND;
use futures::stream::{Stream, StreamExt as _};
use nostr_sdk::{Client, Filter, RelayPoolNotification};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;

/// The human name the dashboard shows for a kind.
///
/// Shared kinds use their protocol-crate constants. The rest are provisional
/// and are named here so they render meaningfully when publishing begins.
fn kind_name(kind: u16) -> &'static str {
    match kind {
        k if k == FMAN_ADVERTISEMENT_EVENT_KIND => "FMan advertisement",
        k if k == HOLDER_AUTHORIZATION_EVENT_KIND => "Holder authorization",
        37702 => "FLIP advertisement (provisional)",
        37703 => "Attester issuer mirror (provisional)",
        37704 => "Credential revocation (provisional)",
        k if k == SETUP_PAYMENT_FEDERATIONS_EVENT_KIND => "Setup-payment federation set",
        7321 => "Trust badge (provisional)",
        _ => "unknown kind",
    }
}

#[derive(Parser)]
#[command(about = "Read-only dashboard for the decentralized-federations dev environment")]
struct Args {
    /// Nostr relay to observe on startup. Optional: the relay can also be set from the page,
    /// which is how you point it at a defe-leased relay or a production relay without a restart.
    #[arg(long = "relay")]
    relay: Option<String>,

    /// Port to serve the dashboard on.
    #[arg(long, default_value_t = 7777)]
    port: u16,
}

/// One FMan as the dashboard understands it, derived purely from its advertisements.
#[derive(Clone, serde::Serialize)]
struct FmanRow {
    pubkey: String,
    endpoints: Vec<String>,
    plans: Vec<String>,
    holder_authorizations: usize,
    expires_at: u64,
    last_seen: u64,
}

/// Everything the dashboard knows, rebuilt from the event stream alone.
#[derive(Default)]
struct Dashboard {
    /// Latest advertisement per FMan pubkey. 37701 is addressable, so one ad per author is
    /// current by construction; this map mirrors that.
    fmans: BTreeMap<String, FmanRow>,
    kind_counts: BTreeMap<u16, u64>,
    total_events: u64,
}

struct AppState {
    dashboard: Mutex<Dashboard>,
    events: broadcast::Sender<String>,
    /// The relay currently being watched, and the task watching it. Changing the relay aborts
    /// the old task and spawns a fresh one, so there is never more than one live subscription.
    relay: Mutex<RelayWatch>,
}

#[derive(Default)]
struct RelayWatch {
    url: Option<String>,
    task: Option<JoinHandle<()>>,
}

/// Point the dashboard at `url` (or at nothing when `None`): abort the current watch, forget the
/// FMans and counts from the previous relay, and start watching the new one.
fn set_relay(state: &Arc<AppState>, url: Option<String>) {
    let mut relay = match state.relay.lock() {
        Ok(relay) => relay,
        Err(_) => return,
    };
    if let Some(task) = relay.task.take() {
        task.abort();
    }
    if let Ok(mut dashboard) = state.dashboard.lock() {
        *dashboard = Dashboard::default();
    }
    relay.url = url.clone();
    relay.task = url.map(|url| tokio::spawn(watch_relay(url, Arc::clone(state))));
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let (events, _) = broadcast::channel(1024);
    let state = Arc::new(AppState {
        dashboard: Mutex::new(Dashboard::default()),
        events,
        relay: Mutex::new(RelayWatch::default()),
    });

    set_relay(&state, args.relay);

    let app = Router::new()
        .route("/", get(async || Html(include_str!("index.html"))))
        .route("/events", get(sse))
        .route("/api/state", get(api_state))
        .route("/api/relay", post(set_relay_handler))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind dashboard on {addr}"))?;
    println!("devmon dashboard: http://{addr}");
    axum::serve(listener, app)
        .await
        .context("serve dashboard")?;
    Ok(())
}

/// Watch one relay and fan each event out to the browser.
///
/// An unreachable relay must not take the dashboard down: the page keeps serving whatever is
/// already known, and the SDK retries the connection underneath. The task runs until it is
/// aborted by the next [`set_relay`].
async fn watch_relay(url: String, state: Arc<AppState>) {
    let client = Client::default();
    if let Err(err) = client.add_relay(&url).await {
        eprintln!("devmon: cannot add relay {url}: {err}");
        return;
    }
    client.connect().await;

    // An empty filter matches every kind. The dashboard decides what is interesting, so a kind
    // nobody has taught it about still shows up in the feed instead of being silently dropped.
    if let Err(err) = client.subscribe(Filter::new(), None).await {
        eprintln!("devmon: subscribe failed: {err}");
        return;
    }

    let mut notifications = client.notifications();
    while let Ok(notification) = notifications.recv().await {
        let RelayPoolNotification::Event { event, .. } = notification else {
            continue;
        };
        ingest(&state, &event);
    }
}

/// Fold one event into the dashboard state and push it to connected browsers.
fn ingest(state: &AppState, event: &nostr_sdk::Event) {
    let kind = event.kind.as_u16();

    let content: serde_json::Value = serde_json::from_str(&event.content)
        .unwrap_or_else(|_| serde_json::Value::String(event.content.clone()));

    if kind == FMAN_ADVERTISEMENT_EVENT_KIND
        && let Some(row) = parse_advertisement(event, &content)
        && let Ok(mut dashboard) = state.dashboard.lock()
    {
        dashboard.fmans.insert(row.pubkey.clone(), row);
    }

    if let Ok(mut dashboard) = state.dashboard.lock() {
        *dashboard.kind_counts.entry(kind).or_default() += 1;
        dashboard.total_events += 1;
    }

    let payload = serde_json::json!({
        "id": event.id.to_hex(),
        "kind": kind,
        "kind_name": kind_name(kind),
        "author": event.pubkey.to_hex(),
        "created_at": event.created_at.as_secs(),
        "tags": event.tags.iter().map(|tag| tag.clone().to_vec()).collect::<Vec<_>>(),
        "content": content,
    });

    // A send error only means nobody has the page open.
    let _ = state.events.send(payload.to_string());
}

/// Pull the fields the roster needs out of an advertisement document.
///
/// The typed form (`AdvertisementDocument`) lives inside the fleet-manager daemon crate, so a
/// read-only observer cannot reuse it without depending on the whole daemon. Parse structurally
/// until that type gets a shared home.
fn parse_advertisement(event: &nostr_sdk::Event, content: &serde_json::Value) -> Option<FmanRow> {
    let payload = content.get("payload")?;

    let endpoints = payload
        .get("api_endpoints")
        .and_then(serde_json::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|endpoint| {
                    let transport = endpoint.get("transport")?.as_str()?;
                    let url = endpoint.get("url")?.as_str()?;
                    Some(format!("{transport}: {url}"))
                })
                .collect()
        })
        .unwrap_or_default();

    let plans = payload
        .get("plans")
        .and_then(serde_json::Value::as_array)
        .map(|list| list.iter().map(compact_plan).collect())
        .unwrap_or_default();

    Some(FmanRow {
        pubkey: event.pubkey.to_hex(),
        endpoints,
        plans,
        holder_authorizations: payload
            .get("holder_authorizations")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
            .unwrap_or_default(),
        expires_at: payload
            .get("expires_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        last_seen: now_secs(),
    })
}

/// Plans are serde-tagged; show the variant name rather than the whole blob.
fn compact_plan(plan: &serde_json::Value) -> String {
    match plan {
        serde_json::Value::String(name) => name.clone(),
        serde_json::Value::Object(map) => map
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "unknown".to_owned()),
        other => other.to_string(),
    }
}

async fn api_state(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let relay = state.relay.lock().ok().and_then(|relay| relay.url.clone());

    let Ok(dashboard) = state.dashboard.lock() else {
        return axum::Json(serde_json::json!({ "error": "dashboard state poisoned" }));
    };

    let kind_counts = dashboard
        .kind_counts
        .iter()
        .map(|(kind, count)| {
            serde_json::json!({ "kind": kind, "name": kind_name(*kind), "count": count })
        })
        .collect::<Vec<_>>();

    axum::Json(serde_json::json!({
        "relay": relay,
        "fmans": dashboard.fmans.values().cloned().collect::<Vec<_>>(),
        "kind_counts": kind_counts,
        "total_events": dashboard.total_events,
        "now": now_secs(),
    }))
}

#[derive(serde::Deserialize)]
struct SetRelayRequest {
    /// Relay to watch. An empty string stops watching and clears the view.
    url: String,
}

/// Point the dashboard at a relay chosen from the page.
async fn set_relay_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SetRelayRequest>,
) -> impl IntoResponse {
    let url = request.url.trim();
    let url = (!url.is_empty()).then(|| url.to_owned());
    set_relay(&state, url.clone());
    axum::Json(serde_json::json!({ "relay": url }))
}

async fn sse(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>> {
    // A lagging browser drops events rather than stalling the broadcast for everyone else.
    let stream = BroadcastStream::new(state.events.subscribe())
        .filter_map(|payload| async move { payload.ok() })
        .map(|payload| Ok(SseEvent::default().data(payload)));
    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}
