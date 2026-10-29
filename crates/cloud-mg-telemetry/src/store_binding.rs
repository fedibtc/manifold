//! The trust binding of the collector's persisted store.

use fedi_decentralized_manifold_environment::ManifoldEnvironment;

/// Revision of the PeerBadge trust inputs that the collector store is bound to.
///
/// The key sentinel and every stored target secret authenticate
/// [`store_trust_profile`], so a store admitted under other trust inputs
/// refuses to open. The collector reads only the PeerBadge inputs that
/// `trust_inputs_digest` covers in its test. The shared
/// `MANIFOLD_ENVIRONMENT_PROFILE_REVISION` also moves for mappings the
/// collector never reads, and binding the store to it made every such bump
/// strand the deployed store. Bump this only when the pinned digest test
/// fails; a bump requires a store reset.
const STORE_TRUST_REVISION: u32 = 9;

/// The trust profile that the collector store is bound to.
pub(crate) fn store_trust_profile(environment: ManifoldEnvironment) -> String {
    format!("{environment}:profile-{STORE_TRUST_REVISION}")
}

#[cfg(test)]
mod tests {
    use fedi_decentralized_manifold_environment::ManifoldEnvironment;
    use sha2::{Digest, Sha256};

    use super::store_trust_profile;

    /// Every profile input that `PeerBadgeVerifier::try_from_profile` reads,
    /// apart from the environment and the shared revision.
    fn trust_inputs_digest(environment: ManifoldEnvironment) -> String {
        let profile = environment.profile().unwrap();
        let mut hasher = Sha256::new();
        for identity in profile.peer_badge_issuer_identities() {
            hasher.update(format!("issuer {}\n", identity.to_hex()));
        }
        for relay in profile.nostr_relays().as_urls() {
            hasher.update(format!("relay {relay}\n"));
        }
        hasher.update(format!(
            "minimum trust level {}\n",
            profile.minimum_peer_badge_trust_level()
        ));
        for authority in profile.pinned_issuer_authorities() {
            hasher.update(format!("authority {authority}\n"));
        }
        hex::encode(hasher.finalize())
    }

    #[test]
    fn deployed_stores_keep_their_binding() {
        assert_eq!(
            store_trust_profile(ManifoldEnvironment::Production),
            "production:profile-9"
        );
        assert_eq!(
            store_trust_profile(ManifoldEnvironment::Staging),
            "staging:profile-9"
        );
    }

    #[test]
    fn store_binding_moves_with_the_collector_trust_inputs() {
        // A failure means the PeerBadge trust inputs changed. Bump
        // `STORE_TRUST_REVISION`, plan the store reset, then update these.
        assert_eq!(
            trust_inputs_digest(ManifoldEnvironment::Production),
            "54b7c376fda3831132741aba39b6feefc78ef6f245dfdca2e6fdb0e8949f3379"
        );
        assert_eq!(
            trust_inputs_digest(ManifoldEnvironment::Staging),
            "26adc04bc6c6556dca32c80a5d7bf9ddf00ffd3e9c9e85c150dca0813df5acde"
        );
    }
}
