//! Source retirement/export/removal effects belong to the seat owner.
//! Relay confirmation belongs to the fleet caller, outside this command queue.
//!
//! The source never stops its guardian in the middle of a consensus session.
//! The destination starts the guardian with an empty database and joins the
//! federation's current session at round 0 with the same key; had this host
//! already signed units in that session, peers would see equivocation and
//! count the guardian as a forker until the session ends. The handover
//! therefore asks fedimintd to exit once its current session completes, and
//! commits the fence only after the stopped guardian's database proves that
//! it exited between sessions (SPEC-guardian-transfer).

use std::time::Duration;

use fedi_decentralized_service_manifold_guardian::{
    SeatTransferGrant, ServiceStatus, SignedRequest, Timestamp,
};
use secp256k1::Keypair;
use tokio::time::Instant;

use super::{ProcessSlot, Seat, SeatCommand, SeatLoop, SeatVerbError};
use crate::backup::{SeatPublication, seat_publication_plan};
use crate::fedimint_api::FedimintApiError;
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
                // The guardian keeps running until its session ends. The
                // destination's retry collects the guardian once the handover
                // has committed the fence.
                if self.handover.is_none() {
                    tracing::info!(
                        safe_to_share = true,
                        seat_id = %self.facts.seat_id,
                        "guardian transfer requested; stopping between sessions"
                    );
                    self.handover = Some(Handover::new());
                    self.handover_at = Some(Instant::now());
                }
                return Err(SeatVerbError::WrongState {
                    status: ServiceStatus::Running,
                });
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

/// How often a waiting handover retries its request, reports, and retries a
/// failed fence commit.
const HANDOVER_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Bound on the local shutdown request made from the seat queue.
const HANDOVER_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
/// Interval of the operator-visible event while the handover waits.
const HANDOVER_REPORT_INTERVAL: Duration = Duration::from_secs(60);

/// The source half of a transfer before its fence. In memory only: a daemon
/// restart forgets it together with fedimintd's own schedule, and the
/// destination's next `TransferSeat` starts it again.
pub(super) struct Handover {
    stage: HandoverStage,
    requested_at: Instant,
    reported_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HandoverStage {
    /// No running child holds the request: none was made yet, or the child
    /// it was made with exited and took it along.
    Requested,
    /// The running child accepted the request and exits once its current
    /// session completes.
    Scheduled,
    /// The stopped guardian's database proves that it exited between
    /// sessions. It never runs again; only the fence commit may be pending.
    Confirmed,
}

impl HandoverStage {
    fn name(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Scheduled => "scheduled",
            Self::Confirmed => "confirmed",
        }
    }
}

impl Handover {
    pub(super) fn new() -> Self {
        let now = Instant::now();
        Self {
            stage: HandoverStage::Requested,
            requested_at: now,
            reported_at: now,
        }
    }

    /// The guardian completed its last session here and must never start
    /// another.
    pub(super) fn is_confirmed(&self) -> bool {
        self.stage == HandoverStage::Confirmed
    }
}

fn handover_failure_kind(error: &FedimintApiError) -> &'static str {
    match error {
        FedimintApiError::Unreachable(_) => "unreachable",
        FedimintApiError::Rejected { .. } => "rejected",
        FedimintApiError::InvalidResponse { .. } => "invalid_response",
    }
}

impl SeatLoop {
    pub(super) async fn handover_tick(&mut self) {
        let Some(stage) = self.handover.as_ref().map(|handover| handover.stage) else {
            return;
        };
        match stage {
            HandoverStage::Requested => match self.child {
                ProcessSlot::Configured { .. } => self.request_child_shutdown().await,
                // A guardian that is already down may have stopped between
                // sessions; then it hands over without running again.
                // Otherwise the pending respawn runs it, and the next tick
                // asks again.
                ProcessSlot::Empty => self.confirm_stopped_guardian().await,
                _ => {}
            },
            HandoverStage::Scheduled => {}
            HandoverStage::Confirmed => self.commit_handover().await,
        }
        self.report_handover_wait();
        if self.handover.is_some() {
            self.handover_at = Some(Instant::now() + HANDOVER_POLL_INTERVAL);
        }
    }

    async fn request_child_shutdown(&mut self) {
        let result = tokio::time::timeout(
            HANDOVER_REQUEST_TIMEOUT,
            self.client().shutdown_after_current_session(),
        )
        .await;
        let failure_kind = match result {
            Ok(Ok(())) => {
                if let Some(handover) = self.handover.as_mut() {
                    handover.stage = HandoverStage::Scheduled;
                }
                tracing::info!(
                    safe_to_share = true,
                    seat_id = %self.facts.seat_id,
                    "guardian transfer asked fedimintd to stop after its current session"
                );
                return;
            }
            Ok(Err(error)) => handover_failure_kind(&error),
            Err(_) => "timeout",
        };
        tracing::warn!(
            safe_to_share = true,
            seat_id = %self.facts.seat_id,
            failure_kind,
            "guardian transfer could not ask fedimintd to stop after its current session"
        );
    }

    /// The handover waits as long as the federation does not end the
    /// session, for example with too many peers offline. Say so instead of
    /// waiting silently, and never fall back to a mid-session stop.
    fn report_handover_wait(&mut self) {
        let Some(handover) = self.handover.as_mut() else {
            return;
        };
        if handover.reported_at.elapsed() < HANDOVER_REPORT_INTERVAL {
            return;
        }
        handover.reported_at = Instant::now();
        tracing::warn!(
            safe_to_share = true,
            seat_id = %self.facts.seat_id,
            stage = handover.stage.name(),
            waited_secs = handover.requested_at.elapsed().as_secs(),
            "guardian transfer still waiting for the source to stop between sessions"
        );
    }

    /// The child is gone, by its own exit or a stop. Returns whether the
    /// handover owns that exit, in which case the child must not respawn.
    pub(super) async fn handover_child_gone(&mut self) -> bool {
        if self.handover.is_none() {
            return false;
        }
        self.confirm_stopped_guardian().await;
        match self.handover.as_mut() {
            None => true,
            Some(handover) if handover.is_confirmed() => true,
            Some(handover) => {
                // Stopped inside a session: a crash. The respawned child
                // resumes that session from its own database, which is no
                // fork, and is asked again.
                tracing::warn!(
                    safe_to_share = true,
                    seat_id = %self.facts.seat_id,
                    "guardian transfer source stopped inside a session; restarting it and asking again"
                );
                handover.stage = HandoverStage::Requested;
                false
            }
        }
    }

    /// Requires a proved-empty process slot: no fedimintd owns the database.
    async fn confirm_stopped_guardian(&mut self) {
        match crate::guardian_db::stopped_between_sessions(&self.data_dir).await {
            Ok(true) => {
                if let Some(handover) = self.handover.as_mut() {
                    handover.stage = HandoverStage::Confirmed;
                }
                self.respawn_at = None;
                self.commit_handover().await;
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(seat_id = %self.facts.seat_id, error = format_args!("{error:#}"), "failed to read the stopped guardian database");
            }
        }
    }

    /// Commit the fence for a guardian proved stopped between sessions. A
    /// failed commit keeps the handover confirmed, which refuses every
    /// spawn, and retries on the next tick.
    async fn commit_handover(&mut self) {
        match self.db.retire_for_transfer(&self.facts.seat_id).await {
            Ok(true) => {
                self.handover = None;
                self.handover_at = None;
                self.mirror_outgoing(OutgoingTransfer::Stopped);
                tracing::info!(
                    safe_to_share = true,
                    seat_id = %self.facts.seat_id,
                    "guardian transfer source stopped between sessions"
                );
            }
            Ok(false) => {
                // The seat no longer qualifies for retirement, for example
                // after a decommission. Nothing was handed over, so ordinary
                // supervision applies again.
                self.handover = None;
                self.handover_at = None;
                self.schedule_respawn();
            }
            Err(error) => {
                tracing::warn!(seat_id = %self.facts.seat_id, %error, "failed to record guardian transfer stop");
                self.handover_at = Some(Instant::now() + HANDOVER_POLL_INTERVAL);
            }
        }
    }
}
