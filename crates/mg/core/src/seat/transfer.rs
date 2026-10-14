//! Source retirement/export/removal effects belong to the seat owner.
//! Relay confirmation belongs to the fleet caller, outside this command queue.

use fedi_decentralized_service_manifold_guardian::{
    SealedSeatPackage, SeatTransferGrant, SignedRequest, Timestamp,
};
use secp256k1::Keypair;

use super::{Seat, SeatCommand, SeatLoop, SeatVerbError};
use crate::backup::{SeatPublication, seat_publication_plan};
use crate::transfer_package::TransferMaterial;
use crate::transfer_state::{OutgoingTransfer, TransferPhase};

impl Seat {
    pub(crate) async fn resume_transfer_stop(&self) -> Result<(), SeatVerbError> {
        self.verb_request(|reply| SeatCommand::ResumeTransferStop { reply })
            .await
    }
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
    ) -> Result<SealedSeatPackage, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::ExportTransfer {
            grant,
            service_key,
            expected,
            reply,
        })
        .await
    }

    pub(crate) async fn begin_transfer_removal(
        &self,
    ) -> Result<Option<SeatPublication>, SeatVerbError> {
        self.verb_request(|reply| SeatCommand::BeginTransferRemoval { reply })
            .await
    }

    pub(crate) async fn finish_transfer_removal(
        &self,
        expected: SeatPublication,
    ) -> Result<(), SeatVerbError> {
        self.verb_request(|reply| SeatCommand::FinishTransferRemoval { expected, reply })
            .await
    }
}

impl SeatLoop {
    pub(super) async fn resume_transfer_stop(&mut self) -> Result<(), SeatVerbError> {
        match self
            .db
            .outgoing_transfer(&self.facts.seat_id)
            .await
            .map_err(SeatVerbError::internal)?
        {
            Some(OutgoingTransfer::Stopping) => {
                self.stop_child().await.map_err(SeatVerbError::internal)?;
                self.advance_outgoing(OutgoingTransfer::Stopping, OutgoingTransfer::Stopped)
                    .await
            }
            _ => Ok(()),
        }
    }
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

    async fn advance_outgoing(
        &self,
        from: OutgoingTransfer,
        to: OutgoingTransfer,
    ) -> Result<(), SeatVerbError> {
        if !self
            .db
            .advance_retirement(&self.facts.seat_id, from, to)
            .await
            .map_err(SeatVerbError::internal)?
        {
            return Err(SeatVerbError::SeatUnavailable);
        }
        // No suspension between durable transition and runtime fence.
        self.mirror_outgoing(to);
        Ok(())
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
            Some(OutgoingTransfer::Removing | OutgoingTransfer::Removed) => {
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
            Some(_) => {}
            None => {
                // Immutable archive availability/size is a precondition, not a
                // reason to retire a running guardian and then discover failure.
                TransferMaterial::read(&self.data_dir, self.keys.api_auth.clone())
                    .await
                    .map_err(SeatVerbError::internal)?;
                if !self
                    .db
                    .retire_for_transfer(&self.facts.seat_id)
                    .await
                    .map_err(SeatVerbError::internal)?
                {
                    return Err(SeatVerbError::SeatUnavailable);
                }
                self.mirror_outgoing(OutgoingTransfer::Stopping);
            }
        }
        if self.state.borrow().transfer_phase
            == Some(TransferPhase::Outgoing(OutgoingTransfer::Stopping))
        {
            self.stop_child().await.map_err(SeatVerbError::internal)?;
            self.advance_outgoing(OutgoingTransfer::Stopping, OutgoingTransfer::Stopped)
                .await?;
        }
        self.transfer_publication().await
    }

    pub(super) async fn export_transfer(
        &mut self,
        signed: &SignedRequest<SeatTransferGrant>,
        service_key: &Keypair,
        expected: &SeatPublication,
    ) -> Result<SealedSeatPackage, SeatVerbError> {
        let grant = self.check_source_grant(signed, service_key)?;
        if self.state.borrow().transfer_phase
            != Some(TransferPhase::Outgoing(OutgoingTransfer::Stopped))
        {
            return Err(SeatVerbError::SeatUnavailable);
        }
        self.require_current_publication(expected).await?;
        let material = TransferMaterial::read(&self.data_dir, self.keys.api_auth.clone())
            .await
            .map_err(SeatVerbError::internal)?;
        if expected.archive_digest() != Some(material.archive.digest().as_str()) {
            return Err(SeatVerbError::SeatUnavailable);
        }
        // Recheck expiry after disk/DB awaits. Seal inside the queue: removal
        // cannot win its permanent fence between this check and export.
        self.check_source_grant(signed, service_key)?;
        crate::transfer_package::seal(&material, &grant, service_key)
            .map_err(SeatVerbError::internal)
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

    pub(super) async fn begin_transfer_removal(
        &mut self,
    ) -> Result<Option<SeatPublication>, SeatVerbError> {
        self.state.borrow().reject_decommissioned()?;
        let outgoing = self
            .db
            .outgoing_transfer(&self.facts.seat_id)
            .await
            .map_err(SeatVerbError::internal)?;
        match outgoing {
            None => return Err(SeatVerbError::SeatUnavailable),
            Some(OutgoingTransfer::Removed) => return Ok(None),
            Some(OutgoingTransfer::Stopping) => {
                self.stop_child().await.map_err(SeatVerbError::internal)?;
                self.advance_outgoing(OutgoingTransfer::Stopping, OutgoingTransfer::Stopped)
                    .await?;
                self.advance_outgoing(OutgoingTransfer::Stopped, OutgoingTransfer::Removing)
                    .await?;
            }
            Some(OutgoingTransfer::Stopped) => {
                self.advance_outgoing(OutgoingTransfer::Stopped, OutgoingTransfer::Removing)
                    .await?;
            }
            Some(OutgoingTransfer::Removing) => {}
        }
        Ok(Some(self.transfer_publication().await?))
    }

    pub(super) async fn finish_transfer_removal(
        &mut self,
        expected: &SeatPublication,
    ) -> Result<(), SeatVerbError> {
        self.state.borrow().reject_decommissioned()?;
        if self.state.borrow().transfer_phase
            == Some(TransferPhase::Outgoing(OutgoingTransfer::Removed))
        {
            return Ok(());
        }
        if self.state.borrow().transfer_phase
            != Some(TransferPhase::Outgoing(OutgoingTransfer::Removing))
        {
            return Err(SeatVerbError::SeatUnavailable);
        }
        self.require_current_publication(expected).await?;
        if expected.archive_digest().is_some() {
            return Err(SeatVerbError::SeatUnavailable);
        }
        // Only these two guardian-specific paths. Never the containing seat
        // directory, safe-event journal, fee wallet, or payment wallet.
        for path in [
            self.data_dir.clone(),
            crate::transfer_native::transfer_work_dir(&self.process, self.facts.seat_no),
        ] {
            match tokio::fs::remove_dir_all(path).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(SeatVerbError::SeatUnavailable),
            }
        }
        let seat_dir = crate::seat_process::seat_dir(&self.process, self.facts.seat_no);
        if seat_dir.exists() {
            crate::backup::sync_directory(&seat_dir)
                .await
                .map_err(SeatVerbError::internal)?;
        }
        if !self
            .db
            .finish_transfer_removal(&self.facts.seat_id)
            .await
            .map_err(SeatVerbError::internal)?
        {
            return Err(SeatVerbError::SeatUnavailable);
        }
        self.mirror_outgoing(OutgoingTransfer::Removed);
        Ok(())
    }
}
