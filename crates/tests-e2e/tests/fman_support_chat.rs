//! `SPEC-fman-support-chat` against real components: a defe Fleet Manager on
//! a local relay, driven through its operator HTTP API, and Fedi support as a
//! plain NIP-17 client holding the profile key, then the key a newer
//! setup-payment policy names.

use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};
use defe_api::{FmanInfo, FmanRequest, ResourceDescriptor, SharingMode};
use defe_client::AsyncDefeClient;
use fedi_decentralized_nostr::setup_payment_federations::{
    SETUP_PAYMENT_FEDERATIONS_D_TAG, SETUP_PAYMENT_FEDERATIONS_EVENT_KIND,
};
use fedi_decentralized_nostr_clients::NostrRelayClient;
use nostr_sdk::nips::nip44;
use nostr_sdk::nips::nip59::UnwrappedGift;
use nostr_sdk::{
    Event, EventBuilder, Filter, JsonUtil as _, Keys, Kind, PublicKey, Tag, Timestamp,
};
use serde_json::{Value, json};

const DEADLINE: Duration = Duration::from_secs(90);

/// The Fedi support key the development profile pins.
const DEVELOPMENT_SUPPORT_SECRET: &str =
    "0000000000000000000000000000000000000000000000000000000000000007";

/// The setup-payment publisher every defe Fleet Manager trusts.
const SETUP_PAYMENT_PUBLISHER_SECRET: &str =
    "0000000000000000000000000000000000000000000000000000000000000001";

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

    // No policy names Fedi support yet, so the chat uses the key the
    // development profile pins: the publicly known test secret 7.
    let chat = operator.admin(json!("SupportChat")).await?;
    ensure!(chat["available"] == true, "{chat}");
    let fedi = Keys::parse(DEVELOPMENT_SUPPORT_SECRET)?;

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
    eventually(|| async {
        let inbox = fedi_relay
            .fetch_events_capped(
                Filter::new().kind(Kind::InboxRelays).author(fman_key),
                Duration::from_secs(10),
                4,
            )
            .await
            .map_err(|error| anyhow::anyhow!("fetch FMan inbox: {error}"))?;
        ensure!(
            inbox.iter().any(|event| {
                nostr_sdk::nips::nip17::extract_relay_list(event).any(|url| {
                    url.as_str_without_trailing_slash() == relay.url.trim_end_matches('/')
                })
            }),
            "the FMan must list the relay as its NIP-17 inbox: {inbox:?}"
        );
        Ok(())
    })
    .await?;

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
        .admin(json!({ "MarkSupportRead": { "up_to": messages[1]["id"] } }))
        .await?;
    ensure!(read == json!({ "unread": 0 }), "{read}");

    // Fedi rotates its support key with a newer policy. The new key's
    // replies join the stored thread once the FMan admits the policy; the
    // old key's later message stays out.
    let rotated = Keys::generate();
    let rotated_relay =
        NostrRelayClient::connect(&relay.url, rotated.clone(), Duration::from_secs(10))
            .await
            .map_err(|error| anyhow::anyhow!("connect rotated Fedi support: {error}"))?;
    let publisher = NostrRelayClient::connect(
        &relay.url,
        Keys::parse(SETUP_PAYMENT_PUBLISHER_SECRET)?,
        Duration::from_secs(10),
    )
    .await
    .map_err(|error| anyhow::anyhow!("connect setup-payment publisher: {error}"))?;
    // Older than the live subscription reaches back, so only the read-back of
    // history the key change starts can bring it in.
    let three_days_ago = Timestamp::now() - Duration::from_secs(3 * 24 * 60 * 60);
    reply_at(
        &rotated_relay,
        &rotated,
        fman_key,
        "From before the rotation.",
        three_days_ago,
    )
    .await?;
    publish_policy(&publisher, rotated.public_key()).await?;
    reply(&rotated_relay, &rotated, fman_key, "New key here.").await?;
    let thread_len = |chat: &Value| chat["messages"].as_array().map_or(0, Vec::len);
    eventually(|| async {
        let chat = operator.admin(json!("SupportChat")).await?;
        ensure!(
            thread_len(&chat) >= 4,
            "the new key's replies has not arrived yet: {chat}"
        );
        Ok(())
    })
    .await?;
    // Published before the next reply, so the FMan judges it against the new
    // key before the reply arrives.
    reply(&fedi_relay, &fedi, fman_key, "Old key, after the rotation.").await?;
    reply(&rotated_relay, &rotated, fman_key, "Still the new key.").await?;
    let chat = eventually(|| async {
        let chat = operator.admin(json!("SupportChat")).await?;
        ensure!(
            thread_len(&chat) >= 5,
            "the second reply has not arrived yet: {chat}"
        );
        Ok(chat)
    })
    .await?;
    let bodies = chat["messages"]
        .as_array()
        .context("messages")?
        .iter()
        .map(|message| message["body"].as_str())
        .collect::<Vec<_>>();
    ensure!(
        bodies
            == [
                Some("From before the rotation."),
                Some(body),
                Some("Thanks. Is the host online?"),
                Some("New key here."),
                Some("Still the new key."),
            ],
        "{chat}"
    );
    // Every Fedi message stored after the mark is unread, including the one
    // that sorts before it.
    ensure!(chat["unread"] == 3, "{chat}");

    let after = "Thanks, new key.";
    operator
        .admin(json!({ "SendSupportMessage": { "body": after } }))
        .await?;
    eventually(|| async {
        for wrap in fetch_wraps(&rotated_relay, rotated.public_key()).await? {
            let gift = UnwrappedGift::from_gift_wrap(&rotated, &wrap).await?;
            if gift.sender == fman_key && gift.rumor.content == after {
                return Ok(());
            }
        }
        bail!("the new key has not received the operator message yet")
    })
    .await?;

    drop((fman_lease, relay_lease, bitcoind_lease));
    Ok(())
}

/// Publish the setup-payment policy, naming `support` as Fedi support.
async fn publish_policy(publisher: &NostrRelayClient, support: PublicKey) -> Result<()> {
    let content = json!({
        "version": 1,
        "fman_version": "0.1.0",
        "federations": [],
        "telemetry_registration_url": "https://push.fedi.example/v1/telemetry/registrations",
        "support_nostr_pubkey": support.to_hex(),
    });
    publisher
        .publish_event(
            EventBuilder::new(
                Kind::Custom(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND),
                content.to_string(),
            )
            .tag(Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG)),
        )
        .await
        .map_err(|error| anyhow::anyhow!("publish setup-payment policy: {error}"))?;
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

/// A reply whose rumor, seal, and gift wrap all carry `at`.
async fn reply_at(
    relay: &NostrRelayClient,
    from: &Keys,
    to: PublicKey,
    text: &str,
    at: Timestamp,
) -> Result<()> {
    let rumor = EventBuilder::private_msg_rumor(to, text)
        .custom_created_at(at)
        .build(from.public_key());
    let seal = EventBuilder::seal(from, &to, rumor)
        .await?
        .custom_created_at(at)
        .sign_with_keys(from)?;
    let ephemeral = Keys::generate();
    let content = nip44::encrypt(
        ephemeral.secret_key(),
        &to,
        seal.as_json(),
        nip44::Version::default(),
    )?;
    let wrap = EventBuilder::new(Kind::GiftWrap, content)
        .tag(Tag::public_key(to))
        .custom_created_at(at)
        .sign_with_keys(&ephemeral)?;
    relay
        .publish_signed_event(&wrap)
        .await
        .map_err(|error| anyhow::anyhow!("publish backdated reply: {error}"))?;
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
