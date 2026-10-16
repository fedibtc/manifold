//! Guardian transfer messages
//! ([`SPEC-guardian-transfer`](../../mg/specs/SPEC-guardian-transfer.md)).
//!
//! The FA moves one formed guardian seat from a source MG to a seat it
//! bought on a destination MG. The destination pulls the guardian from the
//! source with an FA-signed [`SeatTransferGrant`], so neither the FA nor the
//! source has to be online at the moment the other is.

use std::fmt;

use fedi_decentralized_services::domain::{MgPeerAttestation, MgSeatBindings, MgSeatBindingsError};
use secp256k1::XOnlyPublicKey;

use crate::{
    DkgCompletionCallback, FaId, FederationId, FormationSeatBinding, GuardianFeeAccount,
    GuardianFeeRecipient, GuardianFeeRecipientListError, Locator, MetaConsensusBase, SeatId,
    SignedRequest, Timestamp, canonical_guardian_fee_recipient_list,
    signing::FA_REQUEST_FRESHNESS_WINDOW_SECS,
};

/// Longest validity an FA may give a [`SeatTransferGrant`].
pub const SEAT_TRANSFER_GRANT_MAX_LIFETIME_SECS: u64 = 30 * 24 * 60 * 60;

/// Largest guardian material a source returns: the total byte length of the
/// five [`GuardianMaterial`] fields. JSON escaping at most doubles it, which
/// keeps the response under the 1 MiB RPC frame.
pub const SEAT_TRANSFER_MATERIAL_MAX_BYTES: usize = 384 * 1024;

/// FA-signed authority to move one formed guardian seat to a destination seat.
///
/// Carried as a [`SignedRequest`]. Unlike every other FA request it is not a
/// one-hour challenge: it stays valid from `ts` until `expires_at`, because
/// the destination presents it to the source repeatedly until the source is
/// online. The source answers it with `TransferSeat`; the destination accepts
/// it through `PullSeat`.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SeatTransferGrant {
    /// Issue time.
    pub ts: Timestamp,

    /// Federation Ambassador identity; must own both seats.
    #[serde(rename = "fi_id")]
    pub fa_id: FaId,

    /// Federation whose guardian moves. The source checks it against its seat;
    /// the destination refuses a federation it already hosts.
    pub federation_id: FederationId,

    /// Source MG, as the destination dials it.
    pub source: Locator,

    /// Formed seat on the source MG.
    pub source_seat_id: SeatId,

    /// Destination MG service key; the source hands the guardian only to a
    /// `TransferSeat` call signed by it.
    pub destination_service_pubkey: XOnlyPublicKey,

    /// Bought, unformed seat on the destination MG.
    pub destination_seat_id: SeatId,

    /// Last second at which the destination asks the source and the source
    /// answers. A destination that accepted the guardian before then finishes
    /// the import after it. At most
    /// [`SEAT_TRANSFER_GRANT_MAX_LIFETIME_SECS`] after `ts`.
    pub expires_at: Timestamp,
}

impl SeatTransferGrant {
    /// Valid from issue, with the usual clock allowance, until `expires_at`.
    /// A grant whose lifetime exceeds the maximum is never valid.
    pub(crate) fn is_valid_at(&self, now: Timestamp) -> bool {
        self.ts.0 <= self.expires_at.0
            && self.expires_at.0 - self.ts.0 <= SEAT_TRANSFER_GRANT_MAX_LIFETIME_SECS
            && self.ts.0 <= now.0.saturating_add(FA_REQUEST_FRESHNESS_WINDOW_SECS)
            && now.0 <= self.expires_at.0
    }
}

/// Ask a bought `New` or `TransferGrantExpired` seat to pull a guardian from
/// its source MG.
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

    /// Federation Ambassador identity; must own the seat and sign the grant.
    #[serde(rename = "fi_id")]
    pub fa_id: FaId,

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

/// The destination's call to the source: the FA grant, signed by the
/// destination service key the grant names.
///
/// Carried as an [`MgSignedRequest`](crate::MgSignedRequest). The grant alone
/// authorizes the move; this envelope proves the caller is the destination
/// the FA chose, so the source hands the guardian only to it. The Iroh
/// connection keeps the answer confidential.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TransferSeatRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Signer: must equal the grant's `destination_service_pubkey`.
    pub destination_service_pubkey: XOnlyPublicKey,

    /// The FA grant, presented byte-for-byte as the destination received it.
    pub grant: SignedRequest<SeatTransferGrant>,
}

/// The source's answer to a [`TransferSeatRequest`].
///
/// The source has durably recorded `Transferred`, stopped its guardian, and
/// published `Transferred` in the seat's backup before it answers. Repeats
/// return the same guardian.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct TransferSeatResponse {
    pub material: GuardianMaterial,
}

/// The moved guardian's config files, exactly as `fedimintd` wrote them, and
/// the Iroh secrets the source MG ran it with. Confidential: it travels only
/// inside the Iroh connection between the two MGs, and `Debug` never prints
/// it. The guardian's admin password is not part of it: each MG runs the
/// child under its own derived `api_auth`.
#[derive(serde::Deserialize, serde::Serialize, Clone, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GuardianMaterial {
    /// `private.json`.
    pub private_json: String,
    /// `local.json`.
    pub local_json: String,
    /// `consensus.json`.
    pub consensus_json: String,
    /// The guardian's Iroh API secret key, which `private.json` also names.
    pub iroh_api_sk: [u8; 32],
    /// The guardian's Iroh P2P secret key, which `private.json` also names.
    pub iroh_p2p_sk: [u8; 32],
}

impl GuardianMaterial {
    /// Refuse material over [`SEAT_TRANSFER_MATERIAL_MAX_BYTES`].
    pub fn check_size(&self) -> Result<(), GuardianMaterialTooLarge> {
        let total = [&self.private_json, &self.local_json, &self.consensus_json]
            .iter()
            .try_fold(
                self.iroh_api_sk.len() + self.iroh_p2p_sk.len(),
                |total, field| total.checked_add(field.len()),
            )
            .ok_or(GuardianMaterialTooLarge)?;
        if SEAT_TRANSFER_MATERIAL_MAX_BYTES < total {
            return Err(GuardianMaterialTooLarge);
        }
        Ok(())
    }
}

impl fmt::Debug for GuardianMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GuardianMaterial(redacted)")
    }
}

/// Guardian material exceeds [`SEAT_TRANSFER_MATERIAL_MAX_BYTES`].
#[derive(Debug, thiserror::Error)]
#[error("guardian transfer material exceeds {SEAT_TRANSFER_MATERIAL_MAX_BYTES} bytes")]
pub struct GuardianMaterialTooLarge;

/// Vote to replace one peer's directory entry and fee recipient after a
/// transfer.
///
/// The binding is the destination's attestation and endpoint proof for the
/// moved peer. Each MG replaces only the entry with the same peer id and
/// that peer's recipient account, keeps every other field from
/// `expected_base`, and votes. Success means submitted, not live.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReplaceSeatBindingRequest {
    /// Freshness challenge timestamp (±1h window, SPEC-signed-envelopes).
    pub ts: Timestamp,

    /// Federation Ambassador identity.
    #[serde(rename = "fi_id")]
    pub fa_id: FaId,

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

    /// Federation Ambassador identity; must own the seat.
    #[serde(rename = "fi_id")]
    pub fa_id: FaId,

    /// Transferred seat to remove.
    pub seat_id: SeatId,
}

/// Outcome of a removal.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct RemoveSeatResponse {
    /// Whether the seat was already removed before this call.
    pub already_removed: bool,
}

/// The seat-binding directory and guardian-fee recipient list after a moved
/// peer's new attestation replaces its entry.
///
/// Every MG builds its `ReplaceSeatBinding` vote with this function, and the
/// FA predicts the readback with it, so both derive the same bytes from the
/// same consensus values. The peer's old fee account gives its recipient
/// share to the new account; every other entry and weight stays. Applying it
/// to values that already carry the attestation returns them unchanged.
pub fn rebind_transferred_seat(
    seat_bindings: &str,
    recipients: &str,
    attestation: &MgPeerAttestation,
) -> Result<(String, String), RebindSeatError> {
    let directory = MgSeatBindings::parse_canonical(seat_bindings)?;
    let peer_id = &attestation.attestation.peer_id;
    let old = directory
        .seat_bindings()
        .iter()
        .find(|entry| &entry.attestation.peer_id == peer_id)
        .ok_or(RebindSeatError::UnknownPeer)?;
    let old_account = old.attestation.guardian_fee_account.clone();
    let new_account =
        GuardianFeeAccount::try_from(attestation.attestation.guardian_fee_account.clone())
            .map_err(|_| RebindSeatError::InvalidFeeAccount)?;
    let directory = MgSeatBindings::new(directory.seat_bindings().iter().map(|entry| {
        if &entry.attestation.peer_id == peer_id {
            attestation.clone()
        } else {
            entry.clone()
        }
    }))?
    .canonical_string()?;

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RecipientList {
        version: u16,
        recipients: Vec<GuardianFeeRecipient>,
    }
    let list: RecipientList =
        serde_json::from_str(recipients).map_err(|_| RebindSeatError::InvalidRecipients)?;
    if list.version != 1 {
        return Err(RebindSeatError::InvalidRecipients);
    }
    let mut list = list.recipients;
    let recipient = list
        .iter_mut()
        .find(|recipient| recipient.account.as_account() == &old_account)
        .ok_or(RebindSeatError::MissingRecipient)?;
    recipient.account = new_account;
    list.sort_by_key(|recipient| recipient.account.as_account().id());
    let recipients = canonical_guardian_fee_recipient_list(&list)?;
    Ok((directory, recipients))
}

/// Why consensus values cannot be rebound to a moved peer.
#[derive(Debug, thiserror::Error)]
pub enum RebindSeatError {
    #[error("seat-binding directory is invalid: {0}")]
    Directory(#[from] MgSeatBindingsError),
    #[error("the attestation names a peer the directory does not hold")]
    UnknownPeer,
    #[error("the attestation's guardian-fee account is invalid")]
    InvalidFeeAccount,
    #[error("guardian-fee recipient list is invalid")]
    InvalidRecipients,
    #[error("guardian-fee recipient list has no share for the moved peer")]
    MissingRecipient,
    #[error("guardian-fee recipient list cannot be encoded: {0}")]
    RecipientList(#[from] GuardianFeeRecipientListError),
}

#[cfg(test)]
mod tests;
