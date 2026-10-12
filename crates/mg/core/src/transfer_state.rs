//! Durable transfer facts, independent of child processes and network tasks.
//!
//! Incoming acceptance and source retirement are monotone fences. Expiry only
//! cancels polling; it cannot undo acceptance or make a retired guardian runnable.
//! Source retirement does not pin a destination: the protocol explicitly relies
//! on an FA that does not issue conflicting transfers.

use fedi_decentralized_service_manifold_guardian::{
    FaId, FederationId, InviteCode, Locator, SeatId, SeatTransferGrant, ServiceStatus,
    SignedRequest,
};
use secp256k1::XOnlyPublicKey;
use serde::{Deserialize, Serialize};

/// The stable move identity, excluding renewal timestamps and signatures.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TransferMove {
    /// FA that owns both source and destination seats.
    #[serde(rename = "fi_id")]
    pub fa_id: FaId,
    /// Federation whose existing guardian is moved.
    pub federation_id: FederationId,
    /// Authenticated transport and service identity of the source.
    pub source: Locator,
    /// Original source seat.
    pub source_seat_id: SeatId,
    /// Destination service signing identity, not its attestation identity.
    pub destination_service_pubkey: XOnlyPublicKey,
    /// Bought destination seat.
    pub destination_seat_id: SeatId,
}

impl From<&SeatTransferGrant> for TransferMove {
    fn from(grant: &SeatTransferGrant) -> Self {
        Self {
            fa_id: grant.fa_id,
            federation_id: grant.federation_id.clone(),
            source: grant.source.clone(),
            source_seat_id: grant.source_seat_id.clone(),
            destination_service_pubkey: grant.destination_service_pubkey,
            destination_seat_id: grant.destination_seat_id.clone(),
        }
    }
}

/// Durable destination work; the accepted package lives in the same SQLite
/// transaction as `Importing`, so a crash cannot orphan an acceptance promise.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct IncomingTransfer {
    /// Incremented on each new polling attempt to reject delayed worker replies.
    pub(crate) generation: u64,
    /// Stable provenance retained after the live grant is discarded.
    pub(crate) binding: TransferMove,
    /// Confidential, non-authorizing commitment to the first callback choice.
    pub(crate) callback_commitment: Option<[u8; 32]>,
    /// Polling or irrevocably accepted installation work.
    pub(crate) state: IncomingState,
}

/// The incoming state owns the material appropriate to its crash boundary.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) enum IncomingState {
    /// Only this variant retains the live FA grant.
    Waiting {
        /// Original signed grant, presented byte-for-byte to the source.
        grant: SignedRequest<SeatTransferGrant>,
    },
    /// No package was accepted. This seat is not an ordinary free spare.
    Expired,
    /// Package accepted while valid; completion no longer depends on expiry.
    Importing {
        /// Bounded codec plaintext, confidential under data-root permissions.
        material: Vec<u8>,
        /// Immutable archive identity used by installation and backup barriers.
        archive_digest: String,
        /// Native-validated federation invite recoverable before local install.
        invite_code: InviteCode,
    },
    /// Installed and backup-confirmed; original auth remains needed on restart.
    Imported {
        /// The same immutable accepted material, never destination-derived keys.
        material: Vec<u8>,
        /// Original archive identity, preserved for subsequent transfers.
        archive_digest: String,
        /// Same immutable federation invite as the accepted package.
        invite_code: InviteCode,
    },
}

impl IncomingTransfer {
    /// Return accepted material without conflating waiting and accepted states.
    pub(crate) fn material(&self) -> Option<(&[u8], &str)> {
        match &self.state {
            IncomingState::Importing {
                material,
                archive_digest,
                ..
            }
            | IncomingState::Imported {
                material,
                archive_digest,
                ..
            } => Some((material, archive_digest)),
            IncomingState::Waiting { .. } | IncomingState::Expired => None,
        }
    }

    /// The accepted guardian's identity, even before formed-directory install.
    pub(crate) fn invite_code(&self) -> Option<&InviteCode> {
        match &self.state {
            IncomingState::Importing { invite_code, .. }
            | IncomingState::Imported { invite_code, .. } => Some(invite_code),
            _ => None,
        }
    }

    /// Cheap status projection; source retirement takes precedence over this.
    pub(crate) fn phase(&self) -> TransferPhase {
        match self.state {
            IncomingState::Waiting { .. } => TransferPhase::Waiting,
            IncomingState::Expired => TransferPhase::Expired,
            IncomingState::Importing { .. } => TransferPhase::Importing,
            IncomingState::Imported { .. } => TransferPhase::Imported,
        }
    }
}

impl std::fmt::Debug for IncomingTransfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IncomingTransfer(redacted)")
    }
}

/// The terminal source fence, retained independently of incoming provenance.
/// An imported guardian may later move again without forgetting its old keys.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum OutgoingTransfer {
    /// Retirement committed; child exit is not yet proved.
    Stopping,
    /// Child exit proved. Export still requires exact backup confirmation.
    Stopped,
    /// Deletion fence committed; no new export may begin.
    Removing,
    /// Tombstone confirmed and only guardian material deleted.
    Removed,
}

/// In-memory projection loaded before any possible guardian spawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransferPhase {
    Waiting,
    Expired,
    Importing,
    Imported,
    Outgoing(OutgoingTransfer),
}

impl TransferPhase {
    /// Transfer statuses override both formed-directory and missing-data status.
    pub(crate) fn service_status(self) -> Option<ServiceStatus> {
        Some(match self {
            Self::Waiting | Self::Importing => ServiceStatus::AwaitingTransfer,
            Self::Expired => ServiceStatus::TransferGrantExpired,
            Self::Imported => return None,
            Self::Outgoing(OutgoingTransfer::Removed) => ServiceStatus::Removed,
            Self::Outgoing(_) => ServiceStatus::Transferred,
        })
    }
}
