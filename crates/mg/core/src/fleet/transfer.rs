use fedi_decentralized_service_manifold_guardian::{
    Timestamp, TransferSeatRequest, TransferSeatResponse,
};

use super::{Fleet, Seat, SeatVerbError};
use crate::transfer_state::TransferPhase;
use std::sync::Arc;

impl Fleet {
    /// `request` carries the caller's verified service key. Only a caller
    /// holding the key the FA named in the grant receives the guardian,
    /// however it obtained the grant. Without `may_start`, only a seat that
    /// already stopped for a transfer answers. Destination PeerBadge admission
    /// still applies to every request, including retries after retirement.
    #[cfg(test)]
    pub(crate) async fn transfer_seat(
        &self,
        request: TransferSeatRequest,
        may_start: bool,
    ) -> Result<TransferSeatResponse, SeatVerbError> {
        self.transfer_seat_with_admission(request, None, may_start)
            .await
    }

    /// Authenticate destination trust on every stop/release, using direct proof
    /// when present and complete legacy discovery only when absent.
    pub(crate) async fn transfer_seat_with_admission(
        &self,
        request: TransferSeatRequest,
        admission: Option<fedi_decentralized_service_manifold_guardian::DestinationAdmission>,
        may_start: bool,
    ) -> Result<TransferSeatResponse, SeatVerbError> {
        let signed = request.grant;
        let grant = signed
            .verify(Timestamp((crate::db::now_ms() / 1000) as u64))
            .map_err(|_| SeatVerbError::TransferGrantMismatch)?;
        if grant.destination_service_pubkey != request.destination_service_pubkey {
            return Err(SeatVerbError::TransferGrantMismatch);
        }
        let seat = self
            .seat_by_id(&grant.source_seat_id)
            .ok_or(SeatVerbError::UnknownSeat)?;
        if seat.facts().fa_id != grant.fa_id {
            return Err(SeatVerbError::UnknownSeat);
        }
        if grant.source.service_pubkey != self.identity.derive_service_pubkey() {
            return Err(SeatVerbError::TransferGrantMismatch);
        }
        if !may_start && !matches!(seat.transfer_phase(), Some(TransferPhase::Outgoing(_))) {
            return Err(SeatVerbError::TransferDisabled);
        }
        // The FA owns the seat, not the authority to bypass the source's trust
        // policy. Check before both the irreversible stop and every release.
        // Never queue here: queued handlers would still occupy the transport's
        // shared RPC permits and starve unrelated seat operations.
        let _admission = self
            .replacement_admissions
            .try_acquire()
            .map_err(|_| SeatVerbError::ReplacementGuardianUnverified)?;
        self.config
            .replacement_guardian_verifier
            .verify(grant.destination_service_pubkey, admission.as_ref())
            .await
            .map_err(|_| SeatVerbError::ReplacementGuardianUnverified)?;
        let key = self.identity.derive_service_signing_key();
        let material = seat.transfer_out(signed, key).await;
        // The guardian's stop, which frees its capacity, lands between
        // retries.
        self.advertisement_changed.notify_one();
        Ok(TransferSeatResponse {
            material: material?.into_wire(),
        })
    }

    /// The caller already authorized this exact seat against its fresh FA
    /// RemoveSeat envelope. A failed attempt leaves the seat stopped; the
    /// caller retries.
    pub(crate) async fn remove_transferred_seat(
        &self,
        seat: &Arc<Seat>,
    ) -> Result<bool, SeatVerbError> {
        let removed = seat.remove_transferred().await;
        self.advertisement_changed.notify_one();
        removed
    }
}
