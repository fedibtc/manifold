//! Relay publication for the provider advertisement.
//!
//! [`RelayPublisher`] is the seam: production publishes to the configured
//! relays, and the test publishers stand in for one without a relay.

use std::time::Duration;

use async_trait::async_trait;
pub use fedi_decentralized_nostr::flip::{
    FLIP_PROVIDER_ADVERTISEMENT_D_TAG, FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND,
    FLIP_PROVIDER_ADVERTISEMENT_HASHTAG,
};
use fedi_decentralized_service_liquidity_manager::{Timestamp, Url};
use nostr_sdk::nips::nip01::Coordinate;
use nostr_sdk::nips::nip09::EventDeletionRequest;
use nostr_sdk::{Client, EventBuilder, Keys, Kind, RelayUrl, Tag};

const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Request to publish the current ready advertisement to one relay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayPublishRequest {
    /// Relay URL.
    pub relay_url: Url,

    /// Canonical signed advertisement document JSON.
    pub content: String,

    /// Event `created_at`, pinned to the advertisement `issued_at`.
    ///
    /// Replaceable-event ordering on relays uses `created_at` with a
    /// lowest-event-id tiebreak for equal timestamps (NIP-01), so stamping
    /// publish-time "now" lets a republish that lands in the same second as an
    /// earlier publication lose the tiebreak and be silently discarded.
    /// Deriving `created_at` from the signed payload makes the newer
    /// advertisement always replace the older one, and makes a byte-identical
    /// republish produce a byte-identical event.
    pub created_at: Timestamp,

    /// Hex Nostr secret key whose public key equals the advertisement provider key.
    pub nostr_secret_key_hex: String,
}

/// Publication result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayPublishResult {
    /// Relay-accepted event id.
    pub event_id: String,
}

/// Request to withdraw the published advertisement from one relay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayWithdrawRequest {
    /// Relay URL.
    pub relay_url: Url,

    /// Optional operator-readable reason.
    pub reason: Option<String>,

    /// Hex Nostr secret key whose public key equals the advertisement provider key.
    ///
    /// Both halves of a withdrawal need it: the superseding event must be
    /// authored by the same identity to share the addressable coordinate, and a
    /// deletion request is only honoured when signed by the author of the event
    /// it names.
    pub nostr_secret_key_hex: String,

    /// Canonical signed advertisement JSON that already carries a passed `expires_at`.
    ///
    /// Deleting is a request a relay may decline; replacing an addressable event
    /// is not. Publishing this under the live advertisement's coordinate is what
    /// makes the withdrawal effective everywhere, and its content stays a
    /// verifiable signed document rather than a bare tombstone so that a client
    /// which fetches it rejects it on the same freshness rule it already applies.
    pub expired_content: String,

    /// Event `created_at` for the superseding event, strictly after the
    /// advertisement it replaces so replaceable-event ordering cannot keep the
    /// live one on an equal-timestamp lowest-id tiebreak (NIP-01).
    pub expired_created_at: Timestamp,
}

/// Relay publication backend boundary.
#[async_trait]
pub trait RelayPublisher: Send + Sync {
    /// Publish a signed provider advertisement.
    async fn publish(&self, request: RelayPublishRequest) -> Result<RelayPublishResult, String>;

    /// Ask one relay to stop serving this provider's advertisement.
    async fn withdraw(&self, request: RelayWithdrawRequest) -> Result<(), String>;
}

/// Provisional Nostr-backed relay publisher.
#[derive(Clone, Debug, Default)]
pub struct NostrRelayPublisher;

#[async_trait]
impl RelayPublisher for NostrRelayPublisher {
    async fn publish(&self, request: RelayPublishRequest) -> Result<RelayPublishResult, String> {
        let keys = Keys::parse(&request.nostr_secret_key_hex).map_err(|err| err.to_string())?;
        let client = Client::new(keys);
        let relay_url = RelayUrl::parse(&request.relay_url.0).map_err(|err| err.to_string())?;
        client
            .add_relay(relay_url)
            .await
            .map_err(|err| err.to_string())?;
        let output = client.try_connect(RELAY_CONNECT_TIMEOUT).await;
        if output.success.is_empty() {
            return Err("no relay connection succeeded".to_owned());
        }

        let builder = EventBuilder::new(
            Kind::Custom(FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND),
            request.content,
        )
        .custom_created_at(nostr_sdk::Timestamp::from_secs(request.created_at.0))
        .tags([
            tag(["d", FLIP_PROVIDER_ADVERTISEMENT_D_TAG]),
            tag(["t", FLIP_PROVIDER_ADVERTISEMENT_HASHTAG]),
        ]);
        let output = client
            .send_event_builder(builder)
            .await
            .map_err(|err| err.to_string())?;
        if !output.failed.is_empty() {
            return Err(format!("{:?}", output.failed));
        }
        if output.success.is_empty() {
            return Err("no relay accepted the event".to_owned());
        }

        Ok(RelayPublishResult {
            event_id: output.id().to_string(),
        })
    }

    async fn withdraw(&self, request: RelayWithdrawRequest) -> Result<(), String> {
        let keys = Keys::parse(&request.nostr_secret_key_hex).map_err(|err| err.to_string())?;
        let provider_pubkey = keys.public_key();
        let client = Client::new(keys);
        let relay_url = RelayUrl::parse(&request.relay_url.0).map_err(|err| err.to_string())?;
        client
            .add_relay(relay_url)
            .await
            .map_err(|err| err.to_string())?;
        let output = client.try_connect(RELAY_CONNECT_TIMEOUT).await;
        if output.success.is_empty() {
            return Err("no relay connection succeeded".to_owned());
        }

        // Supersede first. This is the half that carries a guarantee: a relay
        // must keep only the newest event per addressable coordinate, so the
        // live advertisement stops being served whatever the relay thinks of
        // deletion requests.
        let superseding = EventBuilder::new(
            Kind::Custom(FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND),
            request.expired_content,
        )
        .custom_created_at(nostr_sdk::Timestamp::from_secs(
            request.expired_created_at.0,
        ))
        .tags([
            tag(["d", FLIP_PROVIDER_ADVERTISEMENT_D_TAG]),
            tag(["t", FLIP_PROVIDER_ADVERTISEMENT_HASHTAG]),
        ]);
        let output = client
            .send_event_builder(superseding)
            .await
            .map_err(|err| err.to_string())?;
        if !output.failed.is_empty() {
            return Err(format!("{:?}", output.failed));
        }
        if output.success.is_empty() {
            return Err("no relay accepted the superseding advertisement".to_owned());
        }
        let superseding_id = *output.id();

        // Then ask for outright removal. Naming the coordinate covers every
        // generation at once; naming the event just published covers relays
        // that implement deletion for event ids only. Either way the event this
        // would delete is already the expired one, so a relay that declines the
        // request is left serving a document clients reject rather than a live
        // advertisement — which is why a failure here is not an error.
        let mut deletion = EventDeletionRequest::new()
            .coordinate(Coordinate {
                kind: Kind::Custom(FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND),
                public_key: provider_pubkey,
                identifier: FLIP_PROVIDER_ADVERTISEMENT_D_TAG.to_owned(),
            })
            .id(superseding_id);
        if let Some(reason) = request.reason {
            deletion = deletion.reason(reason);
        }
        if let Err(error) = client
            .send_event_builder(EventBuilder::delete(deletion))
            .await
        {
            tracing::debug!(%error, "relay did not accept the advertisement deletion request");
        }

        Ok(())
    }
}

/// Fake publisher used by fast daemon tests.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(crate) struct FakeRelayPublisher;

#[cfg(test)]
#[async_trait]
impl RelayPublisher for FakeRelayPublisher {
    async fn publish(&self, request: RelayPublishRequest) -> Result<RelayPublishResult, String> {
        let digest = fedi_decentralized_service_liquidity_manager::domain_tagged_sha256(
            b"fedi-flip-fake-relay-event-id/v1\0",
            format!("{}:{}", request.relay_url.0, request.content).as_bytes(),
        );
        Ok(RelayPublishResult {
            event_id: hex::encode(digest.0),
        })
    }

    async fn withdraw(&self, request: RelayWithdrawRequest) -> Result<(), String> {
        let _ = request.reason;
        Ok(())
    }
}

/// Fake publisher that publishes normally but cannot withdraw, so a test can
/// reach the state where an advertisement is still on a relay after FLIP has
/// decided to take it down.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(crate) struct FailingWithdrawRelayPublisher;

#[cfg(test)]
#[async_trait]
impl RelayPublisher for FailingWithdrawRelayPublisher {
    async fn publish(&self, request: RelayPublishRequest) -> Result<RelayPublishResult, String> {
        FakeRelayPublisher.publish(request).await
    }

    async fn withdraw(&self, _request: RelayWithdrawRequest) -> Result<(), String> {
        Err("relay refused the deletion request".to_owned())
    }
}

/// Fake publisher that turns the daemon not-ready as its first relay publish
/// lands, so a test can reach the window between two relay publications.
///
/// The published event is what asserts readiness, and each relay is a separate
/// assertion after a separate round trip, so the readiness that held for the
/// first says nothing about the second.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct ReadinessFlippingRelayPublisher {
    /// Relay URLs this publisher was actually asked to publish to, in order.
    pub published: std::sync::Arc<std::sync::Mutex<Vec<String>>>,

    /// Installed by the test after the context exists, because the publisher
    /// has to be built before the context that owns this state.
    pub state: std::sync::Arc<
        tokio::sync::OnceCell<std::sync::Arc<tokio::sync::RwLock<crate::daemon::DaemonState>>>,
    >,
}

#[cfg(test)]
#[async_trait]
impl RelayPublisher for ReadinessFlippingRelayPublisher {
    async fn publish(&self, request: RelayPublishRequest) -> Result<RelayPublishResult, String> {
        let first = {
            let mut published = self.published.lock().expect("relay log is not poisoned");
            published.push(request.relay_url.0.clone());
            published.len() == 1
        };
        if first && let Some(state) = self.state.get() {
            // Recovery rather than phase: `DaemonPhase::Ready` is also what the
            // shutdown path moves away from, and using recovery keeps this test
            // about readiness rather than about shutdown.
            state.write().await.recovery_complete = false;
        }
        FakeRelayPublisher.publish(request).await
    }

    async fn withdraw(&self, request: RelayWithdrawRequest) -> Result<(), String> {
        FakeRelayPublisher.withdraw(request).await
    }
}

pub(crate) fn nostr_relay_publisher() -> std::sync::Arc<dyn RelayPublisher> {
    std::sync::Arc::new(NostrRelayPublisher)
}

#[cfg(test)]
pub(crate) fn fake_relay_publisher() -> std::sync::Arc<dyn RelayPublisher> {
    std::sync::Arc::new(FakeRelayPublisher)
}

#[cfg(test)]
pub(crate) fn failing_withdraw_relay_publisher() -> std::sync::Arc<dyn RelayPublisher> {
    std::sync::Arc::new(FailingWithdrawRelayPublisher)
}

fn tag<const N: usize>(values: [&str; N]) -> Tag {
    Tag::parse(values).expect("Nostr tag construction uses valid tags")
}
