//! Join maintenance for the authenticated common setup-payment set.
//!
//! The caller supplies policy as a watch of the currently admitted set
//! (SPEC-setup-payment-federations); this task is the mechanism keeping the
//! wallet joined to every member. It only ever joins: a federation removed
//! from the set keeps its wallet state and balance (sweepable by the
//! operator), and quote acceptance — not this task — is what stops selling
//! against it.
//!
//! A join that fails before the wallet starts opening a client, such as a
//! config download from a federation not reachable yet, is retried with a
//! bounded backoff for as long as the federation stays admitted. Once an open
//! has started, a failed or cancelled join can leave a dependency-owned
//! database task running; the wallet then refuses every further open of that
//! client, and only a restart retries it (see [`JoinError`]).
//! Admitted members join independently: one stalled federation cannot prevent
//! another member or a replacement policy from being processed. The reconciler
//! owns every join task and cancels removed members and all work on shutdown.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use fedi_decentralized_domain::{
    AdmittedSetupPaymentFederations, FederationId as SetupPaymentFederationId,
    SETUP_PAYMENT_FEDERATIONS_MAX_COUNT,
};
use tokio::sync::{oneshot, watch};
use tokio::task::{AbortHandle, JoinSet};

use mg_core::wallet::{EcashWallet, JoinError};

/// Wait before the first retry of a retryable join failure; it doubles per
/// failure up to [`JOIN_RETRY_MAX_DELAY`].
const JOIN_RETRY_INITIAL_DELAY: Duration = Duration::from_secs(5);
/// Longest wait between retries. It bounds how long a federation that became
/// reachable stays unsellable, on top of one join attempt.
const JOIN_RETRY_MAX_DELAY: Duration = Duration::from_secs(60);

/// Owned setup-payment join reconciler with graceful child-task shutdown.
pub struct SetupPaymentJoinReconciler {
    /// Explicit graceful-shutdown request.
    shutdown: oneshot::Sender<()>,
    /// Parent task that owns and joins every federation join task.
    task: tokio::task::JoinHandle<()>,
}

impl SetupPaymentJoinReconciler {
    /// Cancel and join all reconciliation work.
    pub async fn shutdown(self) -> Result<(), tokio::task::JoinError> {
        let Self { shutdown, task } = self;
        let _ = shutdown.send(());
        task.await
    }

    #[cfg(test)]
    async fn join(self) -> Result<(), tokio::task::JoinError> {
        let Self { shutdown, task } = self;
        let result = task.await;
        drop(shutdown);
        result
    }
}

/// Spawn the reconciler: on every change of the admitted set, start a join for
/// each member federation the wallet does not hold yet and is not already
/// joining. Exits when the policy sender is dropped.
///
/// Joins run concurrently, bounded by the admitted set's protocol limit. Each
/// join retries retryable failures until it succeeds, the wallet requires a
/// restart, or the federation leaves the admitted set.
pub fn spawn_setup_payment_join_reconciler(
    wallet: Arc<dyn EcashWallet>,
    mut policy: watch::Receiver<Option<AdmittedSetupPaymentFederations>>,
) -> SetupPaymentJoinReconciler {
    let (shutdown, mut shutdown_requested) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut joins = JoinSet::new();
        let mut active = std::collections::BTreeMap::<SetupPaymentFederationId, AbortHandle>::new();
        // The wallet refuses these until restart; attempting them again
        // would only repeat the refusal.
        let mut awaiting_restart = BTreeSet::<SetupPaymentFederationId>::new();
        'reconcile: loop {
            while let Some(completed) = joins.try_join_next() {
                if let Ok(settled) = completed {
                    settle(&mut active, &mut awaiting_restart, settled);
                }
            }

            let set = policy.borrow_and_update().clone();
            let admitted_ids = set
                .as_ref()
                .map(|set| {
                    set.iter()
                        .map(|(federation_id, _)| federation_id.clone())
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default();

            active.retain(|federation_id, join| {
                if admitted_ids.contains(federation_id) {
                    true
                } else {
                    join.abort();
                    false
                }
            });

            if let Some(set) = set {
                // The policy speaks the domain crate's federation id and the
                // wallet speaks the wire one; both newtype the same canonical
                // string, which is where they can meet.
                let joined: BTreeSet<String> = wallet
                    .joined_federation_ids()
                    .await
                    .into_iter()
                    .map(|federation_id| federation_id.0)
                    .collect();
                for (federation_id, invite_code) in set.iter() {
                    if joins.len() == SETUP_PAYMENT_FEDERATIONS_MAX_COUNT {
                        break;
                    }
                    if joined.contains(&federation_id.0)
                        || active.contains_key(federation_id)
                        || awaiting_restart.contains(federation_id)
                    {
                        continue;
                    }
                    let join = joins.spawn(join_until_settled(
                        wallet.clone(),
                        federation_id.clone(),
                        invite_code.0.clone(),
                    ));
                    active.insert(federation_id.clone(), join);
                }
            }

            tokio::select! {
                _ = &mut shutdown_requested => {
                    break 'reconcile;
                }
                changed = policy.changed() => {
                    if changed.is_err() {
                        break 'reconcile;
                    }
                }
                completed = joins.join_next(), if !joins.is_empty() => {
                    if let Some(Ok(settled)) = completed {
                        settle(&mut active, &mut awaiting_restart, settled);
                    }
                }
            }
        }
        joins.abort_all();
        while joins.join_next().await.is_some() {}
    });
    SetupPaymentJoinReconciler { shutdown, task }
}

/// How a join task ended, other than by cancellation.
enum Settled {
    Joined(SetupPaymentFederationId),
    RestartRequired(SetupPaymentFederationId),
}

/// Release a finished join's slot, remembering a federation the wallet will
/// refuse until restart.
fn settle(
    active: &mut std::collections::BTreeMap<SetupPaymentFederationId, AbortHandle>,
    awaiting_restart: &mut BTreeSet<SetupPaymentFederationId>,
    settled: Settled,
) {
    match settled {
        Settled::Joined(federation_id) => {
            active.remove(&federation_id);
        }
        Settled::RestartRequired(federation_id) => {
            active.remove(&federation_id);
            awaiting_restart.insert(federation_id);
        }
    }
}

/// Join one admitted federation, retrying retryable failures with a bounded
/// backoff, until it is joined or the wallet requires a restart.
async fn join_until_settled(
    wallet: Arc<dyn EcashWallet>,
    federation_id: SetupPaymentFederationId,
    invite_code: String,
) -> Settled {
    let mut retry_delay = JOIN_RETRY_INITIAL_DELAY;
    loop {
        match wallet.join(&invite_code).await {
            Ok(_) => {
                tracing::info!(
                    federation_id = %federation_id.0,
                    "joined setup-payment federation"
                );
                return Settled::Joined(federation_id);
            }
            Err(JoinError::RestartRequired(_)) => {
                tracing::warn!(
                    safe_to_share = true,
                    federation_id = %federation_id.0,
                    failure_kind = "join_open_failed",
                    "failed to join setup-payment federation; restart to retry"
                );
                return Settled::RestartRequired(federation_id);
            }
            Err(JoinError::Retryable(_)) => {
                tracing::warn!(
                    safe_to_share = true,
                    federation_id = %federation_id.0,
                    failure_kind = "join_failed",
                    retry_in_secs = retry_delay.as_secs(),
                    "failed to join setup-payment federation; retrying"
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(JOIN_RETRY_MAX_DELAY);
            }
        }
    }
}

#[cfg(test)]
#[path = "setup_payment_policy/tests.rs"]
mod tests;
