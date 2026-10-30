//! Source-owned PeerBadge admission for guardian replacement.

use std::time::Duration;

use fedi_decentralized_manifold_environment::ManifoldEnvironmentProfile;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedi_decentralized_service_manifold_guardian::DestinationAdmission;
use mg_core::replacement_guardian::{ReplacementGuardianUnverified, ReplacementGuardianVerifier};

/// Apply the source's complete PeerBadge policy with fresh revocation checks.
pub struct NostrReplacementGuardianVerifier {
    /// Immutable canonical verifier configuration; never a trust-result cache.
    verifier: PeerBadgeVerifier,
}

impl NostrReplacementGuardianVerifier {
    /// Construct admission from the same canonical profile as the source MG.
    pub fn new(profile: ManifoldEnvironmentProfile) -> anyhow::Result<Self> {
        Ok(Self {
            verifier: PeerBadgeVerifier::try_from_profile(&profile)?,
        })
    }

    async fn verify_admission(&self, admission: &DestinationAdmission) -> anyhow::Result<()> {
        let envelope = admission.verify_subject()?;
        // Screen forged issuance before spending the deadline on relay reads.
        self.verifier
            .verify_issuance_at(envelope, super::now_secs())?;
        self.verifier.verify(envelope).await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ReplacementGuardianVerifier for NostrReplacementGuardianVerifier {
    async fn verify(
        &self,
        admission: &DestinationAdmission,
    ) -> Result<(), ReplacementGuardianUnverified> {
        tokio::time::timeout(Duration::from_secs(30), self.verify_admission(admission))
            .await
            .map_err(|_| ReplacementGuardianUnverified)?
            .map_err(|_| ReplacementGuardianUnverified)
    }
}
