//! Complete, bounded relay reads used by the shared PeerBadge verifier.

#[cfg(test)]
mod tests;

use std::future::Future;

use fedi_decentralized_nostr::attester::{
    CREDENTIAL_REVOCATION_EVENT_KIND, ISSUER_AUTHORITY_D_TAG, ISSUER_AUTHORITY_EVENT_KIND,
    ISSUER_AUTHORITY_HASHTAG, credential_revocation_d_tag,
};
use fedimint_core::runtime::{Instant, timeout};
use nostr_sdk::{Event, Filter, Kind, PublicKey, RelayUrl};

use crate::{NostrClientError, NostrClientResult, NostrRelayClient};

const PEER_BADGE_EVENT_CANDIDATE_LIMIT: u16 = 16;

/// Nostr relay reader for issuer authorities and credential revocations.
///
/// This type performs bounded role-specific relay queries only. Returned events
/// remain untrusted candidates; `PeerBadgeVerifier` authenticates their event
/// envelopes and signed content.
#[derive(Clone, Debug)]
pub struct NostrPeerBadgeClient {
    authority_relays: Vec<RelayUrl>,
}

impl NostrPeerBadgeClient {
    /// Retain a non-empty canonical authority relay set.
    #[must_use]
    pub fn new(
        first_authority_relay: RelayUrl,
        additional_authority_relays: impl IntoIterator<Item = RelayUrl>,
    ) -> Self {
        let mut authority_relays = vec![first_authority_relay];
        authority_relays.extend(additional_authority_relays);
        Self { authority_relays }
    }

    /// Fetch complete bounded issuer-authority results from every canonical
    /// authority relay.
    ///
    /// Every relay must reach EOSE before the absolute deadline. Results are
    /// combined so the verifier can select the newest authenticated candidate
    /// across the full canonical set. Cryptographic and semantic admission
    /// remains the verifier's responsibility.
    pub async fn fetch_issuer_authority_candidates(
        &self,
        issuer: PublicKey,
        deadline: Instant,
    ) -> NostrClientResult<Vec<Event>> {
        let filter = Filter::new()
            .kind(Kind::Custom(ISSUER_AUTHORITY_EVENT_KIND))
            .author(issuer)
            .identifier(ISSUER_AUTHORITY_D_TAG)
            .hashtag(ISSUER_AUTHORITY_HASHTAG)
            .limit(usize::from(PEER_BADGE_EVENT_CANDIDATE_LIMIT) + 1);

        fetch_all_relay_candidates(&self.authority_relays, |relay_url| {
            fetch_candidates(relay_url, filter.clone(), deadline)
        })
        .await
    }

    /// Fetch complete bounded credential-revocation results from every Nostr
    /// relay listed by the authenticated issuer authority.
    ///
    /// Every relay must reach EOSE before the absolute deadline. Results are
    /// combined so an empty response from one location cannot hide a
    /// revocation held by another. Content admission remains the verifier's
    /// responsibility.
    pub async fn fetch_revocation_candidates(
        &self,
        issuer: PublicKey,
        credential_digest: &str,
        relay_urls: &[RelayUrl],
        deadline: Instant,
    ) -> NostrClientResult<Vec<Event>> {
        let filter = Filter::new()
            .kind(Kind::Custom(CREDENTIAL_REVOCATION_EVENT_KIND))
            .author(issuer)
            .identifier(credential_revocation_d_tag(credential_digest))
            .limit(usize::from(PEER_BADGE_EVENT_CANDIDATE_LIMIT) + 1);

        fetch_all_relay_candidates(relay_urls, |relay_url| {
            fetch_candidates(relay_url, filter.clone(), deadline)
        })
        .await
    }
}

async fn fetch_all_relay_candidates<F, Fut>(
    relay_urls: &[RelayUrl],
    mut fetch: F,
) -> NostrClientResult<Vec<Event>>
where
    F: FnMut(RelayUrl) -> Fut,
    Fut: Future<Output = NostrClientResult<Vec<Event>>>,
{
    let mut candidates = Vec::new();
    for relay_url in relay_urls.iter().cloned() {
        candidates.extend(fetch(relay_url).await?);
    }
    Ok(candidates)
}

async fn fetch_candidates(
    relay_url: RelayUrl,
    filter: Filter,
    deadline: Instant,
) -> NostrClientResult<Vec<Event>> {
    let connect_timeout = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(NostrClientError::IncompleteQuery {
            reason: "PeerBadge lookup deadline elapsed before relay connection",
        })?;
    let relay = NostrRelayClient::connect_without_signer(&relay_url, connect_timeout).await?;
    let result = relay
        .fetch_events_complete_capped(filter, deadline, PEER_BADGE_EVENT_CANDIDATE_LIMIT)
        .await;
    let _ = timeout(
        deadline.saturating_duration_since(Instant::now()),
        relay.disconnect(),
    )
    .await;
    result
}
