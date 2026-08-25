//! Intent-level delegation to the wallet-owned payout orchestrator.
use super::Fleet;
use crate::payout_wire::{PayoutJobStatusWire, PayoutJobWire};
use crate::wallet::PayoutRequestId;
use fedi_decentralized_service_fleet_manager::{FederationId, SeatId};
impl Fleet {
    pub async fn payout_payment_fees(
        &self,
        federation_id: &FederationId,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobWire> {
        self.payouts
            .sweep_payment_fees(federation_id, request_id)
            .await
    }
    pub async fn payout_guardian_fees(
        &self,
        seat_id: &SeatId,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobWire> {
        if let Some(job) = self
            .payouts
            .resume_guardian_sweep(seat_id, request_id)
            .await?
        {
            return Ok(job);
        }
        let invite = fedi_decentralized_service_fleet_manager::InviteCode(
            self.seat_federation(seat_id).await?.to_string(),
        );
        self.payouts
            .sweep_guardian_fees(&invite, seat_id, request_id)
            .await
    }
    pub async fn payout_job_status(
        &self,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobStatusWire> {
        self.payouts.payout_status(request_id).await
    }
    pub async fn await_payout_job(
        &self,
        request_id: &PayoutRequestId,
    ) -> anyhow::Result<PayoutJobStatusWire> {
        self.payouts.await_payout(request_id).await
    }
}
