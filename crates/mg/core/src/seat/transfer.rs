//! Source retirement/export/removal effects belong to the seat owner.
//! Relay confirmation belongs to the fleet caller, outside this command queue.

use fedi_decentralized_service_manifold_guardian::{SeatTransferGrant, SignedRequest, Timestamp};
use secp256k1::Keypair;

use super::{Seat, SeatCommand, SeatLoop, SeatVerbError};
use crate::backup::{SeatPublication, seat_publication_plan};
use crate::seat_process::{effective_iroh_api_key, effective_iroh_p2p_key};
use crate::transfer_native::TransferMaterial;
use crate::transfer_state::{ImportedKeys, OutgoingTransfer, TransferPhase};

impl Seat {
    pub(crate) async fn retire_for_transfer(
        &self,
        grant: SignedRequest<SeatTransferGrant>,
        service_key: Keypair,
    ) -> Result<SeatPublication, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::RetireForTransfer {
            grant,
            service_key,
            reply,
        })
        .await
    }

    pub(crate) async fn export_transfer(
        &self,
        grant: SignedRequest<SeatTransferGrant>,
        service_key: Keypair,
        expected: SeatPublication,
    ) -> Result<TransferMaterial, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::ExportTransfer {
            grant,
            service_key,
            expected,
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

    pub(super) async fn transfer_publication(&self) -> Result<SeatPublication, SeatVerbError> {
        seat_publication_plan(&self.db, &self.process, &self.facts, None)
            .await
            .map_err(SeatVerbError::internal)
    }

    pub(super) async fn retire_for_transfer(
        &mut self,
        grant: &SignedRequest<SeatTransferGrant>,
        service_key: &Keypair,
    ) -> Result<SeatPublication, SeatVerbError> {
        self.check_source_grant(grant, service_key)?;
        let outgoing = self
            .db
            .outgoing_transfer(&self.facts.seat_id)
            .await
            .map_err(SeatVerbError::internal)?;
        match outgoing {
            Some(OutgoingTransfer::Removed) => {
                return Err(SeatVerbError::WrongState {
                    status: self
                        .state
                        .borrow()
                        .transfer_phase
                        .expect("outgoing state mirrored")
                        .service_status()
                        .expect("outgoing has status"),
                });
            }
            Some(OutgoingTransfer::Stopped) => {}
            None => {
                // Immutable archive availability/size is a precondition, not a
                // reason to stop a running guardian and then discover failure.
                TransferMaterial::read(&self.data_dir, self.exported_keys())
                    .await
                    .map_err(SeatVerbError::internal)?;
                // The fence records a proven exit: a failed stop leaves the
                // slot `ExitUnproven` and no row, and the FA's retry stops the
                // child again. A crash between the exit and the commit
                // restarts the guardian, which the retry stops as well; the
                // destination receives nothing until the row exists.
                self.stop_child().await.map_err(SeatVerbError::internal)?;
                if !self
                    .db
                    .retire_for_transfer(&self.facts.seat_id)
                    .await
                    .map_err(SeatVerbError::internal)?
                {
                    return Err(SeatVerbError::SeatUnavailable);
                }
                self.mirror_outgoing(OutgoingTransfer::Stopped);
            }
        }
        self.transfer_publication().await
    }

    pub(super) async fn export_transfer(
        &mut self,
        signed: &SignedRequest<SeatTransferGrant>,
        service_key: &Keypair,
        expected: &SeatPublication,
    ) -> Result<TransferMaterial, SeatVerbError> {
        self.check_source_grant(signed, service_key)?;
        // `Stopped` is recorded only after this seat proved its child's exit,
        // and no child starts under an outgoing fence. Never export while the
        // guardian may run.
        if self.state.borrow().transfer_phase
            != Some(TransferPhase::Outgoing(OutgoingTransfer::Stopped))
        {
            return Err(SeatVerbError::SeatUnavailable);
        }
        self.require_current_publication(expected).await?;
        let material = TransferMaterial::read(&self.data_dir, self.exported_keys())
            .await
            .map_err(SeatVerbError::internal)?;
        if expected.archive_digest() != Some(material.archive.digest().as_str()) {
            return Err(SeatVerbError::SeatUnavailable);
        }
        // Recheck expiry after disk/DB awaits, inside the queue: removal cannot
        // win its permanent fence between this check and export.
        self.check_source_grant(signed, service_key)?;
        Ok(material)
    }

    pub(super) async fn require_current_publication(
        &self,
        expected: &SeatPublication,
    ) -> Result<(), SeatVerbError> {
        let current = self.transfer_publication().await?;
        if current.doc_sha256() != expected.doc_sha256()
            || current.archive_digest() != expected.archive_digest()
        {
            return Err(SeatVerbError::SeatUnavailable);
        }
        // The fleet caller already waited. Verify that exact confirmation is
        // still present without doing network I/O inside the seat owner.
        let confirmed = self
            .db
            .backup_publication(&self.facts.seat_id, self.backup.format_version())
            .await
            .map_err(SeatVerbError::internal)?;
        if !confirmed.is_some_and(|record| {
            record.doc_sha256 == expected.doc_sha256()
                && record.archive_digest.as_deref() == expected.archive_digest()
        }) {
            return Err(SeatVerbError::SeatUnavailable);
        }
        Ok(())
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
