//! Legacy FLIP-facing Nostr service trait and SDK-backed client.
//!
//! FLIP federation-eligibility verification no longer discovers FMan eligibility
//! material from Nostr. Verifiers now fetch the federation config and consensus
//! metadata from the invite code, parse `fedi:fman_api_urls`, and query the
//! public FMan API for signed trust material. This module remains only for
//! compatibility with older exploratory call sites that inspect FMan
//! advertisements as untrusted candidates.

#[cfg(test)]
mod tests;

use std::{num::NonZeroU16, time::Duration};

use nostr_sdk::{Event, Filter, Kind, PublicKey};

use crate::{NostrClientResult, NostrRelayClient};

/// Default candidate count for legacy FMan advertisement relay queries.
pub const FMAN_TRUST_MATERIAL_CANDIDATE_LIMIT_DEFAULT: u16 = 16;

/// Maximum candidate count for legacy FMan advertisement relay queries.
pub const FMAN_TRUST_MATERIAL_CANDIDATE_LIMIT_MAX: u16 = 100;

/// Request for fetching legacy FMan advertisement candidates.
///
/// This is not used for FLIP federation-eligibility verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchFmanTrustMaterialRequest {
    /// FMan Nostr pubkey whose legacy advertisement should be fetched.
    pub fman_pubkey: PublicKey,

    /// Requested maximum number of untrusted candidate events to return.
    ///
    /// `None` uses [`FMAN_TRUST_MATERIAL_CANDIDATE_LIMIT_DEFAULT`]. Larger
    /// values are clamped to [`FMAN_TRUST_MATERIAL_CANDIDATE_LIMIT_MAX`] so
    /// callers cannot accidentally issue unbounded legacy advertisement scans.
    pub candidate_limit: Option<NonZeroU16>,

    /// Maximum time spent querying relays for candidates.
    pub timeout: Duration,
}

/// Legacy Nostr interactions once performed by the FLIP component.
pub trait FlipNostrClient {
    /// Fetch FMan advertisement candidates carrying pre-formation discovery data.
    ///
    /// Returned events are untrusted relay data and must not be used as the
    /// authoritative federation-eligibility source for FLIP. Current FLIP
    /// verification uses invite-code metadata and the public FMan API instead.
    ///
    /// # Errors
    ///
    /// Returns an error if relay querying fails.
    async fn fetch_fman_trust_material_candidates(
        &self,
        request: FetchFmanTrustMaterialRequest,
    ) -> NostrClientResult<Vec<Event>>;
}

/// Nostr SDK implementation of [`FlipNostrClient`].
#[derive(Clone, Debug)]
pub struct NostrFlipClient {
    /// Shared relay client used to fetch FMan advertisements.
    relay: NostrRelayClient,
}

impl NostrFlipClient {
    /// Create a FLIP client from a shared relay client.
    pub fn new(relay: NostrRelayClient) -> Self {
        Self { relay }
    }
}

impl FlipNostrClient for NostrFlipClient {
    async fn fetch_fman_trust_material_candidates(
        &self,
        request: FetchFmanTrustMaterialRequest,
    ) -> NostrClientResult<Vec<Event>> {
        let limit = effective_candidate_limit(request.candidate_limit);
        let timeout = request.timeout;
        let filter = fman_trust_material_filter(request);
        self.relay.fetch_events_capped(filter, timeout, limit).await
    }
}

/// Build the relay filter used to fetch legacy FMan advertisement candidates.
fn fman_trust_material_filter(request: FetchFmanTrustMaterialRequest) -> Filter {
    // `Filter::limit` is only a relay-side hint. `NostrRelayClient::fetch_events_capped`
    // enforces the same limit with a manual bounded subscription before any helper
    // can accumulate all matching event ids.
    Filter::new()
        .kind(Kind::Custom(
            fedi_decentralized_nostr::fman::FMAN_ADVERTISEMENT_EVENT_KIND,
        ))
        .author(request.fman_pubkey)
        .identifier(fedi_decentralized_nostr::fman::FMAN_ADVERTISEMENT_D_TAG)
        .hashtag(fedi_decentralized_nostr::fman::FMAN_ADVERTISEMENT_HASHTAG)
        .limit(effective_candidate_limit(request.candidate_limit).into())
}

/// Compute the bounded candidate limit for legacy FMan advertisement queries.
fn effective_candidate_limit(requested: Option<NonZeroU16>) -> u16 {
    match requested {
        None => FMAN_TRUST_MATERIAL_CANDIDATE_LIMIT_DEFAULT,
        Some(n) => n.get().min(FMAN_TRUST_MATERIAL_CANDIDATE_LIMIT_MAX),
    }
}
