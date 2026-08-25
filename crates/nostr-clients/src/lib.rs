#![allow(async_fn_in_trait)]

//! Service-trait Nostr clients for decentralized federation components.

mod error;
mod fi;
mod flip;
mod holder;
mod nostr_relay_client;
mod peer_badge;
mod relay_candidate_database;

pub use error::{NostrClientError, NostrClientResult};
pub use fi::{
    FLIP_PROVIDER_ADVERTISEMENTS_CANDIDATE_LIMIT, FLIP_PROVIDER_ADVERTISEMENTS_RETAINED_MAX_BYTES,
    FMAN_ADVERTISEMENTS_CANDIDATE_LIMIT, FMAN_ADVERTISEMENTS_RETAINED_MAX_BYTES, FiNostrClient,
    NostrFiClient, SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT,
    SETUP_PAYMENT_FEDERATIONS_RETAINED_MAX_BYTES, setup_payment_federations_filter,
};
pub use flip::{
    FMAN_TRUST_MATERIAL_CANDIDATE_LIMIT_DEFAULT, FMAN_TRUST_MATERIAL_CANDIDATE_LIMIT_MAX,
    FetchFmanTrustMaterialRequest, FlipNostrClient, NostrFlipClient,
};
pub use holder::{
    HolderNostrClient, NostrHolderClient, PublishFlipAuthorizationRequest,
    PublishFmanAuthorizationRequest,
};
pub use nostr_relay_client::NostrRelayClient;
pub use peer_badge::NostrPeerBadgeClient;

/// Maximum normalized JSON size of one untrusted role-fetched event.
pub const ROLE_FETCHED_EVENT_MAX_BYTES: usize = 256 * 1024;
