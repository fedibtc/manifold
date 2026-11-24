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
//! - `aleph_quiescence`: the child creates AlephBFT units only while it has
//!   items to order or a batch of its own awaits ordering, and its modules
//!   stop re-proposing votes their peers already recorded. An idle federation
//!   then stops writing a unit per round to every guardian's disk.

use fedi_decentralized_domain::AdmittedFeaturePolicy;
pub use fedimint_server::config::driven::Features;
use tokio::sync::watch;

use crate::fleet::Fleet;

/// The switches `policy` sets.
fn features(policy: Option<&AdmittedFeaturePolicy>) -> Features {
    Features {
        aleph_quiescence: policy.is_some_and(AdmittedFeaturePolicy::aleph_quiescence),
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
