//! Federation Ambassador-facing Nostr service trait and SDK-backed client.

#[cfg(test)]
mod tests;

use std::time::Duration;

use fedimint_core::runtime::Instant;
use nostr_sdk::{Event, Filter, Kind, PublicKey};

use crate::{NostrClientResult, NostrRelayClient, ROLE_FETCHED_EVENT_MAX_BYTES};

/// Maximum number of untrusted common-set candidates retained from one relay query.
pub const SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT: u16 = 16;

/// Maximum aggregate bytes retained from one common-set relay query.
pub const SETUP_PAYMENT_FEDERATIONS_RETAINED_MAX_BYTES: usize =
    ROLE_FETCHED_EVENT_MAX_BYTES * SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT as usize;

/// Maximum number of untrusted MG advertisement candidates retained from one
/// relay query.
///
/// The canonical relays are deployment-pinned trusted infrastructure, but they
/// cannot gate writes, so the untrusted input is publisher volume: pubkeys are
/// free and advertisements are per-author replaceable, so candidate
/// cardinality tracks distinct publisher pubkeys. The fleet may reach hundreds
/// of MGs over time; this ceiling is ~10-20x that runway — a resource
/// backstop, never an expected listing size. Reaching it completes the
/// enumeration successfully with the retained prefix: with open-write relays a
/// spammer can always exceed any ceiling, so treating a full cap as an error
/// would hand every publisher a discovery-bricking button.
pub const MG_ADVERTISEMENTS_CANDIDATE_LIMIT: u16 = 2048;

/// Maximum aggregate bytes retained from one MG advertisement relay query.
///
/// Deliberately not derived as candidate limit x per-event cap (that product
/// would be 128 MiB): this bound is out-of-memory insurance, and hitting it
/// completes the enumeration successfully with the retained prefix rather
/// than failing it — it degrades, never errors. Honest advertisements
/// serialize to roughly 1-2 KiB, so even a full 2048-candidate listing stays
/// around 4 MiB; only near-cap-sized spam events can approach this bound.
//
// TODO: tighten the per-event cap for advertisements in a follow-up: the
// measured worst-case legitimate advertisement event is 7830 bytes (~7.6 KiB;
// `worst_case_advertisement_event_fits_the_per_event_cap` in
// `crates/nostr/src/mg/tests.rs`), far below the shared
// `ROLE_FETCHED_EVENT_MAX_BYTES` (64 KiB) used per event for now; derive an
// ad-specific per-event cap from that measurement with margin.
pub const MG_ADVERTISEMENTS_RETAINED_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Maximum number of untrusted FLIP provider advertisement candidates retained
/// from one complete registry query.
pub const FLIP_PROVIDER_ADVERTISEMENTS_CANDIDATE_LIMIT: u16 = 256;

/// Aggregate retained bytes for one FLIP provider enumeration.
pub const FLIP_PROVIDER_ADVERTISEMENTS_RETAINED_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Nostr interactions performed by the Federation Ambassador component.
pub trait FaNostrClient {
    /// Fetch one MG advertisement event authored by the given MG pubkey.
    ///
    /// Returned events are untrusted relay data. Callers must verify event signatures,
    /// advertisement signatures, content/tag consistency, credential proofs, and
    /// revocation state before selecting an MG.
    ///
    /// # Errors
    ///
    /// Returns an error if relay querying fails or no advertisement is found.
    async fn fetch_mg_advertisement(
        &self,
        mg_pubkey: PublicKey,
        timeout: Duration,
    ) -> NostrClientResult<Event>;

    /// Fetch hard-bounded common setup-payment federation publication candidates.
    ///
    /// Returned events are untrusted relay data. The caller must authenticate
    /// every complete event against its deployment-pinned publisher and retain
    /// its own durable replacement-order high-water mark.
    ///
    /// Implementations must enforce these transport bounds before returning:
    ///
    /// - reject a complete normalized event larger than
    ///   [`ROLE_FETCHED_EVENT_MAX_BYTES`] before signature verification or
    ///   storage;
    /// - observe at most [`SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT`] matching
    ///   candidates; and
    /// - retain at most [`SETUP_PAYMENT_FEDERATIONS_RETAINED_MAX_BYTES`] across
    ///   the complete returned batch.
    ///
    /// Relay-side filters and limits are hints. Implementations must enforce the
    /// publisher, kind, identifier, count, and byte bounds locally without
    /// applying NIP-01 replacement suppression before semantic admission.
    ///
    /// # Errors
    ///
    /// Returns an error if relay querying fails.
    async fn fetch_setup_payment_federations(
        &self,
        publisher: PublicKey,
        timeout: Duration,
    ) -> NostrClientResult<Vec<Event>>;

    /// Fetch hard-bounded MG advertisement candidates from every author.
    ///
    /// This is the registry enumeration query: unlike
    /// [`Self::fetch_mg_advertisement`] it pins no author, so one relay
    /// answer can contain events from many claimed MG identities, including
    /// multiple or conflicting events per author.
    ///
    /// Returned events are untrusted relay data. The caller must verify event
    /// signatures, advertisement document proofs, content/tag consistency,
    /// credential proofs, revocation state, and the authenticated
    /// subject-to-author binding before selecting an MG, and owns semantic
    /// admission policy including per-author replacement ordering.
    ///
    /// Implementations must enforce these transport bounds before returning:
    ///
    /// - reject a complete normalized event larger than
    ///   [`ROLE_FETCHED_EVENT_MAX_BYTES`] before signature verification or
    ///   storage;
    /// - observe at most [`MG_ADVERTISEMENTS_CANDIDATE_LIMIT`] matching
    ///   candidates; and
    /// - retain at most [`MG_ADVERTISEMENTS_RETAINED_MAX_BYTES`] across the
    ///   complete returned batch.
    ///
    /// Relay-side filters and limits are hints. Implementations must enforce
    /// the kind, identifier, hashtag, count, and byte bounds locally without
    /// applying NIP-01 replacement suppression before semantic admission.
    ///
    /// Enumeration completeness is fail-closed: implementations must accept
    /// an answer only when at least one relay delivered it EOSE-complete,
    /// the local candidate cap was reached, or the aggregate byte bound was
    /// reached — the caps are resource backstops against publisher volume,
    /// so reaching one is a successful, deliberately truncated enumeration.
    /// On a pooled client slower relays merge best-effort and identical
    /// events are counted once. An answer that stalls, ends, or is closed
    /// before any relay's EOSE with neither bound reached must be a typed
    /// error, never a silently truncated `Ok`.
    ///
    /// # Errors
    ///
    /// Returns an error if relay querying fails or the relay answer is
    /// incomplete.
    async fn fetch_mg_advertisements(&self, timeout: Duration) -> NostrClientResult<Vec<Event>>;

    /// Fetch a complete-or-locally-bounded set of current FLIP provider ads.
    ///
    /// Returned events are untrusted. The FA must verify the event role and
    /// signature, signed advertisement, provider identity, live PeerBadge,
    /// policy, freshness, and endpoint binding before disclosing an invite.
    async fn fetch_liquidity_provider_advertisements(
        &self,
        _timeout: Duration,
    ) -> NostrClientResult<Vec<Event>> {
        Err(crate::NostrClientError::MissingEvent {
            context: "FLIP provider registry capability unavailable",
        })
    }
}

/// Nostr SDK implementation of [`FaNostrClient`].
#[derive(Clone, Debug)]
pub struct NostrFaClient {
    /// Shared relay client used to discover MG advertisements.
    relay: NostrRelayClient,
}

impl NostrFaClient {
    /// Create an FA client from a shared relay client.
    pub fn new(relay: NostrRelayClient) -> Self {
        Self { relay }
    }
}

impl FaNostrClient for NostrFaClient {
    async fn fetch_mg_advertisement(
        &self,
        mg_pubkey: PublicKey,
        timeout: Duration,
    ) -> NostrClientResult<Event> {
        let filter = Filter::new()
            .kind(Kind::Custom(
                fedi_decentralized_nostr::mg::MG_ADVERTISEMENT_EVENT_KIND,
            ))
            .author(mg_pubkey)
            .identifier(fedi_decentralized_nostr::mg::MG_ADVERTISEMENT_D_TAG)
            .hashtag(fedi_decentralized_nostr::mg::MG_ADVERTISEMENT_HASHTAG)
            .limit(1);
        self.relay
            .fetch_one_event(filter, timeout, "MG advertisement")
            .await
    }

    async fn fetch_setup_payment_federations(
        &self,
        publisher: PublicKey,
        timeout: Duration,
    ) -> NostrClientResult<Vec<Event>> {
        let filter = setup_payment_federations_filter(publisher);
        self.relay
            .fetch_events_capped(filter, timeout, SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT)
            .await
    }

    async fn fetch_mg_advertisements(&self, timeout: Duration) -> NostrClientResult<Vec<Event>> {
        let filter = mg_advertisements_filter();
        // The enumeration is the registry's only completeness signal, so a
        // stalled or truncated relay answer must fail closed instead of
        // looking like a small registry; reaching a local resource bound —
        // the candidate cap or the aggregate byte bound — is still a
        // successful (deliberately truncated) enumeration.
        self.relay
            .fetch_events_complete_or_capped(
                filter,
                Instant::now() + timeout,
                MG_ADVERTISEMENTS_CANDIDATE_LIMIT,
                MG_ADVERTISEMENTS_RETAINED_MAX_BYTES,
            )
            .await
    }

    async fn fetch_liquidity_provider_advertisements(
        &self,
        timeout: Duration,
    ) -> NostrClientResult<Vec<Event>> {
        self.relay
            .fetch_events_complete_or_capped(
                liquidity_provider_advertisements_filter(),
                Instant::now() + timeout,
                FLIP_PROVIDER_ADVERTISEMENTS_CANDIDATE_LIMIT,
                FLIP_PROVIDER_ADVERTISEMENTS_RETAINED_MAX_BYTES,
            )
            .await
    }
}

fn mg_advertisements_filter() -> Filter {
    Filter::new()
        .kind(Kind::Custom(
            fedi_decentralized_nostr::mg::MG_ADVERTISEMENT_EVENT_KIND,
        ))
        .identifier(fedi_decentralized_nostr::mg::MG_ADVERTISEMENT_D_TAG)
        .hashtag(fedi_decentralized_nostr::mg::MG_ADVERTISEMENT_HASHTAG)
        .limit(MG_ADVERTISEMENTS_CANDIDATE_LIMIT.into())
}

/// Build the canonical setup-payment address query used by consumers and the
/// production publisher's replacement-order verification.
#[must_use]
pub fn setup_payment_federations_filter(publisher: PublicKey) -> Filter {
    Filter::new()
        .kind(Kind::Custom(
            fedi_decentralized_nostr::setup_payment_federations::
                SETUP_PAYMENT_FEDERATIONS_EVENT_KIND,
        ))
        .author(publisher)
        .identifier(
            fedi_decentralized_nostr::setup_payment_federations::SETUP_PAYMENT_FEDERATIONS_D_TAG,
        )
        .limit(SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT.into())
}

fn liquidity_provider_advertisements_filter() -> Filter {
    use fedi_decentralized_nostr::flip::{
        FLIP_PROVIDER_ADVERTISEMENT_D_TAG, FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND,
        FLIP_PROVIDER_ADVERTISEMENT_HASHTAG,
    };

    Filter::new()
        .kind(Kind::Custom(FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND))
        .identifier(FLIP_PROVIDER_ADVERTISEMENT_D_TAG)
        .hashtag(FLIP_PROVIDER_ADVERTISEMENT_HASHTAG)
        .limit(FLIP_PROVIDER_ADVERTISEMENTS_CANDIDATE_LIMIT.into())
}
