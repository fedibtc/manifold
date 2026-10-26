//! The source side of a guardian transfer, owned by the seat loop.
//!
//! The destination retries `TransferSeat` until it gets the guardian. Each
//! call answers from durable state and never waits:
//!
//! 1. No fence: ask fedimintd to stop after its current session and answer
//!    `AwaitingSessionEnd`. When it exits with [`SESSION_BOUNDARY_EXIT_CODE`],
//!    its last session completed and no later one began, so the destination
//!    starts in a session this key never signed. That exit writes the fence,
//!    which also keeps the guardian from starting here again.
//! 2. Fence `stopped`, but the relays do not yet confirm a backup recording
//!    it: wake the backup worker and answer `AwaitingBackup`. Until then a
//!    restore from the relays could start the guardian here again.
//! 3. Confirmed: read the guardian's config files and Iroh secrets and
//!    return them.

use fedi_decentralized_service_manifold_guardian::{SeatTransferGrant, SignedRequest, Timestamp};
use secp256k1::Keypair;

use super::{ProcessSlot, Seat, SeatCommand, SeatLoop, SeatVerbError};
use crate::bundled_fedimintd::SESSION_BOUNDARY_EXIT_CODE;
use crate::fedimint_api::FedimintApiError;
use crate::seat_process::{effective_iroh_api_key, effective_iroh_p2p_key};
use crate::transfer_native::TransferMaterial;
use crate::transfer_state::{ImportedKeys, OutgoingTransfer, TransferPhase};

/// Bound on asking the local fedimintd; the destination's retry asks again.
const SHUTDOWN_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

impl Seat {
    pub(crate) async fn transfer_out(
        &self,
        grant: SignedRequest<SeatTransferGrant>,
        service_key: Keypair,
    ) -> Result<TransferMaterial, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::TransferOut {
            grant,
            service_key,
            reply,
        })
        .await
    }

    /// `Ok(true)` when the guardian material was already gone.
    pub(crate) async fn remove_transferred(&self) -> Result<bool, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::RemoveTransferred { reply })
            .await
    }
}

impl SeatLoop {
    fn check_source_grant(
        &self,
        signed: &SignedRequest<SeatTransferGrant>,
        service_key: &Keypair,
    ) -> Result<SeatTransferGrant, SeatVerbError> {
        self.state.borrow().reject_decommissioned()?;
        let grant = signed
            .verify(Timestamp((crate::db::now_ms() / 1000) as u64))
            .map_err(|_| SeatVerbError::TransferGrantMismatch)?
            .into_inner();
        let state = self.state.borrow();
        let federation = state
            .formed_invite
            .as_ref()
            .and_then(|invite| {
                invite
                    .0
                    .parse::<fedimint_core::invite_code::InviteCode>()
                    .ok()
            })
            .map(|invite| invite.federation_id().to_string());
        if grant.fa_id != self.facts.fa_id
            || grant.source_seat_id != self.facts.seat_id
            || grant.source.service_pubkey != service_key.x_only_public_key().0
            || federation.as_deref() != Some(grant.federation_id.0.as_str())
        {
            return Err(SeatVerbError::TransferGrantMismatch);
        }
        Ok(grant)
    }

    fn mirror_outgoing(&self, phase: OutgoingTransfer) {
        self.state
            .send_modify(|state| state.transfer_phase = Some(TransferPhase::Outgoing(phase)));
        self.transfer_wake.mark(&self.facts.seat_id);
        self.completion_hooks.mark();
        self.backup.mark();
    }

    /// The Iroh identity this seat's child runs under, which goes with the
    /// config to the destination.
    fn exported_keys(&self) -> ImportedKeys {
        ImportedKeys {
            iroh_api: effective_iroh_api_key(&self.keys, self.ports),
            iroh_p2p: iroh::SecretKey::from_bytes(&effective_iroh_p2p_key(&self.keys, self.ports)),
        }
    }

    pub(super) async fn transfer_out(
        &mut self,
        grant: &SignedRequest<SeatTransferGrant>,
        service_key: &Keypair,
    ) -> Result<TransferMaterial, SeatVerbError> {
        self.check_source_grant(grant, service_key)?;
        let outgoing = self
            .db
            .outgoing_transfer(&self.facts.seat_id)
            .await
            .map_err(SeatVerbError::internal)?;
        match outgoing {
            None => {
                // Preflight the export before the stop that retires this
                // source for good: an archive that cannot be exported, such
                // as one over the transfer size cap, must leave a working
                // guardian running. The export below reads it again.
                TransferMaterial::read(&self.data_dir, self.exported_keys())
                    .await
                    .map_err(SeatVerbError::internal)?;
                self.request_shutdown_after_current_session().await;
                return Err(SeatVerbError::AwaitingSessionEnd);
            }
            Some(OutgoingTransfer::Removed) => {
                return Err(SeatVerbError::WrongState {
                    status: TransferPhase::Outgoing(OutgoingTransfer::Removed)
                        .service_status()
                        .expect("outgoing has status"),
                });
            }
            Some(OutgoingTransfer::Stopped) => {}
        }
        let confirmed = self
            .db
            .backup_publication(&self.facts.seat_id, self.backup.format_version())
            .await
            .map_err(SeatVerbError::internal)?
            .and_then(|record| record.confirmed_outgoing);
        if confirmed != Some(OutgoingTransfer::Stopped) {
            self.backup.mark();
            return Err(SeatVerbError::AwaitingBackup);
        }
        let material = TransferMaterial::read(&self.data_dir, self.exported_keys())
            .await
            .map_err(SeatVerbError::internal)?;
        // The grant may have expired during the reads. Removal cannot win
        // between this check and the reply: both run in this queue.
        self.check_source_grant(grant, service_key)?;
        Ok(material)
    }

    async fn request_shutdown_after_current_session(&self) {
        if !matches!(self.child, ProcessSlot::Configured { .. }) {
            // Down or restarting: the retry after the respawn asks.
            return;
        }
        let failure_kind = match tokio::time::timeout(
            SHUTDOWN_REQUEST_TIMEOUT,
            self.client().shutdown_after_current_session(),
        )
        .await
        {
            Ok(Ok(())) => return,
            Ok(Err(FedimintApiError::Unreachable(_))) => "unreachable",
            Ok(Err(FedimintApiError::Rejected { .. })) => "rejected",
            Ok(Err(FedimintApiError::InvalidResponse { .. })) => "invalid_response",
            Err(_) => "timeout",
        };
        tracing::warn!(
            safe_to_share = true,
            seat_id = %self.facts.seat_id,
            failure_kind,
            "guardian transfer could not ask fedimintd to stop after its current session"
        );
    }

    /// Called for every observed child exit. Returns whether the guardian
    /// was handed over, in which case it must never run here again; the
    /// written fence blocks the respawn.
    ///
    /// fedimintd exits with [`SESSION_BOUNDARY_EXIT_CODE`] only when a
    /// `shutdown_after_current_session` request took effect at a session
    /// boundary, and only `TransferSeat` makes that request. A shutdown
    /// scheduled through fedimintd's older `shutdown` endpoint exits with a
    /// different code. Anyone with the guardian's admin password could call
    /// `shutdown_after_current_session` by hand and retire the seat; that
    /// risk is accepted, since the same password can already stop the
    /// guardian. Every other exit, such as a crash, a panic, a session
    /// timeout, or a kill by the watchdog or the operator, restarts the
    /// guardian from its own database, which continues its session rather
    /// than forking it, and the destination's next retry asks again.
    pub(super) async fn commit_session_boundary_exit(&mut self, exit_code: Option<i32>) -> bool {
        if exit_code != Some(SESSION_BOUNDARY_EXIT_CODE) {
            return false;
        }
        match self.db.retire_for_transfer(&self.facts.seat_id).await {
            Ok(true) => {
                self.mirror_outgoing(OutgoingTransfer::Stopped);
                tracing::info!(
                    safe_to_share = true,
                    seat_id = %self.facts.seat_id,
                    "guardian transfer source stopped between sessions"
                );
                true
            }
            // The seat no longer qualifies, for example after a
            // decommission; nothing was handed over.
            Ok(false) => false,
            Err(error) => {
                // Restarting continues the guardian's own session, which is
                // safe; the destination's retry asks for the exit again.
                tracing::warn!(seat_id = %self.facts.seat_id, %error, "failed to record guardian transfer stop");
                false
            }
        }
    }

    /// Delete only the guardian material of a stopped, exported source seat.
    /// The archive-less tombstone follows in the next backup publication; a
    /// restore in between resurrects at most a stopped guardian that never
    /// starts again, so no relay wait holds the seat command queue.
    pub(super) async fn remove_transferred(&mut self) -> Result<bool, SeatVerbError> {
        self.state.borrow().reject_decommissioned()?;
        match self.state.borrow().transfer_phase {
            Some(TransferPhase::Outgoing(OutgoingTransfer::Removed)) => return Ok(true),
            Some(TransferPhase::Outgoing(OutgoingTransfer::Stopped)) => {}
            _ => return Err(SeatVerbError::SeatUnavailable),
        }
        // Only the guardian data directory. Never the containing seat
        // directory, safe-event journal, fee wallet, or payment wallet.
        match tokio::fs::remove_dir_all(&self.data_dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(SeatVerbError::SeatUnavailable),
        }
        let seat_dir = crate::seat_process::seat_dir(&self.process, self.facts.seat_no);
        if seat_dir.exists() {
            crate::backup::sync_directory(&seat_dir)
                .await
                .map_err(SeatVerbError::internal)?;
        }
        if !self
            .db
            .record_transfer_removal(&self.facts.seat_id)
            .await
            .map_err(SeatVerbError::internal)?
        {
            return Err(SeatVerbError::SeatUnavailable);
        }
        self.mirror_outgoing(OutgoingTransfer::Removed);
        Ok(false)
    }
}
