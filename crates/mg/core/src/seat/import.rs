//! Destination effects are serialized by the seat owner. Native validation
//! happens outside this queue; the install command fences its result again.

use fedi_decentralized_service_manifold_guardian::{SeatTransferGrant, SignedRequest, Timestamp};
use secp256k1::XOnlyPublicKey;
use tokio::sync::oneshot;

use super::{Seat, SeatCommand, SeatLoop, SeatVerbError};
use crate::backup::{read_guardian_archive, sync_directory, write_restored_seat_dir};
use crate::push_callback::ValidatedDkgCompletionCallback;
use crate::transfer_native::ValidatedGuardian;
use crate::transfer_state::{IncomingState, IncomingTransfer, TransferMove, TransferPhase};

pub(super) enum ImportCommand {
    Admit {
        grant: SignedRequest<SeatTransferGrant>,
        service_key: XOnlyPublicKey,
        callback: Option<ValidatedDkgCompletionCallback>,
        reply: oneshot::Sender<Result<IncomingTransfer, SeatVerbError>>,
    },
    Install {
        generation: u64,
        binding: TransferMove,
        guardian: ValidatedGuardian,
        reply: oneshot::Sender<Result<(), SeatVerbError>>,
    },
}

impl Seat {
    pub(crate) async fn admit_transfer(
        &self,
        grant: SignedRequest<SeatTransferGrant>,
        service_key: XOnlyPublicKey,
        callback: Option<ValidatedDkgCompletionCallback>,
    ) -> Result<IncomingTransfer, SeatVerbError> {
        self.verb_request(|reply| {
            SeatCommand::Import(ImportCommand::Admit {
                grant,
                service_key,
                callback,
                reply,
            })
        })
        .await
    }

    pub(crate) async fn install_transfer(
        &self,
        generation: u64,
        binding: TransferMove,
        guardian: ValidatedGuardian,
    ) -> Result<(), SeatVerbError> {
        self.verb_request(|reply| {
            SeatCommand::Import(ImportCommand::Install {
                generation,
                binding,
                guardian,
                reply,
            })
        })
        .await
    }
}

impl SeatLoop {
    pub(super) async fn handle_import_command(&mut self, command: ImportCommand) {
        match command {
            ImportCommand::Admit {
                grant,
                service_key,
                callback,
                reply,
            } => {
                let _ = reply.send(self.admit_transfer(&grant, service_key, callback).await);
            }
            ImportCommand::Install {
                generation,
                binding,
                guardian,
                reply,
            } => {
                let _ = reply.send(self.install_transfer(generation, &binding, guardian).await);
            }
        }
    }

    fn check_import_allowed(&self) -> Result<(), SeatVerbError> {
        let state = self.state.borrow();
        state.reject_decommissioned()?;
        if matches!(state.transfer_phase, Some(TransferPhase::Outgoing(_))) {
            return Err(SeatVerbError::SeatUnavailable);
        }
        Ok(())
    }

    fn mirror_incoming(&self, phase: TransferPhase) {
        self.state
            .send_modify(|state| state.transfer_phase = Some(phase));
        self.transfer_wake.mark(&self.facts.seat_id);
        self.backup.mark();
        self.completion_hooks.mark();
    }

    async fn admit_transfer(
        &mut self,
        signed: &SignedRequest<SeatTransferGrant>,
        service_key: XOnlyPublicKey,
        callback: Option<ValidatedDkgCompletionCallback>,
    ) -> Result<IncomingTransfer, SeatVerbError> {
        self.check_import_allowed()?;
        let grant = signed
            .verify(Timestamp((crate::db::now_ms() / 1000) as u64))
            .map_err(|_| SeatVerbError::TransferGrantMismatch)?;
        if grant.fa_id != self.facts.fa_id
            || grant.destination_seat_id != self.facts.seat_id
            || grant.destination_service_pubkey != service_key
        {
            return Err(SeatVerbError::TransferGrantMismatch);
        }
        let callback = callback.map(ValidatedDkgCompletionCallback::into_inner);
        let record = self
            .db
            .admit_transfer(signed, callback.as_ref())
            .await
            .map_err(SeatVerbError::internal)?
            .ok_or(SeatVerbError::TransferGrantMismatch)?;
        self.mirror_incoming(record.phase());
        // Admission is durable even when stopping the old parked child fails.
        // Accepted same-move replay must not stop a successfully imported child.
        if !matches!(record.state, IncomingState::Imported(_)) {
            self.stop_child().await.map_err(SeatVerbError::internal)?;
        }
        Ok(record)
    }

    async fn current_import(&self, generation: u64) -> Result<IncomingTransfer, SeatVerbError> {
        self.check_import_allowed()?;
        self.db
            .incoming_transfer(&self.facts.seat_id)
            .await
            .map_err(SeatVerbError::internal)?
            .filter(|record| record.generation == generation)
            .ok_or(SeatVerbError::SeatUnavailable)
    }

    /// Install the validated guardian, record acceptance and start its child.
    /// From here the seat is a formed seat whose backup follows asynchronously.
    async fn install_transfer(
        &mut self,
        generation: u64,
        binding: &TransferMove,
        guardian: ValidatedGuardian,
    ) -> Result<(), SeatVerbError> {
        let record = self.current_import(generation).await?;
        if &record.binding != binding {
            return Err(SeatVerbError::TransferGrantMismatch);
        }
        let invite = guardian
            .invite
            .0
            .parse::<fedimint_core::invite_code::InviteCode>()
            .map_err(|_| SeatVerbError::TransferGrantMismatch)?;
        if invite.federation_id().to_string() != binding.federation_id.0 {
            return Err(SeatVerbError::TransferGrantMismatch);
        }
        if !matches!(record.state, IncomingState::Waiting { .. }) {
            return Err(SeatVerbError::SeatUnavailable);
        }
        self.stop_child().await.map_err(SeatVerbError::internal)?;
        // The guardian keeps its Iroh identity; the admin password stays this
        // seat's derived one, so the API client needs no change.
        self.keys = guardian.material.keys.seat_keys(self.keys.api_auth.clone());
        // The guardian is durably on disk before acceptance is recorded, so an
        // accepted row always has its files; a crash before the commit leaves
        // files a replay of the same guardian adopts.
        self.install_imported_material(&guardian.material).await?;
        if !self
            .db
            .accept_transfer(
                &self.facts.seat_id,
                generation,
                binding,
                &guardian.material.keys,
                &guardian.invite,
            )
            .await
            .map_err(SeatVerbError::internal)?
        {
            return Err(SeatVerbError::SeatUnavailable);
        }
        self.publish_formed_invite(guardian.invite.clone());
        self.mirror_incoming(TransferPhase::Imported);
        // Acceptance is durable; a spawn failure is now an ordinary formed
        // seat outage that the respawn schedule and the watchdog recover.
        if let Err(error) = self.spawn_child_inner(Some(&guardian.invite)).await {
            tracing::warn!(seat_id = %self.facts.seat_id, error = format_args!("{error:#}"), "imported fedimintd spawn failed");
            self.schedule_respawn();
        }
        self.schedule_watchdog(std::time::Duration::ZERO);
        self.backup.mark();
        self.completion_hooks.mark();
        Ok(())
    }

    /// Stage the archive beside the final directory and rename it into place,
    /// so `data` only ever appears complete. A crash between the rename and
    /// the acceptance commit leaves this exact archive in place and nothing
    /// else can have: no row names the directory yet. A different archive
    /// there is refused, never replaced.
    async fn install_imported_material(
        &self,
        material: &crate::transfer_native::TransferMaterial,
    ) -> Result<(), SeatVerbError> {
        let staging = self.data_dir.with_file_name("data.staging");
        match tokio::fs::remove_dir_all(&staging).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(SeatVerbError::internal(error)),
        }
        if self.final_data_exists()? {
            let installed = read_guardian_archive(&self.data_dir)
                .await
                .map_err(SeatVerbError::internal)?
                .filter(|installed| installed.digest() == material.archive.digest());
            return match installed {
                Some(_) => Ok(()),
                None => Err(SeatVerbError::internal(anyhow::anyhow!(
                    "seat data directory holds a different guardian"
                ))),
            };
        }
        write_restored_seat_dir(&staging, &material.archive, &material.archive.digest())
            .await
            .map_err(SeatVerbError::internal)?;
        tokio::fs::rename(&staging, &self.data_dir)
            .await
            .map_err(SeatVerbError::internal)?;
        sync_directory(&crate::seat_process::seat_dir(
            &self.process,
            self.facts.seat_no,
        ))
        .await
        .map_err(SeatVerbError::internal)
    }
}
