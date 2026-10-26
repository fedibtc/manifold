//! Durable transfer facts, independent of child processes and network tasks.
//!
//! Incoming acceptance and source retirement are monotone fences. A grant has
//! no expiry: a waiting seat polls its source until it obtains the guardian.
//! Source retirement does not pin a destination: the protocol explicitly relies
//! on an FA that does not issue conflicting transfers.

use fedi_decentralized_service_manifold_guardian::{
    FaId, FederationId, Locator, SeatId, SeatTransferGrant, ServiceStatus, SignedRequest,
};
use secp256k1::XOnlyPublicKey;
use serde::{Deserialize, Serialize};

/// The stable move identity, excluding the grant's timestamp and signature.
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

/// Durable destination work. Acceptance commits in the same SQLite
/// transaction as the formed row, after the guardian files are on disk, so a
/// crash cannot leave an acceptance promise without its guardian.
#[derive(Clone)]
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

/// The incoming state owns only what its crash boundary needs: the live grant
/// while polling, and the guardian's Iroh secrets once accepted. The archive
/// itself lives only in the seat data directory and in the encrypted backup.
#[derive(Clone)]
pub(crate) enum IncomingState {
    /// Only this variant retains the live FA grant.
    Waiting {
        /// Original signed grant, presented byte-for-byte to the source.
        grant: SignedRequest<SeatTransferGrant>,
    },
    /// Guardian installed and running since; the keys remain needed on
    /// restart. Its backup follows asynchronously, as a
    /// natively formed seat's does.
    Imported(ImportedKeys),
}

/// The moved guardian's Iroh identity, which the destination cannot derive:
/// it is the peer the federation's consensus config names. Stored in the
/// accepted row and the backup document, never read back from `private.json`.
#[derive(Clone)]
pub(crate) struct ImportedKeys {
    pub(crate) iroh_api: iroh::SecretKey,
    pub(crate) iroh_p2p: iroh::SecretKey,
}

impl ImportedKeys {
    pub(crate) fn from_bytes(iroh_api: &[u8; 32], iroh_p2p: &[u8; 32]) -> Self {
        Self {
            iroh_api: iroh::SecretKey::from_bytes(iroh_api),
            iroh_p2p: iroh::SecretKey::from_bytes(iroh_p2p),
        }
    }

    /// The imported guardian keeps its own Iroh identity; the admin password
    /// is this MG's derived one, like every other seat's.
    pub(crate) fn seat_keys(&self, api_auth: String) -> crate::identity::SeatKeys {
        crate::identity::SeatKeys {
            iroh_api: self.iroh_api.clone(),
            iroh_p2p: self.iroh_p2p.clone(),
            api_auth,
            preserve_guardian_identity: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        self.iroh_api.to_bytes() == other.iroh_api.to_bytes()
            && self.iroh_p2p.to_bytes() == other.iroh_p2p.to_bytes()
    }
}

impl std::fmt::Debug for ImportedKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ImportedKeys(redacted)")
    }
}

impl IncomingTransfer {
    /// The accepted guardian's keys, without conflating waiting and accepted
    /// states.
    pub(crate) fn accepted(&self) -> Option<&ImportedKeys> {
        match &self.state {
            IncomingState::Imported(keys) => Some(keys),
            IncomingState::Waiting { .. } => None,
        }
    }

    /// Cheap status projection; source retirement takes precedence over this.
    pub(crate) fn phase(&self) -> TransferPhase {
        match &self.state {
            IncomingState::Waiting { .. } => TransferPhase::Waiting,
            IncomingState::Imported(_) => TransferPhase::Imported,
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
    /// The source proved its child's exit and committed the fence: the
    /// guardian never starts again, and its config may be exported.
    Stopped,
    /// Only the guardian material is deleted; the archive-less tombstone
    /// follows in the seat's next backup publication.
    Removed,
}

impl OutgoingTransfer {
    /// The value stored for this fence in the database.
    pub(crate) fn as_db_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Removed => "removed",
        }
    }

    pub(crate) fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "stopped" => Some(Self::Stopped),
            "removed" => Some(Self::Removed),
            _ => None,
        }
    }
}

/// In-memory projection loaded before any possible guardian spawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransferPhase {
    Waiting,
    Imported,
    Outgoing(OutgoingTransfer),
}

impl TransferPhase {
    /// Transfer statuses override both formed-directory and missing-data status.
    pub(crate) fn service_status(self) -> Option<ServiceStatus> {
        Some(match self {
            Self::Waiting => ServiceStatus::AwaitingTransfer,
            Self::Imported => return None,
            Self::Outgoing(OutgoingTransfer::Removed) => ServiceStatus::Removed,
            Self::Outgoing(OutgoingTransfer::Stopped) => ServiceStatus::Transferred,
        })
    }
}
