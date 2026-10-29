//! Scheduled checks and in-memory approval of the decision shown to the caller.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use fedi_decentralized_nostr_clients::FaNostrClient;
use futures::{FutureExt as _, pin_mut};
use tokio::sync::{Mutex, watch};

use crate::formation::{PostFormedAuthority, invite_federation_id};
use crate::{
    DkgCompletionCallback, FaClient, FaError, FaIdentity, FaPayments, FaResult, FaStatus,
    FederationConsensusReader, FederationId, FormationFreshness, FormationId, GuardianTransfer,
    MaintenanceRunOptions, ManifoldGuardianConnector, ReplacementCandidate, ReplacementCheck,
    TransferDestinationApproval,
};

const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

pub(crate) struct ReplacementChecks {
    // Identity of this client opening, used to bind receipts; not a badge issuer.
    issuer: Arc<()>,
    // Last successful result for the ready federation, with its original timestamp.
    current: watch::Sender<Option<Arc<ReplacementCheck>>>,
    // Serializes scheduled and on-demand checks.
    checking: Mutex<()>,
}

impl Default for ReplacementChecks {
    fn default() -> Self {
        Self {
            issuer: Arc::new(()),
            current: watch::channel(None).0,
            checking: Mutex::new(()),
        }
    }
}

// Ignore progress details that do not change which formed federation is ready.
fn ready_federation(status: &FaStatus) -> Option<(FormationId, FederationId)> {
    let (formation, invite) = match status {
        FaStatus::Formed(state) if state.freshness == FormationFreshness::Fresh => {
            (&state.formation_id, &state.federation_invite)
        }
        _ => return None,
    };
    Some((
        formation.clone(),
        FederationId(invite_federation_id(invite).ok()?.to_string()),
    ))
}

impl ReplacementChecks {
    fn retain_for(&self, context: Option<&(FormationId, FederationId)>) {
        self.current.send_if_modified(|current| {
            if current.as_ref().is_some_and(|check| {
                context.is_none_or(|(formation, federation)| {
                    check.formation_id != *formation || check.federation_id != *federation
                })
            }) {
                *current = None;
                true
            } else {
                false
            }
        });
    }
}

/// Approval issued with an eligible guardian's displayed result.
///
/// There is no public constructor or deserializer. Clones retain the same
/// decision through confirmation; changing a displayed candidate cannot change
/// the private approved seat. Reopening the FA invalidates every old receipt.
#[derive(Clone, Debug)]
pub struct ReplacementReceipt {
    issuer: Arc<()>,
    formation_id: FormationId,
    federation_id: FederationId,
    candidate: ReplacementCandidate,
}

/// Why a guardian replacement did not start.
#[derive(Debug, thiserror::Error)]
pub enum ReplacementStartError {
    /// The receipt is from another client or opening, or the guardian it
    /// names no longer holds that seat. Check the candidates again.
    #[error("replacement receipt does not belong to this client's current guardian seat")]
    InvalidReceipt,
    /// The receipt was valid, but the guardian transfer did not start.
    #[error(transparent)]
    Transfer(#[from] FaError),
}

impl ReplacementCheck {
    /// Keep this receipt when the user selects the guardian. A subsequent
    /// background refresh cannot reverse that already offered decision.
    pub fn receipt(&self, seat_index: u16) -> Option<&ReplacementReceipt> {
        self.receipts.get(&seat_index)
    }
}

impl ReplacementReceipt {
    fn matches(&self, issuer: &Arc<()>, authority: &PostFormedAuthority) -> bool {
        Arc::ptr_eq(&self.issuer, issuer)
            && self.formation_id == authority.formation_id
            && invite_federation_id(&authority.invite_code)
                .is_ok_and(|id| id.to_string() == self.federation_id.0)
            && authority.seats.iter().any(|seat| {
                seat.index == self.candidate.seat_index
                    && seat.mg_id == self.candidate.mg_id
                    && seat.seat_id == self.candidate.seat_id
            })
    }
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Check on demand and atomically replace the displayed list and its receipts.
    /// A failed refresh keeps the last successful result and its timestamp while
    /// the same federation remains ready. Held receipts keep their original decision.
    pub async fn check_replacement_candidates(&self) -> FaResult<ReplacementCheck> {
        let state = &self.inner.replacement_checks;
        let _guard = state.checking.lock().await;
        state.retain_for(ready_federation(&self.status()).as_ref());
        match self.collect_replacement_candidates().await {
            Ok(mut check) => {
                check.receipts = check
                    .candidates
                    .iter()
                    .map(|candidate| {
                        (
                            candidate.seat_index,
                            ReplacementReceipt {
                                issuer: state.issuer.clone(),
                                formation_id: check.formation_id.clone(),
                                federation_id: check.federation_id.clone(),
                                candidate: candidate.clone(),
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>();
                state.current.send_replace(Some(Arc::new(check.clone())));
                Ok(check)
            }
            Err(error) => {
                if matches!(
                    error,
                    FaError::NoActiveFormation | FaError::MaintenanceWrongState { .. }
                ) {
                    state.retain_for(None);
                } else {
                    state.retain_for(ready_federation(&self.status()).as_ref());
                }
                Err(error)
            }
        }
    }

    /// Observe the last successful check. None means no result for a ready
    /// federation. Refresh failures do not advance checked_at. Receipts are not persisted.
    pub fn observe_replacement_candidates(&self) -> watch::Receiver<Option<Arc<ReplacementCheck>>> {
        self.inner.replacement_checks.current.subscribe()
    }

    /// Drive checks when a formed federation is ready, then hourly. Each start or
    /// resume checks immediately if ready. Consumers drive this future while active;
    /// dropping it stops the loop. On-demand calls use the same serialized check.
    pub async fn run_replacement_checks(&self) {
        let mut progress = self.observe();
        let mut context = ready_federation(&progress.borrow_and_update());
        loop {
            self.inner.replacement_checks.retain_for(context.as_ref());
            let busy = context.is_some()
                && matches!(
                    self.check_replacement_candidates().await,
                    Err(FaError::Busy)
                );
            // Busy can come from another client holding the database lease too.
            // Retry briefly without spinning or postponing the check for an hour.
            let delay = fedimint_core::runtime::sleep(if busy {
                Duration::from_secs(1)
            } else {
                CHECK_INTERVAL
            })
            .fuse();
            pin_mut!(delay);
            loop {
                {
                    let changed = progress.changed().fuse();
                    pin_mut!(changed);
                    futures::select! {
                        _ = delay => break,
                        result = changed => if result.is_err() { return; },
                    }
                }
                let next = ready_federation(&progress.borrow_and_update());
                if next != context {
                    context = next;
                    break;
                }
                // Repeated status notifications must not restart the hourly timer.
            }
        }
    }

    /// Replace the guardian a receipt names: validate the receipt without
    /// asking about health again, then start a guardian transfer of that
    /// guardian onto the approved destination, as
    /// [`Self::start_guardian_transfer`] does.
    ///
    /// The receipt only authorizes the start. Once the transfer started, its
    /// durable row authorizes [`Self::run_guardian_transfer`] to finish it; reopening
    /// the client invalidates unused receipts, not transfers already started.
    pub async fn request_guardian_replacement(
        &self,
        receipt: &ReplacementReceipt,
        approval: TransferDestinationApproval,
        payment_federation_id: Option<FederationId>,
        completion_callback: Option<DkgCompletionCallback>,
        options: MaintenanceRunOptions,
    ) -> Result<GuardianTransfer, ReplacementStartError> {
        let authority = self
            .post_formed_authority(
                self.fa_id()
                    .map_err(|_| ReplacementStartError::InvalidReceipt)?,
            )
            .await
            .map_err(|_| ReplacementStartError::InvalidReceipt)?;
        if !receipt.matches(&self.inner.replacement_checks.issuer, &authority) {
            return Err(ReplacementStartError::InvalidReceipt);
        }
        Ok(self
            .start_guardian_transfer(
                receipt.candidate.seat_index,
                approval,
                payment_federation_id,
                completion_callback,
                options,
            )
            .await?)
    }
}
