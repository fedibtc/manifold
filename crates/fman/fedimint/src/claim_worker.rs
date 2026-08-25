//! Reconcile accepted ecash evidence in the shared FMan SQLite database with
//! the Fedimint clients' durable operation logs.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use fman_core::db::Db;
use fman_core::wallet::{ClaimOutcome, EcashClaimWorker};
use futures::{StreamExt as _, stream};
use rand::Rng as _;
use tokio::sync::{Notify, watch};

use crate::Wallet;

const SCAN_INTERVAL: Duration = Duration::from_secs(15 * 60);
const INITIAL_BACKOFF: Duration = Duration::from_secs(15);
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_CONCURRENT_CLAIMS: usize = 8;

pub(crate) struct ClaimWorker {
    wake: Arc<Notify>,
    shutdown: watch::Sender<bool>,
    task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ClaimWorker {
    pub(crate) fn start(wallet: Arc<Wallet>, db: Db) -> Arc<Self> {
        let wake = Arc::new(Notify::new());
        let (shutdown, receiver) = watch::channel(false);
        let task = tokio::spawn(run(wallet, db, wake.clone(), receiver));
        Arc::new(Self {
            wake,
            shutdown,
            task: StdMutex::new(Some(task)),
        })
    }
}

#[async_trait::async_trait]
impl EcashClaimWorker for ClaimWorker {
    fn mark(&self) {
        self.wake.notify_one();
    }

    async fn shutdown(&self) {
        self.shutdown.send_replace(true);
        let task = self
            .task
            .lock()
            .expect("claim worker task lock is never poisoned")
            .take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

impl Drop for ClaimWorker {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
        if let Some(task) = self
            .task
            .get_mut()
            .expect("claim worker task lock is never poisoned")
            .take()
        {
            task.abort();
        }
    }
}

async fn run(wallet: Arc<Wallet>, db: Db, wake: Arc<Notify>, mut shutdown: watch::Receiver<bool>) {
    let mut backoff = INITIAL_BACKOFF;
    loop {
        let scan = scan(&wallet, &db);
        let pending = tokio::select! {
            result = scan => match result {
                Ok(pending) => pending,
                Err(_) => {
                    tracing::warn!(
                        safe_to_share = true,
                        stage = "payment_claim_scan",
                        failure_kind = "storage_failed",
                        "ecash claim scan failed"
                    );
                    1
                }
            },
            changed = shutdown.changed() => {
                let _ = changed;
                return;
            }
        };
        let delay = if pending == 0 {
            backoff = INITIAL_BACKOFF;
            SCAN_INTERVAL
        } else {
            let delay = jitter(backoff);
            backoff = (backoff * 2).min(SCAN_INTERVAL);
            delay
        };
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = wake.notified() => {}
            changed = shutdown.changed() => {
                let _ = changed;
                return;
            }
        }
    }
}

fn jitter(delay: Duration) -> Duration {
    delay.mul_f64(rand::thread_rng().gen_range(0.8..=1.2))
}

async fn scan(wallet: &Arc<Wallet>, db: &Db) -> anyhow::Result<u32> {
    let rows = db.pending_ecash_claims().await?;
    let results = stream::iter(rows)
        .map(|record| {
            let wallet = wallet.clone();
            let db = db.clone();
            async move {
                let seat_id = record.seat_id;
                let result = async {
                    let outcome = reconcile_after_preparation(
                        wallet.prepare_claim(&record.evidence),
                        wallet.reconcile_prepared_claim(&record.evidence),
                        ATTEMPT_TIMEOUT,
                    )
                    .await?;
                    db.record_claim_outcome(&seat_id, outcome).await?;
                    if outcome == ClaimOutcome::AlreadySpent {
                        tracing::info!(safe_to_share = true, seat_id = %seat_id, "claim inputs already spent; nothing left to claim (expected after a restore)");
                    } else {
                        tracing::info!(safe_to_share = true, seat_id = %seat_id, "claimed accepted locked payment");
                    }
                    Ok::<_, anyhow::Error>(())
                }.await;
                (seat_id, result)
            }
        })
        .buffer_unordered(MAX_CONCURRENT_CLAIMS)
        .collect::<Vec<_>>()
        .await;
    let mut failures = 0;
    for (seat_id, result) in results {
        if result.is_err() {
            failures += 1;
            tracing::warn!(
                safe_to_share = true,
                seat_id = %seat_id,
                stage = "payment_claim",
                failure_kind = "claim_failed",
                "ecash claim attempt failed"
            );
        }
    }
    Ok(failures)
}

/// Prepare a claim's wallet scope before applying the bounded handoff and
/// terminal-observation attempt budget.
///
/// A first open may run mnemonic recovery, whose explicit contract has no
/// timeout. Cancelling it after the wallet's process-lifetime open fence is set
/// would turn a slow honest recovery into an otherwise permanent pending claim.
async fn reconcile_after_preparation<Prepare, Reconcile>(
    prepare: Prepare,
    reconcile: Reconcile,
    attempt_timeout: Duration,
) -> anyhow::Result<ClaimOutcome>
where
    Prepare: std::future::Future<Output = anyhow::Result<()>>,
    Reconcile: std::future::Future<Output = anyhow::Result<ClaimOutcome>>,
{
    prepare.await?;
    tokio::time::timeout(attempt_timeout, reconcile)
        .await
        .map_err(|_| anyhow::anyhow!("claim attempt timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scope recovery is deliberately unbounded, while the post-recovery
    /// handoff remains bounded. Advancing past the latter's budget before
    /// recovery finishes must not cancel the preparation future.
    #[tokio::test(start_paused = true)]
    async fn recovery_preparation_is_not_cancelled_by_claim_attempt_timeout() {
        let (prepared, wait_for_prepare) = tokio::sync::oneshot::channel();
        let (resume_prepare, resume) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            reconcile_after_preparation(
                async move {
                    prepared.send(()).expect("test observes preparation");
                    resume
                        .await
                        .map_err(|_| anyhow::anyhow!("test preparation was cancelled"))?;
                    Ok(())
                },
                async { Ok(ClaimOutcome::Success) },
                Duration::from_secs(1),
            )
            .await
        });

        wait_for_prepare
            .await
            .expect("claim preparation starts before its budget");
        tokio::time::advance(Duration::from_secs(2)).await;
        tokio::task::yield_now().await;
        assert!(
            !task.is_finished(),
            "the handoff timeout must not apply while recovery prepares the client"
        );

        resume_prepare
            .send(())
            .expect("preparation task remains live after elapsed budget");
        assert_eq!(
            task.await
                .expect("claim task completes")
                .expect("claim succeeds"),
            ClaimOutcome::Success
        );
    }
}
