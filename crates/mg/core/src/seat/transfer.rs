//! Source retirement/export/removal effects belong to the seat owner.
//! Relay confirmation belongs to the fleet caller, outside this command queue.
//!
//! The source never stops its guardian in the middle of a consensus session.
//! The destination starts the guardian with an empty database and joins the
//! federation's current session at round 0 with the same key; had this host
//! already signed units in that session, peers would see equivocation and
//! count the guardian as a forker until the session ends. The handover
//! therefore asks fedimintd to exit at the end of the session it is running,
//! confirms that the schedule landed before that session completed, and
//! commits the fence only once that exit is proved (SPEC-guardian-transfer).

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
                        "guardian transfer requested; stopping at the end of the current session"
                    );
                    self.handover = Some(Handover::new());
                    self.handover_at = Some(tokio::time::Instant::now());
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

/// How often the source asks its child about the session it waits for.
/// fedimintd stays up for a minute after its last session, so a few polls
/// observe that session's completion before the child exits.
const HANDOVER_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Bound on one local API request made from the seat queue.
const HANDOVER_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
/// fedimintd exits a minute after its last session so peers can finish it;
/// past this grace the MG stops the child itself.
const HANDOVER_EXIT_GRACE: Duration = Duration::from_secs(120);
/// Interval of the operator-visible event while a session does not end.
const HANDOVER_REPORT_INTERVAL: Duration = Duration::from_secs(60);
/// Schedule attempts per poll while sessions end under the request.
const HANDOVER_SCHEDULE_ATTEMPTS: usize = 3;

/// The source half of a transfer before its fence. In memory only: a daemon
/// restart forgets it together with fedimintd's own schedule, and the
/// destination's next `TransferSeat` starts it again.
pub(super) struct Handover {
    stage: HandoverStage,
    requested_at: Instant,
    reported_at: Instant,
}

#[derive(Clone, Copy, Debug)]
enum HandoverStage {
    /// The running child has not confirmed a schedule: none was made yet, or
    /// the child it was made with has exited and taken the schedule along.
    Requested,
    /// The child still reported `session` as running after the schedule
    /// landed, so it exits when `session` completes.
    Scheduled { session: u64 },
    /// `session` completed under the schedule: the child never starts a later
    /// session and must not run again. Its exit commits the fence.
    Closed { session: u64, since: Instant },
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

    pub(super) fn is_closed(&self) -> bool {
        matches!(self.stage, HandoverStage::Closed { .. })
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
    async fn handover_request<T>(
        request: impl std::future::Future<Output = Result<T, FedimintApiError>>,
    ) -> Result<T, &'static str> {
        match tokio::time::timeout(HANDOVER_REQUEST_TIMEOUT, request).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(handover_failure_kind(&error)),
            Err(_) => Err("timeout"),
        }
    }

    /// The session the child will stop after, once a read after the schedule
    /// still shows it running. A session that ends under the request leaves
    /// it unknown whether the schedule landed in time, so ask again; the
    /// at-or-after rule keeps every earlier schedule effective meanwhile.
    async fn schedule_child_shutdown(&self) -> Result<Option<u64>, &'static str> {
        let api = self.client();
        for _ in 0..HANDOVER_SCHEDULE_ATTEMPTS {
            let running = Self::handover_request(api.session_count()).await?;
            Self::handover_request(api.schedule_shutdown(running)).await?;
            if Self::handover_request(api.session_count()).await? == running {
                return Ok(Some(running));
            }
        }
        Ok(None)
    }

    pub(super) async fn handover_tick(&mut self) {
        let Some(stage) = self.handover.as_ref().map(|handover| handover.stage) else {
            return;
        };
        let next = match stage {
            HandoverStage::Requested => {
                if !matches!(self.child, ProcessSlot::Configured { .. }) {
                    None
                } else {
                    match self.schedule_child_shutdown().await {
                        Ok(Some(session)) => {
                            tracing::info!(
                                safe_to_share = true,
                                seat_id = %self.facts.seat_id,
                                session,
                                "guardian transfer scheduled fedimintd to stop after its session"
                            );
                            Some(HandoverStage::Scheduled { session })
                        }
                        Ok(None) => None,
                        Err(failure_kind) => {
                            tracing::warn!(
                                safe_to_share = true,
                                seat_id = %self.facts.seat_id,
                                failure_kind,
                                "guardian transfer could not schedule fedimintd shutdown"
                            );
                            None
                        }
                    }
                }
            }
            HandoverStage::Scheduled { session } => {
                match Self::handover_request(self.client().session_count()).await {
                    Ok(count) if count > session => {
                        tracing::info!(
                            safe_to_share = true,
                            seat_id = %self.facts.seat_id,
                            session,
                            "guardian transfer source completed its last session"
                        );
                        Some(HandoverStage::Closed {
                            session,
                            since: Instant::now(),
                        })
                    }
                    Ok(_) => None,
                    // An unanswered poll proves nothing; the child either
                    // answers later or exits, and an unobserved exit counts as
                    // a crash.
                    Err(_) => None,
                }
            }
            HandoverStage::Closed { since, .. } => {
                if since.elapsed() >= HANDOVER_EXIT_GRACE && self.child.is_live() {
                    // The child is past its last session; stopping it now
                    // cannot cut a session short.
                    if let Err(error) = self.stop_child().await {
                        tracing::warn!(seat_id = %self.facts.seat_id, error = format_args!("{error:#}"), "failed to stop guardian transfer source after its last session");
                    }
                }
                None
            }
        };
        if let Some(handover) = self.handover.as_mut()
            && let Some(next) = next
        {
            handover.stage = next;
        }
        if self.handover.as_ref().is_some_and(Handover::is_closed)
            && matches!(self.child, ProcessSlot::Empty)
        {
            self.commit_handover().await;
        }
        self.report_handover_wait();
        if self.handover.is_some() {
            self.handover_at = Some(Instant::now() + HANDOVER_POLL_INTERVAL);
        }
    }

    /// The handover cannot finish while the federation does not end the
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
        let (stage, session) = match handover.stage {
            HandoverStage::Requested => ("unscheduled", None),
            HandoverStage::Scheduled { session } => ("session_running", Some(session)),
            HandoverStage::Closed { session, .. } => ("awaiting_exit", Some(session)),
        };
        tracing::warn!(
            safe_to_share = true,
            seat_id = %self.facts.seat_id,
            stage,
            session = ?session,
            waited_secs = handover.requested_at.elapsed().as_secs(),
            "guardian transfer still waiting for the source session to end"
        );
    }

    /// The child is gone, by its own exit or a stop. Returns whether the
    /// handover owns that exit, in which case the child must not respawn.
    pub(super) async fn handover_child_gone(&mut self) -> bool {
        let Some(handover) = self.handover.as_mut() else {
            return false;
        };
        match handover.stage {
            HandoverStage::Closed { .. } => {
                self.commit_handover().await;
                true
            }
            HandoverStage::Scheduled { session } => {
                // Not observed past its session: treat it as a crash. The
                // respawned child resumes from its own database without a
                // fork, and the schedule is made again.
                tracing::warn!(
                    safe_to_share = true,
                    seat_id = %self.facts.seat_id,
                    session,
                    "guardian transfer source exited before its session was seen to end; scheduling again"
                );
                handover.stage = HandoverStage::Requested;
                false
            }
            HandoverStage::Requested => false,
        }
    }

    /// Commit the fence for a child whose exit after its last session is
    /// proved. A failed commit keeps the handover closed, which refuses every
    /// spawn, and retries on the next poll.
    async fn commit_handover(&mut self) {
        match self.db.retire_for_transfer(&self.facts.seat_id).await {
            Ok(true) => {
                self.handover = None;
                self.handover_at = None;
                self.mirror_outgoing(OutgoingTransfer::Stopped);
                tracing::info!(
                    safe_to_share = true,
                    seat_id = %self.facts.seat_id,
                    "guardian transfer source stopped at a session boundary"
                );
            }
            Ok(false) => {
                // The seat stopped qualifying for retirement (decommissioned
                // or otherwise refused). Nothing was handed over, so the
                // guardian may run again under ordinary supervision.
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
