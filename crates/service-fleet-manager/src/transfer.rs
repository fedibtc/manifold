//! Guardian transfer messages
//! ([`SPEC-guardian-transfer`](../../fman/specs/SPEC-guardian-transfer.md)).
//!
//! The FI moves one formed guardian seat from a source FMan to a seat it
//! bought on a destination FMan. The destination pulls the guardian from the
//! source with an FI-signed [`SeatTransferGrant`], so neither the FI nor the
//! source has to be online at the moment the other is.

use std::fmt;

use secp256k1::XOnlyPublicKey;

use crate::{
    DkgCompletionCallback, FederationId, FiId, FormationSeatBinding, Locator, MetaConsensusBase,
    SeatId, SignedRequest, Timestamp, signing::FI_REQUEST_FRESHNESS_WINDOW_SECS,
};

/// Longest validity an FI may give a [`SeatTransferGrant`].
pub const SEAT_TRANSFER_GRANT_MAX_LIFETIME_SECS: u64 = 30 * 24 * 60 * 60;

/// Largest sealed guardian package a source returns. Hex doubles it on the
/// wire, which keeps the response under the 1 MiB RPC frame.
pub const SEALED_SEAT_PACKAGE_MAX_BYTES: usize = 384 * 1024;

/// FI-signed authority to move one formed guardian seat to a destination seat.
///
/// Carried as a [`SignedRequest`]. Unlike every other FI request it is not a
/// one-hour challenge: it stays valid from `ts` until `expires_at`, because
/// the destination presents it to the source repeatedly until the source is
/// online. The source answers it with `TransferSeat`; the destination accepts
/// it through `PullSeat`.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SeatTransferGrant {
    /// Issue time.
    pub ts: Timestamp,

    /// Federation Initiator identity; must own both seats.
    pub fi_id: FiId,

    /// Federation whose guardian moves. The source checks it against its seat;
    /// the destination refuses a federation it already hosts.
    pub federation_id: FederationId,

    /// Source FMan, as the destination dials it.
    pub source: Locator,

    /// Formed seat on the source FMan.
    pub source_seat_id: SeatId,

    /// Destination FMan service key; the source seals the guardian to it.
    pub destination_service_pubkey: XOnlyPublicKey,

    /// Bought, unformed seat on the destination FMan.
    pub destination_seat_id: SeatId,

    /// Last second at which either side acts on this grant. At most
    /// [`SEAT_TRANSFER_GRANT_MAX_LIFETIME_SECS`] after `ts`.
    pub expires_at: Timestamp,
}

impl SeatTransferGrant {
    /// Valid from issue, with the usual clock allowance, until `expires_at`.
    /// A grant whose lifetime exceeds the maximum is never valid.
    pub(crate) fn is_valid_at(&self, now: Timestamp) -> bool {
        self.ts.0 <= self.expires_at.0
            && self.expires_at.0 - self.ts.0 <= SEAT_TRANSFER_GRANT_MAX_LIFETIME_SECS
            && self.ts.0 <= now.0.saturating_add(FI_REQUEST_FRESHNESS_WINDOW_SECS)
            && now.0 <= self.expires_at.0
    }
}

/// Ask a bought `New` or `TransferGrantExpired` seat to pull a guardian from
/// its source FMan.
///
/// The seat becomes `AwaitingTransfer` and polls the source until it obtains
/// the guardian or the grant expires, then reports `TransferGrantExpired`. A
/// replay of the same grant succeeds; a different grant on an awaiting seat
/// is refused.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PullSeatRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity; must own the seat and sign the grant.
    pub fi_id: FiId,

    /// Destination seat; must equal the grant's `destination_seat_id`.
    pub seat_id: SeatId,

    /// Grant this seat presents to the source.
    pub grant: SignedRequest<SeatTransferGrant>,

    /// Bearer callback invoked once the pulled guardian runs. Same rules as
    /// the `StartDkg` callback.
    pub completion_callback: Option<DkgCompletionCallback>,
}

/// Acknowledges that the seat is awaiting the transfer.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct PullSeatResponse;

/// The source's answer to a [`SeatTransferGrant`].
///
/// The source has durably recorded `Transferred`, stopped its guardian, and
/// published `Transferred` in the seat's backup before it answers. Repeats return a fresh seal of the same guardian.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct TransferSeatResponse {
    /// The guardian's config and secrets, sealed to the grant's
    /// `destination_service_pubkey` and bound to both seat ids. The format
    /// belongs to the Fleet Manager on both ends.
    pub sealed_package: SealedSeatPackage,
}

/// Opaque sealed guardian package, hex on the wire.
#[derive(Clone, Eq, PartialEq)]
pub struct SealedSeatPackage(Vec<u8>);

impl SealedSeatPackage {
    /// Wrap sealed bytes; refuses more than [`SEALED_SEAT_PACKAGE_MAX_BYTES`].
    pub fn new(bytes: Vec<u8>) -> Result<Self, SealedSeatPackageTooLarge> {
        if SEALED_SEAT_PACKAGE_MAX_BYTES < bytes.len() {
            return Err(SealedSeatPackageTooLarge);
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SealedSeatPackage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SealedSeatPackage({} bytes)", self.0.len())
    }
}

impl serde::Serialize for SealedSeatPackage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(&self.0))
    }
}

impl<'de> serde::Deserialize<'de> for SealedSeatPackage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let hex = <String as serde::Deserialize>::deserialize(deserializer)?;
        if SEALED_SEAT_PACKAGE_MAX_BYTES * 2 < hex.len() {
            return Err(serde::de::Error::custom(SealedSeatPackageTooLarge));
        }
        let bytes = hex::decode(hex).map_err(serde::de::Error::custom)?;
        Ok(Self(bytes))
    }
}

/// A sealed package exceeds [`SEALED_SEAT_PACKAGE_MAX_BYTES`].
#[derive(Debug, thiserror::Error)]
#[error("sealed seat package exceeds {SEALED_SEAT_PACKAGE_MAX_BYTES} bytes")]
pub struct SealedSeatPackageTooLarge;

/// Vote to replace one peer's directory entry and fee recipient after a
/// transfer.
///
/// The binding is the destination's attestation and endpoint proof for the
/// moved peer. Each FMan replaces only the entry with the same peer id and
/// that peer's recipient account, keeps every other field from
/// `expected_base`, and votes. Success means submitted, not live.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReplaceSeatBindingRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity.
    pub fi_id: FiId,

    /// Seat whose guardian casts this vote.
    pub seat_id: SeatId,

    /// Exact consensus object on which this write is based.
    pub expected_base: MetaConsensusBase,

    /// New binding for the moved peer.
    pub seat_binding: FormationSeatBinding,
}

/// Acknowledges one submitted vote.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct ReplaceSeatBindingResponse;

/// Delete a `Transferred` seat's guardian data and backup.
///
/// Terminal and idempotent. The seat row and its fee-wallet data remain.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RemoveSeatRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Initiator identity; must own the seat.
    pub fi_id: FiId,

    /// Transferred seat to remove.
    pub seat_id: SeatId,
}

/// Outcome of a removal.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct RemoveSeatResponse {
    /// Whether the seat was already removed before this call.
    pub already_removed: bool,
}

#[cfg(test)]
mod tests;
