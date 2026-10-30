//! Source-owned PeerBadge admission for guardian replacement.

use std::collections::BTreeSet;
use std::time::Duration;

use fedi_decentralized_manifold_environment::ManifoldEnvironmentProfile;
use fedi_decentralized_nostr::mg::{
    AdvertisementDocument, MG_ADVERTISEMENT_D_TAG, MG_ADVERTISEMENT_EVENT_KIND,
    verify_advertisement_self_signature,
};
use fedi_decentralized_nostr_clients::{FaNostrClient, NostrFaClient, NostrRelayClient};
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedi_decentralized_service_manifold_guardian::DestinationAdmission;
use mg_core::replacement_guardian::{ReplacementGuardianUnverified, ReplacementGuardianVerifier};
use nostr_sdk::{Event, Keys, Kind};

/// Resolve destination authorizations and apply this deployment's complete
/// PeerBadge policy, including fresh revocation checks.
pub struct NostrReplacementGuardianVerifier {
    /// Source-owned relay routing and issuer/trust policy.
    profile: ManifoldEnvironmentProfile,
    /// Immutable canonical verifier configuration; never a trust-result cache.
    verifier: PeerBadgeVerifier,
}

impl NostrReplacementGuardianVerifier {
    /// Construct admission from the same canonical profile as the source MG.
    pub fn new(profile: ManifoldEnvironmentProfile) -> anyhow::Result<Self> {
        let verifier = PeerBadgeVerifier::try_from_profile(&profile)?;
        Ok(Self { profile, verifier })
    }

    async fn verify_destination(
        &self,
        destination: secp256k1::XOnlyPublicKey,
    ) -> anyhow::Result<()> {
        let nostr = NostrRelayClient::connect_pool(
            self.profile.nostr_relays().as_urls(),
            Keys::generate(),
            super::REQUEST_TIMEOUT,
        )
        .await?;
        // Grants name the RPC commitment key, not the MG's Nostr/badge key.
        // Only the MG-signed advertisement binds those two identities. Reuse
        // bounded registry discovery; never infer a binding from relay tags.
        let mut events = NostrFaClient::new(nostr)
            .fetch_mg_advertisements(super::REQUEST_TIMEOUT)
            .await?;
        events.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        let mut admitted_authors = BTreeSet::new();
        let now = super::now_secs();
        for event in events {
            let Ok(document) = authenticated_advertisement(&event) else {
                continue;
            };
            // Never fall back to a superseded key binding or badge set.
            if !admitted_authors.insert(event.pubkey) {
                continue;
            }
            let payload = document.payload;
            if payload.service_pubkey != destination.to_string()
                || payload.issued_at > now
                || payload.expires_at <= now
                || now - payload.issued_at > 4 * super::REPUBLISH_INTERVAL.as_secs()
            {
                continue;
            }
            for envelope in payload.holder_authorizations {
                // Relay authors can copy a public badge and claim any service
                // key. Reject both borrowed subjects and forged issuance before
                // spending the admission deadline on revocation relay reads.
                if envelope.holder_authorization.authorization.subject_pubkey.0 != event.pubkey
                    || self.verifier.verify_issuance_at(&envelope, now).is_err()
                {
                    continue;
                }
                // Complete verification, not enrollment's offline issuance
                // check, establishes fresh revocation and trust-level policy.
                if let Ok(badge) = self.verifier.verify(&envelope).await
                    && badge.subject().0 == event.pubkey
                {
                    return Ok(());
                }
            }
        }
        anyhow::bail!("no verified destination advertisement")
    }

    async fn verify_admission(&self, admission: &DestinationAdmission) -> anyhow::Result<()> {
        let envelope = admission.verify_subject()?;
        // Screen forged issuance before spending the deadline on relay reads.
        self.verifier
            .verify_issuance_at(envelope, super::now_secs())?;
        let badge = self.verifier.verify(envelope).await?;
        anyhow::ensure!(
            badge.subject() == &envelope.holder_authorization.authorization.subject_pubkey,
            "wrong admission subject"
        );
        Ok(())
    }
}

/// Authenticate both the registry event and its portable MG identity binding.
fn authenticated_advertisement(event: &Event) -> anyhow::Result<AdvertisementDocument> {
    anyhow::ensure!(
        event.kind == Kind::Custom(MG_ADVERTISEMENT_EVENT_KIND),
        "wrong event kind"
    );
    let mut identifiers = event
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().is_some_and(|name| name == "d"));
    anyhow::ensure!(
        identifiers.next().is_some_and(|tag| {
            tag.as_slice().get(1).map(String::as_str) == Some(MG_ADVERTISEMENT_D_TAG)
        }) && identifiers.next().is_none(),
        "wrong advertisement identifier"
    );
    event.verify()?;
    let document: AdvertisementDocument = serde_json::from_str(&event.content)?;
    verify_advertisement_self_signature(&document)?;
    anyhow::ensure!(
        document.payload.mg_id_pubkey == event.pubkey.to_string(),
        "wrong advertisement author"
    );
    Ok(document)
}

#[async_trait::async_trait]
impl ReplacementGuardianVerifier for NostrReplacementGuardianVerifier {
    async fn verify(
        &self,
        destination: secp256k1::XOnlyPublicKey,
        admission: Option<&DestinationAdmission>,
    ) -> Result<(), ReplacementGuardianUnverified> {
        tokio::time::timeout(Duration::from_secs(30), async {
            match admission {
                Some(admission) => self.verify_admission(admission).await,
                None => self.verify_destination(destination).await,
            }
        })
        .await
        .map_err(|_| ReplacementGuardianUnverified)?
        .map_err(|_| ReplacementGuardianUnverified)
    }
}
