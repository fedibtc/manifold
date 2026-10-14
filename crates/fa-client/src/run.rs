//! The single driver loop that moves the FA between its states.

use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_service_manifold_guardian::FaId;
use fedimint_core::runtime::sleep;
use futures::future::{Either, select};
use tokio::sync::{MutexGuard, Notify, futures::Notified, watch};

use crate::db::FormationRecovery;
use crate::formation::{DriverRun, finish_driver_run, start_driver_run};
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FaState, FederationConsensusReader,
    FederationSnapshot, FormationFreshness, FormationRunOptions, ManifoldGuardianConnector,
    db::PurchaseRecovery, state::FaStatus,
};

/// Longest wait before the loop retries failed work.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5 * 60);

/// The FA state, as internal parts and as the single state consumers see.
pub(crate) struct Progress {
    parts: watch::Sender<FaStatus>,
    state: watch::Sender<FaState>,
}

impl Progress {
    pub(crate) fn new(status: FaStatus) -> Self {
        let state = status.state();
        Self {
            parts: watch::channel(status).0,
            state: watch::channel(state).0,
        }
    }

    pub(crate) fn modify(&self, update: impl FnOnce(&mut FaStatus)) {
        self.parts.send_modify(update);
        let state = self.parts.borrow().state();
        self.state.send_if_modified(|current| {
            let changed = *current != state;
            *current = state;
            changed
        });
    }

    pub(crate) fn replace(&self, status: FaStatus) {
        self.modify(|current| *current = status);
    }

    pub(crate) fn status(&self) -> FaStatus {
        self.parts.borrow().clone()
    }

    pub(crate) fn state(&self) -> FaState {
        self.state.borrow().clone()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<FaState> {
        self.state.subscribe()
    }
}

/// How user actions and the run loop share the run guard.
#[derive(Default)]
pub(crate) struct RunControl {
    /// Wakes the loop after a user action, or asks it to yield the guard.
    wake: Notify,
    /// User actions that wait for the guard the loop holds.
    waiting: AtomicUsize,
    /// Whether the loop holds, or waits for, the run guard.
    driving: AtomicBool,
}

/// The run guard held by one user action. Releasing it wakes the loop.
pub(crate) struct ActionGuard<'a> {
    _guard: MutexGuard<'a, ()>,
    control: &'a RunControl,
}

impl Drop for ActionGuard<'_> {
    fn drop(&mut self) {
        self.control.wake.notify_one();
    }
}

/// The next work the loop does under the run guard.
enum Work {
    Purchase(PurchaseRecovery),
    Formation(FormationRecovery),
    Reconcile(FederationSnapshot),
}

/// How one loop step ended.
enum Step {
    /// Nothing to do until the state changes.
    Idle,
    /// Work finished or moved the state; look for more at once.
    Progressed,
    /// A user action asked for the run guard.
    Yielded,
    /// Work failed; retry after a delay.
    Failed,
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Drive the FA for as long as the returned future runs.
    ///
    /// The consumer spawns this once after [`Self::open`]. It buys the seats
    /// of an approved preview, forms the federation once every seat is
    /// bought, and verifies a stored federation against consensus. User
    /// actions such as [`Self::create_federation`] save their input and wake
    /// the loop. Failed work is retried with a growing delay, and its error
    /// appears in [`Self::state`]. Dropping the future stops the work; every
    /// durable checkpoint continues on the next run.
    pub async fn run(&self, options: FormationRunOptions) {
        let control = &self.inner.control;
        let mut parts = self.inner.progress.parts.subscribe();
        let mut retry = options.poll_interval();
        loop {
            let mut wake = pin!(control.wake.notified());
            wake.as_mut().enable();
            parts.mark_unchanged();
            // The backup check holds the run guard while it runs, and user
            // actions must not queue behind it.
            if control.waiting.load(Ordering::SeqCst) > 0
                || self.inner.progress.status().recovery.is_some()
            {
                select(wake, pin!(parts.changed())).await;
                continue;
            }
            control.driving.store(true, Ordering::SeqCst);
            let step = {
                let _guard = self.inner.run_guard.lock().await;
                self.step(options, wake.as_mut()).await
            };
            control.driving.store(false, Ordering::SeqCst);
            match step {
                Step::Idle => {
                    let changed = pin!(parts.changed());
                    select(wake, changed).await;
                }
                Step::Progressed => retry = options.poll_interval(),
                Step::Yielded => {}
                Step::Failed => {
                    select(wake, pin!(sleep(retry))).await;
                    retry = retry.saturating_mul(2).min(MAX_RETRY_DELAY);
                }
            }
        }
    }

    /// Take the run guard for one user action.
    ///
    /// The run loop yields the guard at once. Any other holder, such as
    /// another user action or the backup check, makes this
    /// [`FaError::Busy`].
    pub(crate) async fn claim_run_guard(&self) -> FaResult<ActionGuard<'_>> {
        let control = &self.inner.control;
        let guard = match self.inner.run_guard.try_lock() {
            Ok(guard) => guard,
            Err(_) if control.driving.load(Ordering::SeqCst) => {
                control.waiting.fetch_add(1, Ordering::SeqCst);
                control.wake.notify_one();
                let guard = self.inner.run_guard.lock().await;
                control.waiting.fetch_sub(1, Ordering::SeqCst);
                guard
            }
            Err(_) => return Err(FaError::Busy),
        };
        Ok(ActionGuard {
            _guard: guard,
            control,
        })
    }

    async fn step(&self, options: FormationRunOptions, interrupt: Pin<&mut Notified<'_>>) -> Step {
        let result = async {
            let fa_id = self.fa_id()?;
            let Some(work) = self.next_work(fa_id).await? else {
                return Ok(Step::Idle);
            };
            options.validate_for_start(&self.inner.store)?;
            let (deadline, lease) = start_driver_run(&self.inner.store, options).await?;
            let run = DriverRun::new(options, deadline, &lease);
            let result = match select(pin!(self.do_work(work, fa_id, run)), interrupt).await {
                Either::Left((result, _)) => result.map(|()| Step::Progressed),
                Either::Right(((), _)) => Ok(Step::Yielded),
            };
            finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
        }
        .await;
        result.unwrap_or_else(|error| {
            tracing::warn!(error_code = ?error.code(), %error, "FA run step failed");
            Step::Failed
        })
    }

    async fn next_work(&self, fa_id: FaId) -> FaResult<Option<Work>> {
        let status = self.inner.progress.status();
        if status.recovery.is_some() {
            return Ok(None);
        }
        if let Some(recovery) = self.inner.store.load_purchase(fa_id).await? {
            return Ok(Some(Work::Purchase(recovery)));
        }
        if let Some(recovery) = self.inner.store.load_formation(fa_id).await? {
            return Ok(Some(Work::Formation(recovery)));
        }
        if status.creation.is_some() {
            return Ok(None);
        }
        Ok(status
            .federation
            .filter(|federation| federation.freshness == FormationFreshness::Unsynced)
            .map(Work::Reconcile))
    }

    async fn do_work(&self, work: Work, fa_id: FaId, run: DriverRun<'_>) -> FaResult<()> {
        match work {
            Work::Purchase(recovery) => match self.drive_purchase(recovery, fa_id, run).await {
                // The purchase ended before it bought its seats, and the
                // creation needs a new preview.
                Err(FaError::SelectionReauthorizationRequired { reason, .. }) => {
                    self.inner
                        .progress
                        .modify(|status| status.reauthorization = Some(reason));
                    Ok(())
                }
                result => result,
            },
            Work::Formation(recovery) => self.drive_formation(recovery, fa_id, run).await,
            Work::Reconcile(snapshot) => {
                self.begin_recheck();
                let result = self.reconcile_restored(snapshot, fa_id, run).await;
                if let Err(error) = &result {
                    self.fail_recheck(error);
                }
                result
            }
        }
    }
}
