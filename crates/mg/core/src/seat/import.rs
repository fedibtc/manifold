//! Destination effects are serialized by the seat owner. Native validation and
//! relay waits happen outside this queue; every returned result is fenced again.

use fedi_decentralized_service_manifold_guardian::{SeatTransferGrant, SignedRequest, Timestamp};
use secp256k1::XOnlyPublicKey;
use tokio::sync::oneshot;

use super::{FedimintApi, Seat, SeatCommand, SeatLoop, SeatVerbError};
use crate::backup::{SeatPublication, sync_directory, write_restored_seat_dir};
use crate::push_callback::ValidatedDkgCompletionCallback;
use crate::transfer_native::{ValidatedGuardian, check_installed_material, transfer_work_dir};
use crate::transfer_state::{IncomingState, IncomingTransfer, TransferMove, TransferPhase};

pub(super) enum ImportCommand {
    Admit {
        grant: SignedRequest<SeatTransferGrant>,
        service_key: XOnlyPublicKey,
        callback: Option<ValidatedDkgCompletionCallback>,
        reply: oneshot::Sender<Result<IncomingTransfer, SeatVerbError>>,
    },
    Expire {
        generation: u64,
        reply: oneshot::Sender<Result<(), SeatVerbError>>,
    },
    Install {
        generation: u64,
        binding: TransferMove,
        guardian: ValidatedGuardian,
        reply: oneshot::Sender<Result<SeatPublication, SeatVerbError>>,
    },
    Complete {
        generation: u64,
        expected: SeatPublication,
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

    pub(crate) async fn expire_transfer(&self, generation: u64) -> Result<(), SeatVerbError> {
        self.verb_request(|reply| SeatCommand::Import(ImportCommand::Expire { generation, reply }))
            .await
    }

    pub(crate) async fn install_transfer(
        &self,
        generation: u64,
        binding: TransferMove,
        guardian: ValidatedGuardian,
    ) -> Result<SeatPublication, SeatVerbError> {
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

    pub(crate) async fn complete_transfer(
        &self,
        generation: u64,
        expected: SeatPublication,
    ) -> Result<(), SeatVerbError> {
        self.verb_request(|reply| {
            SeatCommand::Import(ImportCommand::Complete {
                generation,
                expected,
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
            ImportCommand::Expire { generation, reply } => {
                let _ = reply.send(self.expire_transfer(generation).await);
            }
            ImportCommand::Install {
                generation,
                binding,
                guardian,
                reply,
            } => {
                let _ = reply.send(self.install_transfer(generation, &binding, guardian).await);
            }
            ImportCommand::Complete {
                generation,
                expected,
                reply,
            } => {
                let _ = reply.send(self.complete_transfer(generation, &expected).await);
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
        if !matches!(record.state, IncomingState::Imported { .. }) {
            self.stop_child().await.map_err(SeatVerbError::internal)?;
        }
        Ok(record)
    }

    async fn expire_transfer(&mut self, generation: u64) -> Result<(), SeatVerbError> {
        self.check_import_allowed()?;
        if self
            .db
            .expire_transfer(&self.facts.seat_id, generation)
            .await
            .map_err(SeatVerbError::internal)?
        {
            self.mirror_incoming(TransferPhase::Expired);
        }
        Ok(())
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

    async fn install_transfer(
        &mut self,
        generation: u64,
        binding: &TransferMove,
        guardian: ValidatedGuardian,
    ) -> Result<SeatPublication, SeatVerbError> {
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
        let bytes = guardian
            .material
            .encode()
            .map_err(SeatVerbError::internal)?;
        match &record.state {
            IncomingState::Waiting { .. } => {
                if !self
                    .db
                    .accept_transfer(
                        &self.facts.seat_id,
                        generation,
                        binding,
                        &guardian.material,
                        &guardian.invite,
                    )
                    .await
                    .map_err(SeatVerbError::internal)?
                {
                    return Err(SeatVerbError::SeatUnavailable);
                }
                self.mirror_incoming(TransferPhase::Importing);
            }
            IncomingState::Importing {
                material,
                invite_code,
                ..
            } if material == &bytes && invite_code == &guardian.invite => {}
            _ => return Err(SeatVerbError::SeatUnavailable),
        }
        self.stop_child().await.map_err(SeatVerbError::internal)?;
        self.keys = guardian.keys;
        self.fedimint_api = FedimintApi::new(
            self.fedimint_connectors.clone(),
            self.ports.api(),
            &self.keys.api_auth,
        );
        self.install_imported_material(&guardian.material, &bytes)
            .await?;
        self.db
            .record_formed(&self.facts.seat_id, &guardian.invite)
            .await
            .map_err(SeatVerbError::internal)?;
        self.state
            .send_modify(|state| state.formed_invite = Some(guardian.invite));
        self.backup.mark();
        self.transfer_publication().await
    }

    async fn install_imported_material(
        &self,
        material: &crate::transfer_package::TransferMaterial,
        bytes: &[u8],
    ) -> Result<(), SeatVerbError> {
        if self.final_data_exists()? {
            // A previous attempt may already have run the imported child and
            // created database files. Match authoritative identity, not an
            // exact directory listing. Never overwrite a conflicting config.
            check_installed_material(&self.process, self.facts.seat_no, bytes)
                .await
                .map_err(SeatVerbError::internal)?;
            // Byte equality does not prove that the preceding attempt synced
            // the files or the rename before it crashed.
            return self.sync_imported_material().await;
        }
        let work = transfer_work_dir(&self.process, self.facts.seat_no);
        tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&work)
            .await
            .map_err(SeatVerbError::internal)?;
        let staging = tempfile::Builder::new()
            .prefix("install-")
            .tempdir_in(&work)
            .map_err(SeatVerbError::internal)?;
        let staged_data = staging.path().join("data");
        write_restored_seat_dir(
            &staged_data,
            &material.archive,
            &material.archive.digest(),
            &material.api_auth,
        )
        .await
        .map_err(SeatVerbError::internal)?;
        sync_directory(staging.path())
            .await
            .map_err(SeatVerbError::internal)?;
        tokio::fs::rename(&staged_data, &self.data_dir)
            .await
            .map_err(SeatVerbError::internal)?;
        sync_directory(staging.path())
            .await
            .map_err(SeatVerbError::internal)?;
        self.sync_imported_material().await
    }

    async fn sync_imported_material(&self) -> Result<(), SeatVerbError> {
        for name in [
            "private.encrypt",
            "private.salt",
            "local.json",
            "consensus.json",
            "password.private",
        ] {
            sync_import_path(&self.data_dir.join(name))
                .await
                .map_err(SeatVerbError::internal)?;
        }
        for path in [
            self.data_dir.clone(),
            crate::seat_process::seat_dir(&self.process, self.facts.seat_no),
            self.process.data_root.join("seats"),
            self.process.data_root.clone(),
        ] {
            sync_import_path(&path)
                .await
                .map_err(SeatVerbError::internal)?;
        }
        Ok(())
    }

    async fn complete_transfer(
        &mut self,
        generation: u64,
        expected: &SeatPublication,
    ) -> Result<(), SeatVerbError> {
        let record = self.current_import(generation).await?;
        let IncomingState::Importing {
            material,
            invite_code,
            ..
        } = &record.state
        else {
            return if matches!(record.state, IncomingState::Imported { .. }) {
                Ok(())
            } else {
                Err(SeatVerbError::SeatUnavailable)
            };
        };
        self.require_current_publication(expected).await?;
        check_installed_material(&self.process, self.facts.seat_no, material)
            .await
            .map_err(SeatVerbError::internal)?;
        if self.state.borrow().formed_invite.as_ref() != Some(invite_code) {
            return Err(SeatVerbError::SeatUnavailable);
        }
        // An earlier attempt may have configured the child then failed its DB
        // commit. Reap before retrying; expiry cannot undo accepted custody.
        self.stop_child().await.map_err(SeatVerbError::internal)?;
        self.spawn_child_inner(Some(invite_code)).await?;
        if !self
            .db
            .complete_transfer(&self.facts.seat_id, generation)
            .await
            .map_err(SeatVerbError::internal)?
        {
            return Err(SeatVerbError::SeatUnavailable);
        }
        self.mirror_incoming(TransferPhase::Imported);
        self.schedule_watchdog(std::time::Duration::ZERO);
        Ok(())
    }
}

async fn sync_import_path(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(test)]
    if sync_fault::FAILURES.lock().unwrap().contains(path) {
        return Err(std::io::Error::other("injected import sync failure"));
    }
    tokio::fs::File::open(path).await?.sync_all().await
}

/// Path-scoped fault injection is absent from shipped builds. A guard keeps
/// parallel tests isolated and removes the fault even when an assertion fails.
#[cfg(test)]
pub(crate) mod sync_fault;
