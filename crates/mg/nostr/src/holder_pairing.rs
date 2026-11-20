//! The MG's private holder-enrollment service. Never publish its invitation
//! or accept a capability included in any Nostr event.

use std::sync::Arc;

use fedi_decentralized_manifold_environment::ManifoldEnvironmentProfile;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedi_decentralized_service_manifold_guardian::LinkSecret;
use fedi_decentralized_service_manifold_guardian::holder_pairing::*;
use hmac::{Hmac, Mac};
use mg_core::db::Db;
use nostr_sdk::{Event, JsonUtil, Keys, Kind, PublicKey};
use sha2::Sha256;

/// The process-local HMAC key makes a grant useless after restart/restore.
/// Only a digest, never a bearer or its key, is stored in SQLite.
#[derive(Clone)]
pub struct HolderPairing {
    db: Db,
    profile: ManifoldEnvironmentProfile,
    verifier: PeerBadgeVerifier,
    key: Arc<nostr_sdk::SecretKey>,
    live: Arc<tokio::sync::Mutex<Option<super::ManifoldGuardianNostr>>>,
}

impl HolderPairing {
    /// Construct a pairing authority for this process and deployment.
    pub fn new(db: Db, profile: ManifoldEnvironmentProfile) -> anyhow::Result<Self> {
        Ok(Self {
            db,
            verifier: PeerBadgeVerifier::try_from_profile(&profile)?,
            profile,
            key: Arc::new(Keys::generate().secret_key().clone()),
            live: Default::default(),
        })
    }

    /// Revalidate only the one pre-upgrade active event. Never fill missing
    /// evidence by querying a relay during startup or restore.
    pub async fn adopt_legacy(&self) -> anyhow::Result<()> {
        let Some(raw) = self.db.legacy_holder_evidence().await? else {
            return Ok(());
        };
        let mg = self.identity().await?;
        let verified = (|| -> anyhow::Result<PublicKey> {
            let ev = Event::from_json(&raw)?;
            anyhow::ensure!(
                ev.kind
                    == Kind::Custom(fedi_decentralized_nostr::mg::HOLDER_AUTHORIZATION_EVENT_KIND),
                "wrong event role"
            );
            let auth = super::verify_candidate(&ev, &mg)?;
            self.verifier.verify_issuance_at(
                &auth,
                super::holder_authorization_max_issued_at(super::now_secs())?,
            )?;
            let badge = fedi_decentralized_domain::parse_trust_score_badge_v1(
                &auth.signed_credential.credential,
            )?;
            anyhow::ensure!(badge.holder_pubkey == ev.pubkey, "wrong badge holder");
            Ok(ev.pubkey)
        })();
        if let Ok(holder) = verified {
            self.db.adopt_legacy_holder(holder, &raw).await?;
        }
        Ok(())
    }

    /// Attach the publisher after setup; reload under the same lock as direct
    /// submissions so a pairing during fleet startup cannot be missed.
    pub async fn attach(&self, service: super::ManifoldGuardianNostr) -> anyhow::Result<()> {
        let mut live = self.live.lock().await;
        service.reload_holder().await?;
        *live = Some(service);
        Ok(())
    }

    fn digest(&self, secret: &LinkSecret) -> [u8; 32] {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.key.as_secret_bytes())
            .expect("HMAC accepts any key length");
        mac.update(secret.as_bytes());
        mac.finalize().into_bytes().into()
    }

    async fn identity(&self) -> anyhow::Result<PublicKey> {
        Ok(self
            .db
            .load_identity()
            .await?
            .ok_or_else(|| anyhow::anyhow!("create or restore an MG identity first"))?
            .derive_service_nostr_keys()
            .public_key())
    }

    /// Issue one invitation to the authenticated local operator. Creating a
    /// second one invalidates the first immediately.
    pub async fn offer(&self, endpoint_id: String) -> anyhow::Result<HolderPairingInvite> {
        let subject_pubkey = self.identity().await?;
        let secret = LinkSecret::from_bytes(
            Keys::generate()
                .secret_key()
                .as_secret_bytes()
                .try_into()
                .expect("32-byte secret key"),
        );
        let expires_at_ms = i64::try_from((super::now_secs() + 300) * 1000)?;
        self.db
            .install_holder_grant(self.digest(&secret), expires_at_ms)
            .await?;
        Ok(HolderPairingInvite {
            protocol: HolderPairingProtocol::V1,
            environment: self.profile.environment().to_string(),
            subject_pubkey,
            endpoint_id,
            direct_address: None,
            secret,
            expires_at_ms,
        })
    }
}

impl HolderPairingService for HolderPairing {
    async fn submit_holder_authorization(
        &self,
        request: SubmitHolderAuthorization,
    ) -> PairingResult<HolderPairingReceipt> {
        let digest = self.digest(&request.secret);
        if !self
            .db
            .holder_grant_live(digest)
            .await
            .map_err(|_| HolderPairingError::Unavailable)?
        {
            return Err(HolderPairingError::InvalidInvite);
        }
        let invalid = HolderPairingError::InvalidAuthorization;
        if request.event_json.len() > 64 * 1024 {
            return Err(invalid);
        }
        let event = Event::from_json(&request.event_json).map_err(|_| invalid.clone())?;
        if event.kind != Kind::Custom(fedi_decentralized_nostr::mg::HOLDER_AUTHORIZATION_EVENT_KIND)
        {
            return Err(invalid);
        }
        let subject = self
            .identity()
            .await
            .map_err(|_| HolderPairingError::Unavailable)?;
        let authorization =
            super::verify_candidate(&event, &subject).map_err(|_| invalid.clone())?;
        let issued_at = authorization.holder_authorization.authorization.issued_at.0;
        let bound = super::holder_authorization_max_issued_at(super::now_secs())
            .map_err(|_| invalid.clone())?;
        self.verifier
            .verify_issuance_at(&authorization, bound)
            .map_err(|_| invalid.clone())?;
        let badge = fedi_decentralized_domain::parse_trust_score_badge_v1(
            &authorization.signed_credential.credential,
        )
        .map_err(|_| invalid.clone())?;
        if badge.holder_pubkey != event.pubkey {
            return Err(invalid);
        }
        let this = self.clone();
        // Transport cancellation cannot leave durable and live ownership split.
        tokio::spawn(async move {
            let live = this.live.lock().await;
            let _guard = if let Some(service) = live.as_ref() {
                Some(service.inner.authorization_refresh.lock().await)
            } else {
                None
            };
            if !this
                .db
                .commit_holder_pairing(digest, event.pubkey, issued_at, &event.as_json())
                .await
                .map_err(|_| HolderPairingError::Unavailable)?
            {
                return Err(HolderPairingError::InvalidInvite);
            }
            if let Some(live) = live.as_ref() {
                live.inner.project_holder(vec![authorization]);
            }
            Ok(HolderPairingReceipt { event_id: event.id })
        })
        .await
        .map_err(|_| HolderPairingError::Unavailable)?
    }
}
