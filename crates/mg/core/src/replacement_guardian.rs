//! Independent destination admission before a source retires or releases a guardian.

/// The destination has no currently verifiable, policy-acceptable PeerBadge.
#[derive(Debug, thiserror::Error)]
#[error("replacement guardian is not verified")]
pub struct ReplacementGuardianUnverified;

/// Verify the authenticated destination proof against the source MG's trust profile.
#[async_trait::async_trait]
pub trait ReplacementGuardianVerifier: Send + Sync {
    /// Require a fresh, complete PeerBadge verification bound to this service key.
    ///
    /// Missing credentials and unavailable verification inputs must fail closed.
    async fn verify(
        &self,
        admission: &fedi_decentralized_service_manifold_guardian::DestinationAdmission,
    ) -> Result<(), ReplacementGuardianUnverified>;
}

#[cfg(test)]
pub(crate) struct UnverifiedReplacementGuardian;

#[cfg(test)]
#[async_trait::async_trait]
impl ReplacementGuardianVerifier for UnverifiedReplacementGuardian {
    async fn verify(
        &self,
        _: &fedi_decentralized_service_manifold_guardian::DestinationAdmission,
    ) -> Result<(), ReplacementGuardianUnverified> {
        Err(ReplacementGuardianUnverified)
    }
}
