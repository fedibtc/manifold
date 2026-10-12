//! Transfer relay waits run outside the seat queue. Commands on either side
//! recheck the durable fence and exact document before doing irreversible work.

use fedi_decentralized_service_manifold_guardian::{
    SeatTransferGrant, SignedRequest, Timestamp, TransferSeatResponse,
};

use super::{Fleet, Seat, SeatVerbError};
use std::sync::Arc;
use std::time::Duration;

impl Fleet {
    pub(crate) async fn transfer_seat(
        &self,
        signed: SignedRequest<SeatTransferGrant>,
        deadline: Duration,
    ) -> Result<TransferSeatResponse, SeatVerbError> {
        let grant = signed
            .verify(Timestamp((crate::db::now_ms() / 1000) as u64))
            .map_err(|_| SeatVerbError::TransferGrantMismatch)?;
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
        let sealed_package = seat.export_transfer(signed, key, expected).await?;
        Ok(TransferSeatResponse { sealed_package })
    }

    /// The caller already authorized this exact seat against its fresh FA
    /// RemoveSeat envelope. No network wait can hold the seat command queue.
    pub(crate) async fn remove_transferred_seat(
        &self,
        seat: &Arc<Seat>,
        deadline: Duration,
    ) -> Result<bool, SeatVerbError> {
        let removal = seat.begin_transfer_removal().await;
        self.advertisement_changed.notify_one();
        let Some(expected) = removal? else {
            return Ok(true);
        };
        let _pause = self.transfers.quiesce(&seat.facts().seat_id).await;
        self.backup
            .confirm_transfer_publication(&self.db, &expected, deadline)
            .await
            .map_err(|_| SeatVerbError::SeatUnavailable)?;
        seat.finish_transfer_removal(expected).await?;
        Ok(false)
    }
}
