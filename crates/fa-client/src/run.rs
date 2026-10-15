//! The single driver loop that moves the FA between its states.

use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_service_manifold_guardian::FaId;
use fedimint_core::runtime::sleep;
use futures::future::{Either, select};
use tokio::sync::{MutexGuard, Notify, futures::Notified, watch};

use crate::db::{FormationView, PurchaseView};
use crate::formation::Timeouts;
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FaState, FederationConsensusReader,
    ManifoldGuardianConnector, RecoveryCheck, state::FaStatus,
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

    /// Change the status. Watchers wake only when it changed, so the run
    /// loop can refresh it from storage without waking itself.
    pub(crate) fn modify(&self, update: impl FnOnce(&mut FaStatus)) {
        self.parts.send_if_modified(|status| {
            let before = status.clone();
            update(status);
            *status != before
        });
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
    pub(crate) driving: AtomicBool,
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

/// A user action that waits for the guard the loop holds. Dropping it, also
/// when the action is cancelled, withdraws the wait.
struct WaitingAction<'a>(&'a RunControl);

impl<'a> WaitingAction<'a> {
    fn new(control: &'a RunControl) -> Self {
        control.waiting.fetch_add(1, Ordering::SeqCst);
        control.wake.notify_one();
        Self(control)
    }
}

impl Drop for WaitingAction<'_> {
    fn drop(&mut self) {
        self.0.waiting.fetch_sub(1, Ordering::SeqCst);
        self.0.wake.notify_one();
    }
}

/// The loop holds, or waits for, the run guard until this drops.
struct Driving<'a>(&'a RunControl);

impl<'a> Driving<'a> {
    fn new(control: &'a RunControl) -> Self {
        control.driving.store(true, Ordering::SeqCst);
        Self(control)
    }
}

impl Drop for Driving<'_> {
    fn drop(&mut self) {
        self.0.driving.store(false, Ordering::SeqCst);
    }
}

/// The next work the loop does under the run guard.
enum Work {
    /// Look up and verify the backup of a restored mnemonic.
    Recover,
    /// Buy the creation's pending slots.
    Purchase(Box<PurchaseView>),
    /// Form the full pool.
    Formation(Box<FormationView>),
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
    /// bought, restores the backup of a restored mnemonic, and verifies a
    /// stored federation against consensus. User
    /// actions such as [`Self::create_federation`] save their input and wake
    /// the loop. Failed work is retried with a growing delay, and its error
    /// appears in [`Self::state`]. Dropping the future stops the work; every
    /// durable checkpoint continues on the next run.
    pub async fn run(&self) {
        self.run_with(Timeouts::default()).await;
    }

    pub(crate) async fn run_with(&self, timeouts: Timeouts) {
        let control = &self.inner.control;
        let mut parts = self.inner.progress.parts.subscribe();
        let mut retry = timeouts.poll;
        loop {
            let mut wake = pin!(control.wake.notified());
            wake.as_mut().enable();
            parts.mark_unchanged();
            if control.waiting.load(Ordering::SeqCst) > 0 {
                select(wake, pin!(parts.changed())).await;
                continue;
            }
            let step = {
                let _driving = Driving::new(control);
                let _guard = self.inner.run_guard.lock().await;
                self.step(timeouts, wake.as_mut()).await
            };
            match step {
                Step::Idle => {
                    let changed = pin!(parts.changed());
                    select(wake, changed).await;
                }
                Step::Progressed => retry = timeouts.poll,
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
    /// The run loop yields the guard at once. Another user action, or a
    /// backup lookup that has not finished, makes this [`FaError::Busy`].
    pub(crate) async fn claim_run_guard(&self) -> FaResult<ActionGuard<'_>> {
        let control = &self.inner.control;
        if self.inner.progress.status().recovery.is_some() {
            return Err(FaError::Busy);
        }
        let guard = match self.inner.run_guard.try_lock() {
            Ok(guard) => guard,
            Err(_) if control.driving.load(Ordering::SeqCst) => {
                let _waiting = WaitingAction::new(control);
                self.inner.run_guard.lock().await
            }
            Err(_) => return Err(FaError::Busy),
        };
        Ok(ActionGuard {
            _guard: guard,
            control,
        })
    }

    async fn step(&self, timeouts: Timeouts, interrupt: Pin<&mut Notified<'_>>) -> Step {
        let result = async {
            let fa_id = self.fa_id()?;
            let Some(work) = self.next_work(fa_id).await? else {
                // Work that ended between its last commit and its state
                // update leaves a stale state, so refresh it from storage.
                self.publish(None).await;
                return Ok(Step::Idle);
            };
            match select(pin!(self.do_work(work, fa_id, timeouts)), interrupt).await {
                Either::Left((result, _)) => result.map(|()| Step::Progressed),
                Either::Right(((), _)) => Ok(Step::Yielded),
            }
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
            return Ok(Some(Work::Recover));
        }
        // A creation that needs seats waits for the user.
        if let Some(purchase) = self.inner.store.load_purchase(fa_id).await? {
            return Ok(Some(Work::Purchase(Box::new(purchase))));
        }
        Ok(self
            .inner
            .store
            .load_formation(fa_id)
            .await?
            .map(|formation| Work::Formation(Box::new(formation))))
    }

    async fn do_work(&self, work: Work, fa_id: FaId, timeouts: Timeouts) -> FaResult<()> {
        match work {
            Work::Recover => {
                let result = self.recover_backup(fa_id, timeouts).await;
                if let Err(error) = &result {
                    let last_error = Some(error.code());
                    self.inner.progress.modify(|status| {
                        status.recovery = Some(RecoveryCheck { last_error });
                    });
                }
                result
            }
            Work::Purchase(purchase) => self.drive_purchase(*purchase, fa_id, timeouts).await,
            Work::Formation(formation) => self.drive_formation(*formation, fa_id, timeouts).await,
        }
    }
}
