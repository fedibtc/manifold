//! The deployment label that the collector store is bound to.

use fedi_decentralized_manifold_environment::ManifoldEnvironment;

/// The label that the key sentinel and every stored target secret
/// authenticate, so a store from another environment refuses to open.
///
/// Stored state does not depend on the trust settings: every registration,
/// heartbeats included, verifies its PeerBadge with the running build's
/// trust settings, and an admission lapses when its lease ends. The
/// `profile-9` suffix is frozen persisted text left from a profile version
/// that Manifold no longer has; changing it would mean re-encrypting every
/// stored secret for no gain.
pub(crate) fn store_binding_label(environment: ManifoldEnvironment) -> String {
    format!("{environment}:profile-9")
}

#[cfg(test)]
mod tests {
    use fedi_decentralized_manifold_environment::ManifoldEnvironment;

    use super::store_binding_label;

    #[test]
    fn deployed_stores_keep_their_label() {
        assert_eq!(
            store_binding_label(ManifoldEnvironment::Production),
            "production:profile-9"
        );
        assert_eq!(
            store_binding_label(ManifoldEnvironment::Staging),
            "staging:profile-9"
        );
    }
}
