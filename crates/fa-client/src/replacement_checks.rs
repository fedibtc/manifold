//! Scheduled checks and in-memory approval of the decision shown to the caller.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use fedi_decentralized_nostr_clients::FaNostrClient;
use futures::{FutureExt as _, pin_mut};
use tokio::sync::{Mutex, watch};

use crate::formation::{PostFormedAuthority, invite_federation_id};
use crate::{
    FaClient, FaIdentity, FaPayments, FaResult, FederationConsensusReader, FederationId,
    FormationId, ManifoldGuardianConnector, ReplacementCandidate, ReplacementCheck,
};

const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

pub(crate) struct ReplacementChecks {
    issuer: Arc<()>,
    current: watch::Sender<Option<Arc<ReplacementCheck>>>,
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

/// The replacement entry point is intentionally a stub until transfer integration.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ReplacementStartError {
    #[error("replacement receipt does not belong to this client's current guardian seat")]
    InvalidReceipt,
    #[error("guardian replacement is not implemented yet")]
    NotImplemented,
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
    /// An unavailable check clears the list; receipts already held for confirmation
    /// remain valid for their original seat on this same client instance.
    pub async fn check_replacement_candidates(&self) -> FaResult<ReplacementCheck> {
        let state = &self.inner.replacement_checks;
        let _guard = state.checking.lock().await;
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
                state.current.send_replace(None);
                Err(error)
            }
        }
    }

    /// Observe eligibility snapshots. None means not checked or unavailable,
    /// not that the federation is healthy. Receipts are never persisted.
    pub fn observe_replacement_candidates(&self) -> watch::Receiver<Option<Arc<ReplacementCheck>>> {
        self.inner.replacement_checks.current.subscribe()
    }

    /// Drive checks while the consumer is active: immediately on each start or
    /// resume, then hourly, and when formation state changes. Dropping this future
    /// stops the loop. Consumers own its lifetime, not the timer or decision rules.
    /// A separately requested on-demand check uses the same serialized path.
    pub async fn run_replacement_checks(&self) {
        let mut progress = self.observe();
        loop {
            let _ = self.check_replacement_candidates().await;
            let delay = fedimint_core::runtime::sleep(CHECK_INTERVAL).fuse();
            let changed = progress.changed().fuse();
            pin_mut!(delay, changed);
            futures::select! {
                _ = delay => {},
                result = changed => if result.is_err() { return; },
            }
        }
    }

    /// Validate the eligibility receipt without asking about health again.
    ///
    /// This stub performs no purchase, authorization, or transfer. A valid receipt
    /// reaches NotImplemented; PR #184 can replace that final step with its driver.
    pub async fn request_guardian_replacement(
        &self,
        receipt: &ReplacementReceipt,
    ) -> Result<(), ReplacementStartError> {
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
        Err(ReplacementStartError::NotImplemented)
    }
}
