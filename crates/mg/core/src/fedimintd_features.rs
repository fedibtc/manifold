//! Feature switches of the seats' fedimintd.
//!
//! The bundled fedimintd (fedibtc/fedimint fork) takes its [`Features`] in
//! two ways. At process start it reads the complete set from
//! [`FM_FEATURES_ENV`], which the seat process sets at spawn
//! ([`crate::seat_process`]). Once its consensus starts it reads further sets
//! over the driven control channel on its stdin, so a runtime switch changes
//! on a running child without a restart. A switch fixed at process start,
//! such as the iroh timeouts, takes a change when the child next starts.
//!
//! The admitted feature policy decides the switches; no policy means all off
//! ([SPEC-setup-payment-federations](../../../../specs/SPEC-setup-payment-federations.md)).
//! The fleet derives them from the policy before its first seat starts and
//! republishes them on a watch that every seat loop follows. A seat starts
//! each child with the current set, sends it again when the child's consensus
//! starts, and again whenever it changes.
//!
//! - `aleph_quiescence`: the child creates AlephBFT units only while it has
//!   items to order or a batch of its own awaits ordering, and its modules
//!   stop re-proposing votes their peers already recorded. An idle federation
//!   then stops writing a unit per round to every guardian's disk.
//! - `iroh_legacy_timeouts`, a kill switch fixed at process start: the
//!   child's iroh endpoints use fedimint 0.12's QUIC timeouts, 60s idle and
//!   30s keep-alive, instead of 0.13's 5s and 1s.
//! - `walletv2_legacy_vote_delay`, a kill switch: the child votes the
//!   walletv2 block count one block further behind its backend's tip, as
//!   fedimint 0.12 does.
//!
//! Guardians with a switch on and guardians with it off agree on every
//! consensus outcome, so each MG may apply a change on its own schedule.

use fedi_decentralized_domain::AdmittedFeaturePolicy;
pub use fedimint_server::config::driven::{FM_FEATURES_ENV, Features};
use tokio::sync::watch;

/// The switches `policy` sets.
fn features(policy: Option<&AdmittedFeaturePolicy>) -> Features {
    Features {
        aleph_quiescence: policy.is_some_and(AdmittedFeaturePolicy::aleph_quiescence),
        iroh_legacy_timeouts: policy.is_some_and(AdmittedFeaturePolicy::iroh_legacy_timeouts),
        walletv2_legacy_vote_delay: policy
            .is_some_and(AdmittedFeaturePolicy::walletv2_legacy_vote_delay),
    }
}

/// The switches `policy` sets now, republished as it changes. The returned
/// watch holds the current policy's switches before this returns, so a seat
/// started right after sees them.
pub(crate) fn follow_policy(
    mut policy: watch::Receiver<Option<AdmittedFeaturePolicy>>,
) -> watch::Sender<Features> {
    let switches = watch::Sender::new(features(policy.borrow_and_update().as_ref()));
    let follower = switches.clone();
    tokio::spawn(async move {
        while policy.changed().await.is_ok() {
            let next = features(policy.borrow_and_update().as_ref());
            follower.send_if_modified(|current| {
                let changed = *current != next;
                *current = next;
                changed
            });
        }
    });
    switches
}
