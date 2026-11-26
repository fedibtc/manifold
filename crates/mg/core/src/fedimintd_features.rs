//! Runtime feature switches of the seats' fedimintd.
//!
//! The bundled fedimintd (fedibtc/fedimint fork) reads its
//! [`Features`] over the driven control channel on its stdin once its
//! consensus starts. Every process starts with all of them off, and each
//! frame MG sends is the complete set, so a switch changes on a running
//! child without a restart.
//!
//! The admitted feature policy decides the switches; no policy means all off
//! ([SPEC-setup-payment-federations](../../../../specs/SPEC-setup-payment-federations.md)).
//! The fleet republishes them on a watch that every seat loop follows. A
//! seat sends the current set when its child's consensus starts, which covers
//! a respawned child, and again whenever it changes.
//!
//! - `aleph_idle_round_delay_ms`: while the child has nothing to order, it
//!   creates its AlephBFT unit only once this delay has passed since its last
//!   one, and its modules stop re-proposing votes their peers already
//!   recorded. It returns to the normal pace while it, or a peer, has items
//!   to order, and when it or a peer is about to shut down. An idle
//!   federation then exchanges a small fraction of the data it exchanges at
//!   the normal pace. `None` keeps the normal pace.

use fedi_decentralized_domain::AdmittedFeaturePolicy;
pub use fedimint_server::config::driven::Features;
use tokio::sync::watch;

use crate::fleet::Fleet;

/// The switches `policy` sets.
fn features(policy: Option<&AdmittedFeaturePolicy>) -> Features {
    Features {
        aleph_idle_round_delay_ms: policy
            .and_then(AdmittedFeaturePolicy::aleph_idle_round_delay_ms),
    }
}

impl Fleet {
    /// Follow the admitted feature policy. Call once, when the Nostr
    /// boundary exists.
    pub fn bind_fedimintd_features_policy(
        &self,
        mut policy: watch::Receiver<Option<AdmittedFeaturePolicy>>,
    ) {
        let switches = self.fedimintd_features.clone();
        tokio::spawn(async move {
            loop {
                let next = features(policy.borrow_and_update().as_ref());
                switches.send_if_modified(|current| {
                    let changed = *current != next;
                    *current = next;
                    changed
                });
                if policy.changed().await.is_err() {
                    return;
                }
            }
        });
    }
}
