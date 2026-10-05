//! NIP-17 private messages between this FMan and Fedi support
//! ([`SPEC-fman-support-chat`](../../specs/SPEC-fman-support-chat.md)).
//!
//! The FMan writes as its service key and reads the gift wraps addressed to
//! it on the environment's canonical relays, which it also lists as its
//! kind-10050 inbox. A message joins the thread only when its seal is signed
//! by Fedi support or by this FMan and the rumor's room is exactly the two
//! of them.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use fedi_decentralized_nostr_clients::NostrRelayClient;
use fman_core::support::{SupportAuthor, SupportMessage};
use nostr_sdk::nips::nip59::{RANGE_RANDOM_TIMESTAMP_TWEAK, UnwrappedGift};
use nostr_sdk::{
    Event, EventBuilder, EventId, Filter, Keys, Kind, PublicKey, Tag, TagKind, Timestamp,
};

use crate::{Inner, REQUEST_TIMEOUT};

const POLL_INTERVAL: Duration = Duration::from_secs(10);
const LOCAL_E2E_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Gift wraps read per poll. The first poll after start reads the newest
/// ones of the whole history; later polls read only the recent window.
const FETCH_LIMIT: u16 = 500;

/// Gift wraps backdate their `created_at` by up to two days (NIP-59), so a
/// poll reaches back that far, with an hour for clock skew.
const BACKDATE_WINDOW_SECS: u64 = RANGE_RANDOM_TIMESTAMP_TWEAK.end + 60 * 60;

pub(crate) async fn send(inner: &Inner, body: String) -> anyhow::Result<SupportMessage> {
    let fedi = inner
        .support
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
    // The copy to ourselves only restores the thread after a reinstall.
    if let Err(err) = nostr.publish_signed_event(&to_self).await {
        tracing::warn!(error = %err, "publish own copy of support message failed");
    }
    let message = SupportMessage {
        id: id.to_hex(),
        author: SupportAuthor::Operator,
        body: rumor.content,
        created_at: rumor.created_at.as_secs(),
    };
    inner.support_store.record(&message).await?;
    Ok(message)
}

pub(crate) async fn run_inbox(inner: Arc<Inner>, nostr: NostrRelayClient) {
    let Some(fedi) = inner.support else {
        return;
    };
    let poll_interval = if std::env::var_os("FMAN_E2E_LOCAL_IROH").is_some() {
        LOCAL_E2E_POLL_INTERVAL
    } else {
        POLL_INTERVAL
    };
    let mut inbox_listed = false;
    let mut seen = HashSet::<EventId>::new();
    let mut since = None;
    loop {
        if !inbox_listed {
            match nostr.publish_event(inbox_relays(&inner)).await {
                Ok(_) => inbox_listed = true,
                Err(err) => tracing::warn!(error = %err, "publish support inbox relays failed"),
            }
        }
        let started = Timestamp::now();
        let mut filter = Filter::new()
            .kind(Kind::GiftWrap)
            .pubkey(inner.keys.public_key());
        if let Some(since) = since {
            filter = filter.since(since);
        }
        match nostr
            .fetch_events_capped(filter, REQUEST_TIMEOUT, FETCH_LIMIT)
            .await
        {
            Ok(events) => {
                for event in events {
                    if !seen.insert(event.id) {
                        continue;
                    }
                    let Some(message) = admit(&inner.keys, fedi, &event).await else {
                        continue;
                    };
                    if let Err(err) = inner.support_store.record(&message).await {
                        tracing::warn!(?err, "record support message failed");
                        seen.remove(&event.id);
                    }
                }
                since = Some(Timestamp::from(
                    started.as_secs().saturating_sub(BACKDATE_WINDOW_SECS),
                ));
            }
            Err(err) => tracing::warn!(error = %err, "fetch support messages failed"),
        }
        tokio::time::sleep(poll_interval).await;
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
pub(crate) async fn admit(keys: &Keys, fedi: PublicKey, event: &Event) -> Option<SupportMessage> {
    let UnwrappedGift { sender, rumor } = UnwrappedGift::from_gift_wrap(keys, event).await.ok()?;
    if rumor.kind != Kind::PrivateDirectMessage {
        return None;
    }
    let me = keys.public_key();
    let recipients = rumor.tags.public_keys().copied().collect::<Vec<_>>();
    let author = if sender == fedi && recipients == [me] {
        SupportAuthor::Fedi
    } else if sender == me && recipients == [fedi] {
        SupportAuthor::Operator
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
    Some(SupportMessage {
        id: id.to_hex(),
        author,
        body: rumor.content,
        created_at: rumor.created_at.as_secs(),
    })
}
