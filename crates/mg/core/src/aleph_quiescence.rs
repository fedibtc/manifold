//! Aleph quiescence in the seats' fedimintd.
//!
//! With quiescence on, a seat's fedimintd creates AlephBFT units only while
//! it has items to order or a batch of its own awaits ordering, and its
//! modules stop re-proposing votes their peers already recorded. An idle
//! federation then stops writing a unit per round to every guardian's disk,
//! and a session lasts as long as it takes its rounds to fill with work. The
//! bundled fedimintd (fedibtc/fedimint fork) holds the switch in memory: every
//! process starts with it off, and the local `set_aleph_quiescence` admin
//! call turns it on or off without a restart.
//!
//! The admitted feature policy's `aleph_quiescence` flag decides; no policy
//! means off ([SPEC-setup-payment-federations](../../../../specs/SPEC-setup-payment-federations.md)).
//! The fleet republishes the flag on a watch that every seat loop follows.
//! Each loop applies it to its own child when the flag changes while the
//! child answers, and after every healthy watchdog probe while the child
//! still differs, which covers a respawned child.

use fedi_decentralized_domain::AdmittedFeaturePolicy;
use tokio::sync::watch;

use crate::fleet::Fleet;

impl Fleet {
    /// Follow the `aleph_quiescence` flag of the admitted feature policy.
    /// Call once, when the Nostr boundary exists.
    pub fn bind_aleph_quiescence_policy(
        &self,
        mut policy: watch::Receiver<Option<AdmittedFeaturePolicy>>,
    ) {
        let switch = self.aleph_quiescence.clone();
        tokio::spawn(async move {
            loop {
                let enabled = policy
                    .borrow_and_update()
                    .as_ref()
                    .is_some_and(AdmittedFeaturePolicy::aleph_quiescence);
                switch.send_if_modified(|current| std::mem::replace(current, enabled) != enabled);
                if policy.changed().await.is_err() {
                    return;
                }
            }
        });
    }
}
