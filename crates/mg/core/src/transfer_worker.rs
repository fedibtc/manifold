//! Event-driven, fleet-owned transfer jobs. Cancellation is cooperative: native
//! blocking work and enqueued seat commands must finish before a job is joined.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use fedi_decentralized_service_manifold_guardian::{
    MgSignedRequest, SeatId, Timestamp, TransferSeatRequest,
};
use tokio::sync::{Notify, Semaphore, watch};
use tokio::task::{JoinHandle, JoinSet};

use crate::db::Db;
use crate::fleet::FleetConfig;
use crate::identity::RootMnemonic;
use crate::seat::Seat;
use crate::transfer_native::TransferMaterial;
use crate::transfer_state::{IncomingState, TransferPhase};

#[derive(Clone, Default)]
pub(crate) struct TransferWake(Arc<WakeState>);

#[derive(Default)]
struct WakeState {
    notify: Notify,
    changed: Mutex<HashSet<SeatId>>,
}

impl TransferWake {
    pub(crate) fn mark(&self, seat: &SeatId) {
        self.0
            .changed
            .lock()
            .expect("transfer wake lock poisoned")
            .insert(seat.clone());
        self.0.notify.notify_one();
    }
}

type Seats = Arc<RwLock<HashMap<SeatId, Arc<Seat>>>>;

#[derive(Debug, thiserror::Error)]
#[error("guardian transfer requires local repair")]
struct ParkTransfer;

fn owner_failure(error: crate::seat::SeatVerbError) -> anyhow::Error {
    match error {
        crate::seat::SeatVerbError::Internal(error) => {
            if error.is::<crate::transfer_native::InvalidGuardian>() {
                ParkTransfer.into()
            } else {
                error
            }
        }
        crate::seat::SeatVerbError::TransferGrantMismatch => ParkTransfer.into(),
        error => error.into(),
    }
}

struct Active {
    cancel: watch::Sender<bool>,
    retry: watch::Sender<()>,
}

struct Runtime {
    stop: watch::Sender<bool>,
    join: JoinHandle<()>,
}

pub(crate) struct TransferWorker {
    runtime: Mutex<Option<Runtime>>,
    wake: TransferWake,
}

struct Jobs {
    db: Db,
    config: FleetConfig,
    identity: Arc<RootMnemonic>,
    seats: Seats,
    permits: Semaphore,
    advertisement: Arc<Notify>,
}

impl TransferWorker {
    pub(crate) fn new(
        db: Db,
        config: FleetConfig,
        identity: Arc<RootMnemonic>,
        seats: Seats,
        advertisement: Arc<Notify>,
        wake: TransferWake,
    ) -> Self {
        // Stored transfers always resume, whatever the setup-payment policy
        // says now: the policy gates only the start of a new transfer, so a
        // guardian is never left half moved.
        let jobs = Arc::new(Jobs {
            db,
            config,
            identity,
            seats,
            permits: Semaphore::new(4),
            advertisement,
        });
        let (stop, stop_rx) = watch::channel(false);
        let runtime = Some(Runtime {
            stop,
            join: tokio::spawn(supervise(jobs, wake.clone(), stop_rx)),
        });
        Self {
            runtime: Mutex::new(runtime),
            wake,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_running_for_test(&self) -> bool {
        self.runtime
            .lock()
            .expect("transfer runtime lock poisoned")
            .is_some()
    }

    pub(crate) fn wake_handle(&self) -> TransferWake {
        self.wake.clone()
    }

    pub(crate) async fn shutdown(&self) {
        let runtime = self
            .runtime
            .lock()
            .expect("transfer runtime lock poisoned")
            .take();
        if let Some(runtime) = runtime {
            let _ = runtime.stop.send(true);
            let _ = runtime.join.await;
        }
    }
}

impl Drop for TransferWorker {
    fn drop(&mut self) {
        if let Some(runtime) = self
            .runtime
            .get_mut()
            .expect("transfer runtime lock poisoned")
            .take()
        {
            // Detach cooperative draining, not the jobs themselves. The
            // supervisor retains the database/root lock until every job joins.
            let _ = runtime.stop.send(true);
        }
    }
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    if *cancel.borrow() {
        return;
    }
    let _ = cancel.wait_for(|cancelled| *cancelled).await;
}

async fn drain_seat(
    seat: &SeatId,
    active: &mut HashMap<SeatId, Active>,
    tasks: &mut JoinSet<SeatId>,
) {
    if let Some(job) = active.get(seat) {
        let _ = job.cancel.send(true);
    }
    while active.contains_key(seat) {
        match tasks.join_next().await {
            Some(Ok(finished)) => {
                active.remove(&finished);
            }
            // Jobs do not panic intentionally; an unexpected panic must not
            // leave a phantom active job blocking shutdown forever.
            Some(Err(_)) => {
                for job in active.values() {
                    let _ = job.cancel.send(true);
                }
                while tasks.join_next().await.is_some() {}
                active.clear();
            }
            None => {
                active.clear();
            }
        }
    }
}

async fn supervise(jobs: Arc<Jobs>, wake: TransferWake, mut stop: watch::Receiver<bool>) {
    let mut active = HashMap::<SeatId, Active>::new();
    let mut tasks = JoinSet::new();
    loop {
        // Startup and transition/completion notifications, never local polling.
        let changed =
            std::mem::take(&mut *wake.0.changed.lock().expect("transfer wake lock poisoned"));
        for id in changed {
            if let Some(job) = active.get(&id) {
                job.retry.send_replace(());
            }
        }
        let seats: Vec<_> = jobs
            .seats
            .read()
            .expect("seat registry lock poisoned")
            .values()
            .cloned()
            .collect();
        let mut failed = false;
        for seat in seats {
            if *stop.borrow() {
                break;
            }
            let id = seat.facts().seat_id.clone();
            let wanted = if seat.is_decommissioned() {
                false
            } else {
                match jobs.pulls(&seat).await {
                    Ok(pulls) => pulls,
                    Err(_) => {
                        failed = true;
                        continue;
                    }
                }
            };
            if active.contains_key(&id) != wanted {
                drain_seat(&id, &mut active, &mut tasks).await;
                if wanted {
                    let (cancel, cancel_rx) = watch::channel(false);
                    let (retry, retry_rx) = watch::channel(());
                    active.insert(id.clone(), Active { cancel, retry });
                    let jobs = jobs.clone();
                    tasks.spawn(async move {
                        jobs.run(seat, cancel_rx, retry_rx).await;
                        id
                    });
                }
            }
        }
        tokio::select! {
            biased;
            _ = cancelled(&mut stop) => break,
            _ = wake.0.notify.notified() => {}
            result = tasks.join_next(), if !tasks.is_empty() => {
                match result {
                    Some(Ok(id)) => { active.remove(&id); }
                    Some(Err(_)) => {
                        for job in active.values() { let _ = job.cancel.send(true); }
                        while tasks.join_next().await.is_some() {}
                        active.clear();
                    }
                    None => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(1)), if failed => {}
        }
    }
    for job in active.values() {
        let _ = job.cancel.send(true);
    }
    while tasks.join_next().await.is_some() {}
}

impl Jobs {
    /// Whether the seat waits for its source to hand over a guardian.
    async fn pulls(&self, seat: &Seat) -> anyhow::Result<bool> {
        let id = &seat.facts().seat_id;
        if self.db.outgoing_transfer(id).await?.is_some() {
            return Ok(false);
        }
        Ok(self
            .db
            .incoming_transfer(id)
            .await?
            .is_some_and(|incoming| incoming.phase() == TransferPhase::Waiting))
    }

    async fn run(
        self: &Arc<Self>,
        seat: Arc<Seat>,
        mut cancel: watch::Receiver<bool>,
        mut retry: watch::Receiver<()>,
    ) {
        let mut backoff = Duration::from_millis(500);
        loop {
            if *cancel.borrow() || seat.is_decommissioned() {
                return;
            }
            match self.pulls(&seat).await {
                Ok(true) | Err(_) => {}
                Ok(false) => return,
            }
            retry.borrow_and_update();
            let error = match self.attempt(&seat, &mut cancel).await {
                Ok(()) => return,
                Err(error) => error,
            };
            if *cancel.borrow() {
                return;
            }
            if seat.transfer_exit_unproven()
                || error.chain().any(|source| {
                    source.is::<ParkTransfer>()
                        || source.is::<crate::transfer_native::InvalidGuardian>()
                })
            {
                // Keep this job registered, with no retry timer. Another
                // seat's transitions and JoinSet completions cannot restart
                // immutable local failures. Same-seat replay, operator action
                // or cancellation can wake it.
                tokio::select! {
                    biased;
                    _ = cancelled(&mut cancel) => return,
                    _ = retry.changed() => {}
                }
                continue;
            }
            // Remote/process/storage failures get bounded retries; jitter
            // avoids synchronizing source and relay requests after restart.
            let delay = backoff + Duration::from_millis(rand::random::<u64>() % 250);
            tokio::select! {
                _ = cancelled(&mut cancel) => return,
                _ = tokio::time::sleep(delay) => {}
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }

    async fn attempt(&self, seat: &Seat, cancel: &mut watch::Receiver<bool>) -> anyhow::Result<()> {
        let record = self
            .db
            .incoming_transfer(&seat.facts().seat_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("transfer disappeared"))?;
        let IncomingState::Waiting { grant } = record.state else {
            return Ok(());
        };
        let permit = tokio::select! {
            _ = cancelled(cancel) => anyhow::bail!("transfer cancelled"),
            permit = self.permits.acquire() => permit?,
        };
        let now = Timestamp((crate::db::now_ms() / 1000) as u64);
        let request = MgSignedRequest::create(
            &TransferSeatRequest {
                ts: now,
                destination_service_pubkey: record.binding.destination_service_pubkey,
                grant,
            },
            &self.identity.derive_service_signing_key(),
        )?;
        let response = tokio::select! {
            _ = cancelled(cancel) => anyhow::bail!("transfer cancelled"),
            response = tokio::time::timeout(Duration::from_secs(30), self.config.transfer_source.transfer(&record.binding.source, request)) => response???,
        };
        let material = TransferMaterial::from_wire(response.material)?;
        // Do not select on cancellation here: dropping this future
        // would detach spawn_blocking and violate cleanup's join fence.
        let guardian = crate::transfer_native::validate_material(
            &self.config.process,
            material,
            record.binding.federation_id.clone(),
            self.config.guardian_config_validator.clone(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("source guardian material is invalid"))?;
        drop(permit);
        anyhow::ensure!(!*cancel.borrow(), "transfer cancelled");
        seat.install_transfer(record.binding, guardian)
            .await
            .map_err(owner_failure)?;
        self.advertisement.notify_one();
        Ok(())
    }
}
