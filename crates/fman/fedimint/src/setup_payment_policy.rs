//! Join maintenance for the authenticated common setup-payment set.
//!
//! The caller supplies policy as a watch of the currently admitted set
//! (SPEC-setup-payment-federations); this task is the mechanism keeping the
//! wallet joined to every member. It only ever joins: a federation removed
//! from the set keeps its wallet state and balance (sweepable by the
//! operator), and quote acceptance — not this task — is what stops selling
//! against it.
//!
//! Each federation is attempted once per daemon process. A failed or cancelled
//! join can leave a dependency-owned database task running, so restart is the
//! retry boundary: it clears stale tasks before the durable prefix is reused.
//! Admitted members join independently: one stalled federation cannot prevent
//! another member or a replacement policy from being processed. The reconciler
//! owns every join task and cancels removed members and all work on shutdown.

use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock, Mutex};

use fedi_decentralized_domain::{
    AdmittedSetupPaymentFederations, FederationId as SetupPaymentFederationId,
    SETUP_PAYMENT_FEDERATIONS_MAX_COUNT,
};
use tokio::sync::{oneshot, watch};
use tokio::task::{AbortHandle, JoinSet};

use fman_core::wallet::EcashWallet;

type AttemptedFederationIds = Arc<Mutex<BTreeSet<SetupPaymentFederationId>>>;

/// Process-lifetime ledger shared by every production reconciler task.
static ATTEMPTED_FEDERATION_IDS: LazyLock<AttemptedFederationIds> =
    LazyLock::new(|| Arc::new(Mutex::new(BTreeSet::new())));

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

/// Spawn the reconciler: on every change of the admitted set, make one
/// process-lifetime join attempt for each member federation the wallet does not
/// hold yet. Exits when the policy sender is dropped.
///
/// Joins run concurrently, bounded by the admitted set's protocol limit.
/// Failed attempts are retried only after a daemon restart. The Fedimint client
/// can leave database-owning tasks behind before a failed join returns, so an
/// in-process retry could create concurrent writers for one wallet partition.
pub fn spawn_setup_payment_join_reconciler(
    wallet: Arc<dyn EcashWallet>,
    policy: watch::Receiver<Option<AdmittedSetupPaymentFederations>>,
) -> SetupPaymentJoinReconciler {
    spawn_setup_payment_join_reconciler_with_attempts(
        wallet,
        policy,
        ATTEMPTED_FEDERATION_IDS.clone(),
    )
}

fn spawn_setup_payment_join_reconciler_with_attempts(
    wallet: Arc<dyn EcashWallet>,
    mut policy: watch::Receiver<Option<AdmittedSetupPaymentFederations>>,
    attempted_federation_ids: AttemptedFederationIds,
) -> SetupPaymentJoinReconciler {
    let (shutdown, mut shutdown_requested) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut joins = JoinSet::new();
        let mut active = std::collections::BTreeMap::<SetupPaymentFederationId, AbortHandle>::new();
        'reconcile: loop {
            while let Some(completed) = joins.try_join_next() {
                if let Ok(federation_id) = completed {
                    active.remove(&federation_id);
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
                    if joined.contains(&federation_id.0) || active.contains_key(federation_id) {
                        continue;
                    }
                    // Register before awaiting join. Task cancellation or
                    // replacement must not permit another same-process attempt.
                    if !attempted_federation_ids
                        .lock()
                        .expect("setup-payment attempt ledger is not poisoned")
                        .insert(federation_id.clone())
                    {
                        continue;
                    }
                    let wallet = wallet.clone();
                    let federation_id = federation_id.clone();
                    let active_federation_id = federation_id.clone();
                    let invite_code = invite_code.0.clone();
                    let join = joins.spawn(async move {
                        match wallet.join(&invite_code).await {
                            Ok(_) => {
                                tracing::info!(
                                    federation_id = %federation_id.0,
                                    "joined setup-payment federation"
                                );
                            }
                            Err(_) => {
                                tracing::warn!(
                                    safe_to_share = true,
                                    federation_id = %federation_id.0,
                                    failure_kind = "join_failed",
                                    "failed to join setup-payment federation; restart to retry"
                                );
                            }
                        }
                        federation_id
                    });
                    active.insert(active_federation_id, join);
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
                    if let Some(Ok(federation_id)) = completed {
                        active.remove(&federation_id);
                    }
                }
            }
        }
        joins.abort_all();
        while joins.join_next().await.is_some() {}
    });
    SetupPaymentJoinReconciler { shutdown, task }
}

#[cfg(test)]
#[path = "setup_payment_policy/tests.rs"]
mod tests;
