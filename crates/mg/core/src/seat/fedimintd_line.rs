//! Moving a formed seat's guardian to a newer `fedimintd` line, owned by the
//! seat loop (ARCH-manifold-guardian-seat-processes *`fedimintd` lines*).
//!
//! The setup-payment policy names the federations whose guardians run 0.13
//! (SPEC-setup-payment-federations). On each healthy watchdog probe, a formed
//! seat of a listed federation that still runs 0.12 asks its fedimintd to
//! stop after its current session. When the child exits with
//! [`SESSION_BOUNDARY_EXIT_CODE`], its last session completed and no later one
//! began, so the seat records the new line and respawns at once on the new
//! binary, which opens the guardian database at that session boundary.
//!
//! A guardian transfer source asks for the same exit. The seat loop tells the
//! two apart by what it asked: once it asked, or may have asked, for a line
//! move, the next session-boundary exit moves the line and never retires the
//! seat; a transfer asks again after the respawn.

use std::collections::BTreeSet;
use std::sync::Arc;

use fedi_decentralized_domain::FederationId;
use tokio::sync::watch;

use super::{ProcessSlot, SeatLoop};
use crate::bundled_fedimintd::SESSION_BOUNDARY_EXIT_CODE;
use crate::facts::FedimintdLine;
use crate::fedimint_api::FedimintApiError;

/// Bound on asking the local fedimintd; the next healthy probe asks again.
const SHUTDOWN_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The federations whose guardians run the 0.13 line, from the retained
/// setup-payment policy.
pub(crate) type Fedimintd013Federations = watch::Receiver<Arc<BTreeSet<FederationId>>>;

impl SeatLoop {
    /// The line this seat should move to now, if any.
    fn fedimintd_line_target(&self) -> Option<FedimintdLine> {
        let state = self.state.borrow();
        if state.fedimintd_line != FedimintdLine::V0_12 || state.transfer_status().is_some() {
            return None;
        }
        let federation_id = state
            .formed_invite
            .as_ref()?
            .0
            .parse::<fedimint_core::invite_code::InviteCode>()
            .ok()?
            .federation_id()
            .to_string();
        self.fedimintd_013_federations
            .borrow()
            .contains(&FederationId(federation_id))
            .then_some(FedimintdLine::V0_13)
    }

    /// Ask a healthy configured child to stop after its current session when
    /// its federation moved to a newer line.
    pub(super) async fn request_fedimintd_line_move(&mut self) {
        if self.fedimintd_line_move_requested
            || !matches!(self.child, ProcessSlot::Configured { .. })
        {
            return;
        }
        let Some(to) = self.fedimintd_line_target() else {
            return;
        };
        if !self.process_spawner.hosts(to) {
            tracing::warn!(
                safe_to_share = true,
                seat_id = %self.facts.seat_id,
                to = to.as_str(),
                "seat cannot move to a fedimintd line this install lacks"
            );
            return;
        }
        // Set before asking: a request that timed out may still take effect,
        // and its exit must not read as a transfer's.
        self.fedimintd_line_move_requested = true;
        let failure_kind = match tokio::time::timeout(
            SHUTDOWN_REQUEST_TIMEOUT,
            self.client().shutdown_after_current_session(),
        )
        .await
        {
            Ok(Ok(())) => {
                tracing::info!(
                    safe_to_share = true,
                    seat_id = %self.facts.seat_id,
                    to = to.as_str(),
                    "seat fedimintd stops after its current session to change line"
                );
                return;
            }
            Ok(Err(FedimintApiError::Unreachable(_))) => "unreachable",
            Ok(Err(FedimintApiError::Rejected { .. })) => "rejected",
            Ok(Err(FedimintApiError::InvalidResponse { .. })) => "invalid_response",
            Err(_) => "timeout",
        };
        tracing::warn!(
            safe_to_share = true,
            seat_id = %self.facts.seat_id,
            failure_kind,
            "seat could not ask fedimintd to stop after its current session to change line"
        );
    }

    /// Called for every observed child exit, before the transfer source's
    /// [`Self::commit_session_boundary_exit`]. Returns whether this exit
    /// belonged to a line move, in which case the respawn is already due.
    pub(super) async fn commit_fedimintd_line_move(&mut self, exit_code: Option<i32>) -> bool {
        if !self.fedimintd_line_move_requested || exit_code != Some(SESSION_BOUNDARY_EXIT_CODE) {
            return false;
        }
        // The policy may have dropped the federation since the request; the
        // guardian then restarts on its own line.
        if let Some(to) = self.fedimintd_line_target() {
            let from = self.state.borrow().fedimintd_line;
            match self
                .db
                .move_fedimintd_line(&self.facts.seat_id, from, to)
                .await
            {
                Ok(true) => {
                    self.state.send_modify(|state| state.fedimintd_line = to);
                    self.backup.mark();
                    tracing::info!(
                        safe_to_share = true,
                        seat_id = %self.facts.seat_id,
                        from = from.as_str(),
                        to = to.as_str(),
                        "seat moved to a newer fedimintd line between sessions"
                    );
                }
                Ok(false) => {}
                // Restarting on the old line continues its own database,
                // which is safe; the next healthy probe asks again.
                Err(error) => {
                    tracing::warn!(seat_id = %self.facts.seat_id, %error, "failed to record fedimintd line move");
                }
            }
        }
        self.respawn_at = Some(tokio::time::Instant::now());
        true
    }
}
