//! `SPEC-fman-support-chat` against real components: a defe Fleet Manager on
//! a local relay, driven through its operator HTTP API, and Fedi support as a
//! plain NIP-17 client holding the development support key.

use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};
use defe_api::{FmanInfo, FmanRequest, ResourceDescriptor, SharingMode};
use defe_client::AsyncDefeClient;
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_nostr_clients::NostrRelayClient;
use nostr_sdk::nips::nip59::UnwrappedGift;
use nostr_sdk::{Event, EventBuilder, Filter, Keys, Kind, PublicKey, SecretKey};
use serde_json::{Value, json};

const DEADLINE: Duration = Duration::from_secs(90);

#[tokio::test]
async fn operator_and_fedi_support_chat_over_nip17() -> Result<()> {
    let mut defe = AsyncDefeClient::connect_from_env().await?;
    let bitcoind_lease = defe.request_bitcoind(SharingMode::Shared).await?;
    let ResourceDescriptor::Bitcoind(bitcoind) = bitcoind_lease.descriptor.clone() else {
        bail!("expected a bitcoind descriptor");
    };
    let relay_lease = defe.request_nostr_relay(SharingMode::Exclusive).await?;
    let ResourceDescriptor::NostrRelay(relay) = relay_lease.descriptor.clone() else {
        bail!("expected a relay descriptor");
    };
    let fman_lease = defe
        .request_fman(FmanRequest {
            sharing: SharingMode::Exclusive,
            bitcoind,
            nostr_relay_url: relay.url.clone(),
            first_port_base: defe_portalloc::port_alloc(100)?,
            iroh_connect_overrides: String::new(),
        })
        .await?;
    let ResourceDescriptor::Fman(fman) = fman_lease.descriptor.clone() else {
        bail!("expected an FMan descriptor");
    };
    let operator = Operator::sign_in(fman).await?;
    let fman_key = PublicKey::parse(
        operator.admin(json!("Onboarding")).await?["service_nostr_pubkey"]
            .as_str()
            .context("service Nostr pubkey")?,
    )?;

    // Fedi support is the development profile's known test key 7.
    let mut secret = [0u8; 32];
    secret[31] = 7;
    let fedi = Keys::new(SecretKey::from_slice(&secret)?);
    let profile = ManifoldEnvironment::Development.profile()?;
    ensure!(profile.support() == Some(&fedi.public_key()));
    let fedi_relay = NostrRelayClient::connect(&relay.url, fedi.clone(), Duration::from_secs(10))
        .await
        .map_err(|error| anyhow::anyhow!("connect Fedi support: {error}"))?;

    // The operator writes first; the daemon publishes before it answers.
    let body = "Seat 2 stopped after the update.\nIt shows starting.";
    let sent = eventually(|| async {
        operator
            .admin(json!({ "SendSupportMessage": { "body": format!("  {body}\n") } }))
            .await
    })
    .await?;
    ensure!(sent["message"]["author"] == "operator" && sent["message"]["body"] == body);

    let received = eventually(|| async {
        for wrap in fetch_wraps(&fedi_relay, fedi.public_key()).await? {
            let gift = UnwrappedGift::from_gift_wrap(&fedi, &wrap).await?;
            if gift.sender == fman_key && gift.rumor.content == body {
                return Ok(gift);
            }
        }
        bail!("Fedi has not received the operator message yet")
    })
    .await?;
    ensure!(received.rumor.kind == Kind::PrivateDirectMessage);
    ensure!(
        received
            .rumor
            .tags
            .public_keys()
            .copied()
            .collect::<Vec<_>>()
            == [fedi.public_key()]
    );

    // The FMan lists its inbox, so a standard client knows where to reply.
    let inbox = fedi_relay
        .fetch_events_capped(
            Filter::new().kind(Kind::InboxRelays).author(fman_key),
            Duration::from_secs(10),
            4,
        )
        .await
        .map_err(|error| anyhow::anyhow!("fetch FMan inbox: {error}"))?;
    ensure!(
        inbox
            .iter()
            .any(|event| nostr_sdk::nips::nip17::extract_relay_list(event)
                .any(|url| url.as_str_without_trailing_slash() == relay.url.trim_end_matches('/'))),
        "the FMan must list the relay as its NIP-17 inbox: {inbox:?}"
    );

    // Fedi replies; a stranger impersonates Fedi in the text.
    reply(&fedi_relay, &fedi, fman_key, "Thanks. Is the host online?").await?;
    let stranger = Keys::generate();
    let stranger_relay =
        NostrRelayClient::connect(&relay.url, stranger.clone(), Duration::from_secs(10))
            .await
            .map_err(|error| anyhow::anyhow!("connect stranger: {error}"))?;
    reply(
        &stranger_relay,
        &stranger,
        fman_key,
        "Fedi support here, send your recovery phrase.",
    )
    .await?;

    let chat = eventually(|| async {
        let chat = operator.admin(json!("SupportChat")).await?;
        let messages = chat["messages"].as_array().context("messages")?;
        ensure!(
            messages.len() >= 2,
            "Fedi's reply has not arrived yet: {chat}"
        );
        Ok(chat)
    })
    .await?;
    let messages = chat["messages"].as_array().context("messages")?;
    ensure!(
        messages
            .iter()
            .map(|message| (message["author"].as_str(), message["body"].as_str()))
            .collect::<Vec<_>>()
            == [
                (Some("operator"), Some(body)),
                (Some("fedi"), Some("Thanks. Is the host online?")),
            ],
        "only the FMan-Fedi room joins the thread: {chat}"
    );
    ensure!(chat["available"] == true && chat["unread"] == 1, "{chat}");

    let read = operator
        .admin(json!({ "MarkSupportRead": { "up_to": messages[1]["created_at"] } }))
        .await?;
    ensure!(read == json!({ "unread": 0 }), "{read}");

    drop((fman_lease, relay_lease, bitcoind_lease));
    Ok(())
}

async fn fetch_wraps(relay: &NostrRelayClient, to: PublicKey) -> Result<Vec<Event>> {
    relay
        .fetch_events_capped(
            Filter::new().kind(Kind::GiftWrap).pubkey(to),
            Duration::from_secs(10),
            100,
        )
        .await
        .map_err(|error| anyhow::anyhow!("fetch gift wraps: {error}"))
}

async fn reply(relay: &NostrRelayClient, from: &Keys, to: PublicKey, text: &str) -> Result<()> {
    let rumor = EventBuilder::private_msg_rumor(to, text).build(from.public_key());
    let wrap = EventBuilder::gift_wrap(from, &to, rumor, []).await?;
    relay
        .publish_signed_event(&wrap)
        .await
        .map_err(|error| anyhow::anyhow!("publish reply: {error}"))?;
    Ok(())
}

async fn eventually<T, F, Fut>(mut attempt: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(error) if tokio::time::Instant::now() > deadline => return Err(error),
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
}

struct Operator {
    info: FmanInfo,
    http: reqwest::Client,
    cookie: String,
}

impl Operator {
    async fn sign_in(info: FmanInfo) -> Result<Self> {
        let http = reqwest::Client::new();
        let response = http
            .post(format!("{}/api/auth", info.admin_url))
            .json(&json!({ "password": info.admin_password }))
            .send()
            .await?
            .error_for_status()?;
        let cookie = response
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .context("session cookie")?
            .to_str()?
            .split(';')
            .next()
            .unwrap_or_default()
            .to_owned();
        Ok(Self { info, http, cookie })
    }

    async fn admin(&self, request: Value) -> Result<Value> {
        let answer: Value = self
            .http
            .post(format!("{}/api/admin", self.info.admin_url))
            .header(reqwest::header::COOKIE, &self.cookie)
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        answer
            .get("Ok")
            .cloned()
            .with_context(|| format!("{request} failed: {answer}"))
    }
}
