//! Operator issuance of SeatGrants and the refusal sets their redemption
//! checks ([SPEC-seat-grant](../../../specs/SPEC-seat-grant.md)).
//!
//! The list of issued grants grants nothing: a grant is authorized by the
//! MG's signature over it, and refused by its nonce being used or revoked.
//! The list exists so the operator can see each grant's state, show an unused
//! grant's code again, and revoke one.

use anyhow::Context as _;
use fedi_decentralized_service_manifold_guardian::{
    Locator, ManagerSignature, OfferEpoch, SeatGrant, SeatGrantCode, SeatGrantLabel,
    SeatGrantNonce, SeatId, SignedResponse, Timestamp,
};
use fedi_iroh_rpc::iroh::EndpointAddr;

use super::Fleet;
use crate::backup::MAX_REVOKED_SEAT_GRANTS;
use crate::db::{SeatGrantRevocation, StoredSeatGrant, StoredSeatGrantState};

/// One grant in the operator's list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssuedSeatGrant {
    pub grant: SeatGrant,
    pub status: IssuedSeatGrantStatus,
}

/// Where an issued grant stands. Only an unused grant has a code to share.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IssuedSeatGrantStatus {
    /// Redeemable by whoever holds the code.
    Unused { code: Box<SeatGrantCode> },
    /// A seat was created from it.
    Used { seat_id: SeatId },
    /// The operator revoked it before anyone redeemed it.
    Revoked,
}

impl Fleet {
    /// Sign and record a new grant for the operator to share.
    pub async fn create_seat_grant(
        &self,
        label: Option<SeatGrantLabel>,
    ) -> anyhow::Result<IssuedSeatGrant> {
        let grant = SeatGrant {
            grant_nonce: SeatGrantNonce::from_bytes(rand::random()),
            issued_at: Timestamp(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("system clock is after the Unix epoch")
                    .as_secs(),
            ),
            label,
        };
        let signed = SignedResponse::create(&grant, &self.identity.derive_service_signing_key())?;
        let (payload, signature) = signed.as_parts();
        let signature = signature.0.to_byte_array();
        self.db
            .insert_seat_grant(&grant.grant_nonce, payload, &signature)
            .await?;
        self.issued_seat_grant(StoredSeatGrant {
            payload: payload.to_vec(),
            signature,
            state: StoredSeatGrantState::Unused,
        })
    }

    /// Every grant this install issued, oldest first.
    pub async fn seat_grants(&self) -> anyhow::Result<Vec<IssuedSeatGrant>> {
        self.db
            .seat_grants()
            .await?
            .into_iter()
            .map(|stored| self.issued_seat_grant(stored))
            .collect()
    }

    /// Revoke one unused grant this install issued. Repeating it succeeds; a
    /// used grant has nothing left to revoke and is refused.
    pub async fn revoke_seat_grant(
        &self,
        grant_nonce: &SeatGrantNonce,
    ) -> anyhow::Result<IssuedSeatGrant> {
        match self
            .db
            .revoke_seat_grant(grant_nonce, MAX_REVOKED_SEAT_GRANTS)
            .await?
        {
            SeatGrantRevocation::Revoked => {}
            SeatGrantRevocation::AlreadyUsed => {
                anyhow::bail!("this guardian code was already used to create a seat")
            }
            SeatGrantRevocation::Unknown => {
                anyhow::bail!("this Manifold Guardian issued no guardian code with that nonce")
            }
            SeatGrantRevocation::TooMany => anyhow::bail!(
                "this Manifold Guardian has revoked {MAX_REVOKED_SEAT_GRANTS} guardian codes, \
                 as many as its backup can carry"
            ),
        }
        // Hint only: the revocation is durable now, and its backup document
        // converges on the worker's next scan.
        self.backup.mark();
        let stored = self
            .db
            .seat_grant(grant_nonce)
            .await?
            .context("a revoked grant stays in the list")?;
        self.issued_seat_grant(stored)
    }

    /// Whether a presented grant's nonce is already used or revoked.
    pub(crate) async fn is_seat_grant_refused(
        &self,
        grant_nonce: &SeatGrantNonce,
    ) -> anyhow::Result<bool> {
        Ok(self.db.is_seat_grant_refused(grant_nonce).await?)
    }

    /// The offer epoch to quote a grant at, or `None` without a free slot.
    ///
    /// A grant skips the readiness gate: its checks decide fitness for public
    /// selling, and an MG with a connectivity or Bitcoin problem can still
    /// take a seat its operator gave away, whose DKG then waits until the MG
    /// is healthy. Capacity still applies, and the epoch still binds the
    /// quote to the offer it was issued under.
    pub(crate) async fn seat_grant_quote_epoch(&self) -> Option<OfferEpoch> {
        let snapshot = self
            .db
            .offer_snapshot(self.config.first_port_base)
            .await
            .expect("offer snapshot");
        (snapshot.slots > 0).then_some(snapshot.offer.epoch)
    }

    /// Rebuild a stored grant into the operator's view: the verified grant,
    /// and for an unused one the code exactly as it was issued.
    fn issued_seat_grant(&self, stored: StoredSeatGrant) -> anyhow::Result<IssuedSeatGrant> {
        let signature = secp256k1::schnorr::Signature::from_byte_array(stored.signature);
        let signed =
            SignedResponse::<SeatGrant>::from_parts(stored.payload, ManagerSignature(signature));
        let locator = Locator::new(
            EndpointAddr::new(self.identity.derive_iroh_secret_key().public()),
            self.identity.derive_service_pubkey(),
        );
        let code = SeatGrantCode::new(self.config.manifold_environment, locator, signed)
            .context("a stored seat grant does not verify under this MG's key")?;
        let grant = code.grant().clone();
        let status = match stored.state {
            StoredSeatGrantState::Unused => IssuedSeatGrantStatus::Unused {
                code: Box::new(code),
            },
            StoredSeatGrantState::Used { seat_id } => IssuedSeatGrantStatus::Used { seat_id },
            StoredSeatGrantState::Revoked => IssuedSeatGrantStatus::Revoked,
        };
        Ok(IssuedSeatGrant { grant, status })
    }
}
