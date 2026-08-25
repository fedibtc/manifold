//! The FMan's presence in the world: what it publishes about itself, what it
//! learns about who may onboard it, and what it is told about setup payments.
//!
//! Nostr is how all of that travels today, but none of it is stated in Nostr
//! terms here — this module is the daemon's half of the contract, and
//! `fman-nostr` is one runtime that fulfils it
//! ([SPEC-advertisement](../../specs/SPEC-advertisement.md),
//! [SPEC-fman-nostr-events](../../../nostr/specs/SPEC-fman-nostr-events.md)).
//!
//! Nothing here is a trait. A trait would be a hole — something the daemon
//! needs *done* by a crate it cannot name — and none of this is that. What
//! the runtime reads out of the daemon it calls directly
//! ([`crate::fleet::FleetNostrHost`],
//! [`crate::fleet::FleetSetupPaymentPolicyStore`]), because it depends on this
//! crate. What the daemon reads back is not behavior but the runtime's latest
//! observation, so it travels as a value on a [`tokio::sync::watch`] channel:
//! the admin socket borrows the last one published and cannot block on a relay
//! even in principle.

use fedi_decentralized_domain::FmanVersion;
use fedi_decentralized_service_fleet_manager::Plan;
use nostr_sdk::PublicKey;

/// Dynamic daemon state needed to build an advertisement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdvertisementSnapshot {
    pub iroh_endpoint_id: String,
    /// The FMan's commitment-signing service pubkey — the locator
    /// `service_pubkey` FIs verify signed responses against, derived from
    /// the daemon's root mnemonic (`fman/v1/service-sign`). Distinct from
    /// the Nostr signing identity. Typed so an unparseable key cannot reach
    /// the wire; the payload carries its canonical lowercase-hex rendering.
    pub service_pubkey: secp256k1::XOnlyPublicKey,
    pub plans: Vec<Plan>,
}

/// Current Holder-authorization discovery state.
///
/// Four states, not two. "No authorization was found" and "we have not looked
/// yet" are different facts, and a dashboard that cannot tell them apart has to
/// hedge every sentence it writes about either one. A relay that refused the
/// read is a third fact, and it is the operator's to act on, not ours to hide.
///
/// [`Self::AuthorizationObserved`] outranks the other three. Retained
/// authorizations are durable and are re-verified before reuse, so an empty or
/// failed read never demotes a fleet that has one: the read failed, the
/// authorization did not go away.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OnboardingStatus {
    /// No read has completed since this daemon started, and nothing is
    /// retained. The honest answer to "is this fleet authorized" is "not known
    /// yet".
    Checking,
    /// A read completed and found no authorization naming this FMan. This is
    /// the state a fleet sits in while it waits for a Holder to sign.
    NotObserved {
        /// When the read that found nothing completed, in seconds since the
        /// epoch.
        checked_at: u64,
    },
    AuthorizationObserved {
        authorizations: usize,
        holders: Vec<PublicKey>,
        /// When the most recent successful read completed, or `None` when the
        /// authorizations were loaded from the retained store and no read has
        /// succeeded since. A dashboard showing "last checked" must be able to
        /// say "not since this daemon started".
        checked_at: Option<u64>,
    },
    /// The last read failed and nothing is retained, so nothing can be said
    /// about this fleet's authorization until a read succeeds.
    RelayError {
        /// The failure, for the operator to read. Not for the dashboard to
        /// match on.
        error: String,
    },
}

/// What the operator socket reports about this FMan's directory presence: the
/// identity an operator hands a Holder, and whether that Holder has authorized
/// this FMan yet ([SPEC-admin-socket](../../specs/SPEC-admin-socket.md)).
///
/// Published by the directory runtime on a watch channel; the socket reads the
/// latest value, which is what the runtime last observed and never a relay
/// round trip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryPresence {
    /// The pubkey a Holder authorizes to onboard this FMan. Constant for an
    /// install; it rides along so the socket has one value to read.
    pub service_nostr_pubkey: PublicKey,
    pub onboarding: OnboardingStatus,
    /// Latest FMan release in the last authenticated setup-payment
    /// publication, or `None` before one has been admitted.
    pub latest_fman_version: Option<FmanVersion>,
}
