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

use std::collections::HashSet;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use fedi_decentralized_nostr_clients::NostrRelayClient;
use fman_core::admin::AdminRequest;
use fman_core::db::SupportRow;
use futures_util::StreamExt as _;
use nostr_sdk::nips::nip59::{RANGE_RANDOM_TIMESTAMP_TWEAK, UnwrappedGift};
use nostr_sdk::{
    Event, EventBuilder, EventId, Filter, Keys, Kind, PublicKey, Tag, TagKind, Timestamp,
};
use serde_json::{Value, json};

use crate::{Inner, REQUEST_TIMEOUT};

/// Longest operator message, in characters.
pub const MAX_SUPPORT_MESSAGE_CHARS: usize = 4000;

/// The live subscription delivers new messages. This fetch only fills what
/// it missed while a relay was away.
const CATCH_UP_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Gift wraps read per fetch. The first fetch for a support key reads the
/// newest ones of the whole history; later ones only the recent window.
const FETCH_LIMIT: u16 = 500;

/// Memory bound for one fetch. Reaching it ends the fetch like the count does.
const FETCH_MAX_BYTES: usize = 8 * 1024 * 1024;

/// Gift wrap ids kept to skip judging a repeat.
const MAX_SEEN_WRAPS: usize = 10_000;

/// Gift wraps backdate their `created_at` by up to two days (NIP-59), so a
/// catch-up fetch reaches back that far, with an hour for clock skew.
const BACKDATE_WINDOW_SECS: u64 = RANGE_RANDOM_TIMESTAMP_TWEAK.end + 60 * 60;

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
        AdminRequest::MarkSupportRead { up_to } => {
            inner.db.mark_support_read(up_to).await?;
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
    };
    inner.db.record_support_message(&message).await?;
    Ok(message)
}

/// Keep the stored thread current: a live subscription for new gift wraps,
/// and a periodic catch-up fetch for what the subscription missed.
pub(crate) async fn run_inbox(inner: Arc<Inner>, nostr: NostrRelayClient) {
    let addressed_to_me = Filter::new()
        .kind(Kind::GiftWrap)
        .pubkey(inner.keys.public_key());
    // History is the catch-up fetch's job, but `limit(0)` is not honored by
    // every relay, and the client rejects what such a relay sends. A
    // backdated wrap created from now on is never older than this.
    let live_since = Timestamp::now()
        .as_secs()
        .saturating_sub(BACKDATE_WINDOW_SECS);
    let live = loop {
        match nostr
            .subscribe(addressed_to_me.clone().since(Timestamp::from(live_since)))
            .await
        {
            Ok(live) => break live,
            Err(err) => {
                tracing::warn!(error = %err, "subscribe to support messages failed");
                tokio::time::sleep(REQUEST_TIMEOUT).await;
            }
        }
    };
    let mut live = pin!(live);
    let mut policy = inner.setup_payment_federations.subscribe();
    let mut catch_up = tokio::time::interval(CATCH_UP_INTERVAL);
    let mut inbox_listed = false;
    let mut seen = HashSet::<EventId>::new();
    let mut since = None;
    let mut admitted_for = None;
    loop {
        // The policy can name Fedi support late, or a new key later. Wraps
        // judged against another key are judged again, from the start.
        let fedi = inner.support();
        if fedi != admitted_for {
            admitted_for = fedi;
            seen.clear();
            since = None;
            catch_up.reset_immediately();
        }
        tokio::select! {
            Some(event) = live.next() => {
                if let Some(fedi) = fedi {
                    judge(&inner, fedi, &mut seen, event).await;
                }
            }
            _ = catch_up.tick() => {
                let Some(fedi) = fedi else { continue };
                if !inbox_listed {
                    match nostr.publish_event(inbox_relays(&inner)).await {
                        Ok(_) => inbox_listed = true,
                        Err(err) => {
                            tracing::warn!(error = %err, "publish support inbox relays failed");
                        }
                    }
                }
                let started = Timestamp::now();
                // `limit` asks each relay for its newest wraps rather than the
                // first ones it finds.
                let mut filter = addressed_to_me.clone().limit(usize::from(FETCH_LIMIT));
                if let Some(since) = since {
                    filter = filter.since(since);
                }
                // Only a complete answer moves `since`: a fetch that a relay
                // outage cut short is tried again from the same point.
                match nostr
                    .fetch_events_complete_or_capped(
                        filter,
                        tokio::time::Instant::now() + REQUEST_TIMEOUT,
                        FETCH_LIMIT,
                        FETCH_MAX_BYTES,
                    )
                    .await
                {
                    Ok(events) => {
                        for event in events {
                            judge(&inner, fedi, &mut seen, event).await;
                        }
                        since = Some(Timestamp::from(
                            started.as_secs().saturating_sub(BACKDATE_WINDOW_SECS),
                        ));
                    }
                    Err(err) => tracing::warn!(error = %err, "fetch support messages failed"),
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

/// Store the thread message a gift wrap carries, once.
async fn judge(inner: &Inner, fedi: PublicKey, seen: &mut HashSet<EventId>, event: Event) {
    // Anyone can address gift wraps to this FMan, so the dedupe set is
    // bounded. Forgetting it only costs judging a wrap again; the database
    // stores each message once.
    if seen.len() >= MAX_SEEN_WRAPS {
        seen.clear();
    }
    if !seen.insert(event.id) {
        return;
    }
    let Some(message) = admit(&inner.keys, fedi, &event).await else {
        return;
    };
    if let Err(err) = inner.db.record_support_message(&message).await {
        tracing::warn!(?err, "record support message failed");
        seen.remove(&event.id);
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
    })
}
