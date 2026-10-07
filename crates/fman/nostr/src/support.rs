//! The operator's chat with Fedi support over NIP-17 private messages
//! ([`SPEC-fman-support-chat`](../../specs/SPEC-fman-support-chat.md)).
//!
//! This module owns the chat: the support admin verbs, which core forwards
//! unchanged, and the relay side. The thread is stored in the fleet database.
//!
//! The FMan writes as its service key and reads the gift wraps addressed to
//! it on the environment's canonical relays, which it also lists as its
//! kind-10050 inbox. Fedi support is the key the admitted setup-payment
//! policy names. A message joins the thread only when its seal is signed by
//! Fedi support or by this FMan and the rumor's room is exactly the two of
//! them.

use std::pin::pin;
use std::sync::Arc;

use anyhow::Context as _;
use fedi_decentralized_nostr_clients::NostrRelayClient;
use fman_core::admin::AdminRequest;
use fman_core::db::SupportRow;
use futures_util::StreamExt as _;
use nostr_sdk::nips::nip59::UnwrappedGift;
use nostr_sdk::{Event, EventBuilder, EventId, Filter, Keys, Kind, PublicKey, Tag, TagKind};
use serde_json::{Value, json};

use crate::{Inner, REQUEST_TIMEOUT};

/// Longest operator message, in characters.
pub const MAX_SUPPORT_MESSAGE_CHARS: usize = 4000;

/// Gift wraps the inbox subscription asks each relay to replay. This bounds
/// what a new install or a new support key reads back.
const INBOX_LIMIT: usize = 500;

/// Answer one of the support admin verbs.
pub(crate) async fn answer(inner: &Inner, request: AdminRequest) -> anyhow::Result<Value> {
    match request {
        AdminRequest::SupportChat => Ok(support_chat_json(
            inner.support().is_some(),
            &inner.db.support_messages().await?,
            inner.db.support_unread().await?,
        )),
        AdminRequest::SendSupportMessage { body } => {
            Ok(support_message_json(&send(inner, &body).await?))
        }
        AdminRequest::MarkSupportRead { ids } => {
            inner.db.mark_support_read(&ids).await?;
            Ok(support_read_json(inner.db.support_unread().await?))
        }
        _ => anyhow::bail!("not a support chat request"),
    }
}

pub fn support_chat_json(available: bool, messages: &[SupportRow], unread: u64) -> Value {
    json!({
        "available": available,
        "messages": messages.iter().map(message_json).collect::<Vec<_>>(),
        "unread": unread,
    })
}

pub fn support_message_json(message: &SupportRow) -> Value {
    json!({ "message": message_json(message) })
}

pub fn support_read_json(unread: u64) -> Value {
    json!({ "unread": unread })
}

fn message_json(message: &SupportRow) -> Value {
    json!({
        "id": message.rumor_id,
        "author": if message.from_fedi { "fedi" } else { "operator" },
        "body": message.body,
        "created_at": message.created_at,
        "unread": message.unread,
    })
}

async fn send(inner: &Inner, body: &str) -> anyhow::Result<SupportRow> {
    let body = body.trim();
    if body.is_empty() {
        anyhow::bail!("Write a message first.");
    }
    if body.chars().count() > MAX_SUPPORT_MESSAGE_CHARS {
        anyhow::bail!("A message can have at most {MAX_SUPPORT_MESSAGE_CHARS} characters.");
    }
    let fedi = inner
        .support()
        .context("Fedi support chat is not available for this deployment yet.")?;
    let nostr = inner
        .relays
        .get()
        .context("The Nostr relays are not connected yet. Try again in a minute.")?;
    let me = inner.keys.public_key();
    let mut rumor = EventBuilder::private_msg_rumor(fedi, body).build(me);
    let id = rumor.id();
    let to_fedi = EventBuilder::gift_wrap(&inner.keys, &fedi, rumor.clone(), []).await?;
    let to_self = EventBuilder::gift_wrap(&inner.keys, &me, rumor.clone(), []).await?;
    nostr.publish_signed_event(&to_fedi).await.map_err(|err| {
        tracing::warn!(error = %err, "publish support message failed");
        anyhow::anyhow!("No Nostr relay accepted the message. Try again.")
    })?;
    // A publish waits for every relay's answer or its acknowledgement
    // timeout. The copy to ourselves only restores the thread after a
    // reinstall, so it goes out in the background rather than adding a second
    // wait that could pass the dashboard's request deadline. It starts only
    // after Fedi's copy is accepted, so the inbox never shows a message that
    // Fedi did not get.
    let own_copy = nostr.clone();
    tokio::spawn(async move {
        if let Err(err) = own_copy.publish_signed_event(&to_self).await {
            tracing::warn!(error = %err, "publish own copy of support message failed");
        }
    });
    let message = SupportRow {
        rumor_id: id.to_hex(),
        from_fedi: false,
        body: rumor.content,
        created_at: rumor.created_at.as_secs(),
        unread: false,
    };
    inner.db.record_support_message(&message).await?;
    Ok(message)
}

/// Keep the stored thread current with one subscription per support key. It
/// asks for the newest wraps addressed to this FMan and then stays open for new
/// ones. The SDK sends it again when a relay reconnects, and the relay then
/// replays its newest wraps, so a message sent while the relay was away still
/// arrives. The database stores each message once.
pub(crate) async fn run_inbox(inner: Arc<Inner>, nostr: NostrRelayClient) {
    let addressed_to_me = Filter::new()
        .kind(Kind::GiftWrap)
        .pubkey(inner.keys.public_key())
        .limit(INBOX_LIMIT);
    let mut policy = inner.setup_payment_federations.subscribe();
    loop {
        // The policy can name Fedi support late, or a new key later. A new
        // subscription reads the history again for the new key.
        let fedi = loop {
            if let Some(fedi) = inner.support() {
                break fedi;
            }
            if policy.changed().await.is_err() {
                return;
            }
        };
        if let Err(err) = nostr.publish_event(inbox_relays(&inner)).await {
            tracing::warn!(error = %err, "publish support inbox relays failed");
        }
        let live = loop {
            match nostr.subscribe(addressed_to_me.clone()).await {
                Ok(live) => break live,
                Err(err) => {
                    tracing::warn!(error = %err, "subscribe to support messages failed");
                    tokio::time::sleep(REQUEST_TIMEOUT).await;
                }
            }
        };
        let mut live = pin!(live);
        while inner.support() == Some(fedi) {
            tokio::select! {
                Some(event) = live.next() => {
                    if let Some(message) = admit(&inner.keys, fedi, &event).await
                        && let Err(err) = inner.db.record_support_message(&message).await
                    {
                        tracing::warn!(?err, "record support message failed");
                    }
                }
                changed = policy.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// This FMan's NIP-17 inbox: the relays it reads.
fn inbox_relays(inner: &Inner) -> EventBuilder {
    EventBuilder::new(Kind::InboxRelays, "").tags(
        inner
            .manifold_environment
            .nostr_relays()
            .as_urls()
            .iter()
            .map(|relay| Tag::custom(TagKind::Relay, [relay.to_string()])),
    )
}

/// The thread message a gift wrap carries, if it belongs to the thread.
pub(crate) async fn admit(keys: &Keys, fedi: PublicKey, event: &Event) -> Option<SupportRow> {
    let UnwrappedGift { sender, rumor } = UnwrappedGift::from_gift_wrap(keys, event).await.ok()?;
    if rumor.kind != Kind::PrivateDirectMessage {
        return None;
    }
    let me = keys.public_key();
    let recipients = rumor.tags.public_keys().copied().collect::<Vec<_>>();
    let from_fedi = if sender == fedi && recipients == [me] {
        true
    } else if sender == me && recipients == [fedi] {
        false
    } else {
        return None;
    };
    // The id a rumor claims is unsigned; derive it.
    let id = EventId::new(
        &rumor.pubkey,
        &rumor.created_at,
        &rumor.kind,
        &rumor.tags,
        &rumor.content,
    );
    Some(SupportRow {
        rumor_id: id.to_hex(),
        from_fedi,
        body: rumor.content,
        created_at: rumor.created_at.as_secs(),
        unread: from_fedi,
    })
}
