//! The source's relay wait runs outside the seat queue. The export command
//! rechecks the durable fence and exact document before releasing material.

use fedi_decentralized_service_manifold_guardian::{
    Timestamp, TransferSeatRequest, TransferSeatResponse,
};

use super::{Fleet, Seat, SeatVerbError};
use std::sync::Arc;
use std::time::Duration;

impl Fleet {
    /// `request` carries the caller's verified service key. Only a caller
    /// holding the key the FA named in the grant receives the guardian,
    /// however it obtained the grant.
    pub(crate) async fn transfer_seat(
        &self,
        request: TransferSeatRequest,
        deadline: Duration,
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
        let key = self.identity.derive_service_signing_key();
        let retirement = seat.retire_for_transfer(signed.clone(), key).await;
        self.advertisement_changed.notify_one();
        let expected = retirement?;
        self.backup
            .confirm_transfer_publication(&self.db, &expected, deadline)
            .await
            .map_err(|_| SeatVerbError::SeatUnavailable)?;
        let material = seat.export_transfer(signed, key, expected).await?;
        Ok(TransferSeatResponse {
            material: material.into_wire(),
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
