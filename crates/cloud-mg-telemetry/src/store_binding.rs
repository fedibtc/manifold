//! The deployment label that the collector store is bound to.

use fedi_decentralized_manifold_environment::ManifoldEnvironment;

use crate::config::Admission;

/// The label that the key sentinel and every stored target secret
/// authenticate, so a store from another environment refuses to open.
///
/// Stored state does not depend on the trust settings: every registration,
/// heartbeats included, verifies its PeerBadge with the running build's
/// trust settings, and an admission lapses when its lease ends. The
/// `profile-9` suffix is frozen persisted text left from a profile version
/// that Manifold no longer has; changing it would mean re-encrypting every
/// stored secret for no gain.
///
/// An unbadged store has its own label, so neither mode opens the other's
/// store: a badged collector would export invites from unbadged targets,
/// and an unbadged one would admit badged MGs without their badge.
pub(crate) fn store_binding_label(
    environment: ManifoldEnvironment,
    admission: Admission,
) -> String {
    match admission {
        Admission::Badged => format!("{environment}:profile-9"),
        Admission::Unbadged(_) => format!("{environment}:unbadged"),
    }
}

#[cfg(test)]
mod tests {
    use fedi_decentralized_manifold_environment::ManifoldEnvironment;

    use super::store_binding_label;
    use crate::config::{Admission, UnbadgedLimits};

    #[test]
    fn deployed_stores_keep_their_label() {
        assert_eq!(
            store_binding_label(ManifoldEnvironment::Production, Admission::Badged),
            "production:profile-9"
        );
        assert_eq!(
            store_binding_label(ManifoldEnvironment::Staging, Admission::Badged),
            "staging:profile-9"
        );
    }

    #[test]
    fn unbadged_stores_have_their_own_label() {
        let unbadged = Admission::Unbadged(UnbadgedLimits {
            max_targets: std::num::NonZeroU16::new(1).unwrap(),
            new_targets_per_hour: std::num::NonZeroU16::new(1).unwrap(),
            source_new_targets_per_hour: std::num::NonZeroU8::new(1).unwrap(),
            log_target_quota_bytes: std::num::NonZeroU64::new(1).unwrap(),
        });
        assert_eq!(
            store_binding_label(ManifoldEnvironment::Production, unbadged),
            "production:unbadged"
        );
    }
}
