//! Direct authenticated polling of typed FMan safe-event journals.

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    num::{NonZeroU16, NonZeroUsize},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
    time::Duration,
};

use fedi_iroh_rpc::iroh::{Endpoint, EndpointAddr};

use crate::{
    archive::JournalArchive,
    iroh_journal_source::{IrohJournalSource, JournalSource},
    journal_catalog::JournalCatalog,
    journal_collector::SingleBatchCollector,
    journal_target::{CollectionTarget, CommitOutcome},
    journal_types::{ReceptionDay, unix_seconds},
    store::Store,
};

const MAX_JOURNALS_PER_TARGET: usize = 32;
const MAX_SELECTOR_BYTES: usize = 512;
const MAX_BATCHES_PER_TARGET: usize = 40;
const MAX_TARGET_ELAPSED: Duration = Duration::from_secs(30);

/// Sanitized polling failure classification.
#[derive(Clone, Copy, Debug, thiserror::Error)]
pub(crate) enum PollError {
    #[error("safe-journal source temporarily unavailable")]
    Transient,
    #[error("safe-journal archive capacity reached")]
    Capacity,
    #[error("{0}")]
    Fatal(&'static str),
}

pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> Result<i64, PollError>;
    fn target_budget_expired(&self, started: tokio::time::Instant) -> bool {
        started.elapsed() >= MAX_TARGET_ELAPSED
    }
}

struct SystemClock;

/// A per-poll-cycle fence that closes target admission after a fatal target error.
#[derive(Clone, Default)]
struct FatalAdmission {
    closed: Arc<AtomicBool>,
}

impl FatalAdmission {
    /// Close admission before the fatal worker releases its target permit.
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Report whether a target may begin work that can reach its transport.
    fn allows_target_work(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
struct FatalAdmissionHook {
    allow_queued: tokio::sync::Notify,
    queued: tokio::sync::Notify,
    published: tokio::sync::Barrier,
    release: tokio::sync::Barrier,
    queued_target_id: String,
}

/// Polling runtime with one global target-concurrency bound.
#[derive(Clone)]
pub(crate) struct JournalPoller {
    catalog: JournalCatalog,
    source: Arc<dyn JournalSource>,
    concurrency: NonZeroUsize,
    retention_days: NonZeroU16,
    clock: Arc<dyn Clock>,
    last_pruned_day: Arc<std::sync::Mutex<Option<ReceptionDay>>>,
    stream_offsets: Arc<tokio::sync::Mutex<HashMap<String, usize>>>,
    #[cfg(test)]
    fatal_admission_hook: Option<Arc<FatalAdmissionHook>>,
}

impl JournalPoller {
    /// Build the production direct-Iroh journal poller from validated settings.
    pub(crate) fn new(
        store: Store,
        archive: JournalArchive,
        endpoint: Endpoint,
        concurrency: NonZeroUsize,
        retention_days: NonZeroU16,
        initial_pruned_day: ReceptionDay,
        address_override: Option<EndpointAddr>,
    ) -> Self {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        Self {
            catalog: JournalCatalog::new(store, archive, clock.clone()),
            source: Arc::new(IrohJournalSource::with_optional_address(
                endpoint,
                address_override,
            )),
            concurrency,
            retention_days,
            clock,
            last_pruned_day: Arc::new(std::sync::Mutex::new(Some(initial_pruned_day))),
            stream_offsets: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            #[cfg(test)]
            fatal_admission_hook: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_source(
        store: Store,
        archive: JournalArchive,
        source: Arc<dyn JournalSource>,
        concurrency: NonZeroUsize,
        retention_days: NonZeroU16,
    ) -> Self {
        Self::with_source_and_clock(
            store,
            archive,
            source,
            concurrency,
            retention_days,
            Arc::new(SystemClock),
        )
    }

    #[cfg(test)]
    pub(crate) fn with_source_and_clock(
        store: Store,
        archive: JournalArchive,
        source: Arc<dyn JournalSource>,
        concurrency: NonZeroUsize,
        retention_days: NonZeroU16,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            catalog: JournalCatalog::new(store, archive, clock.clone()),
            source,
            concurrency,
            retention_days,
            clock,
            last_pruned_day: Arc::new(std::sync::Mutex::new(None)),
            stream_offsets: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            fatal_admission_hook: None,
        }
    }

    /// Poll forever at a journal-specific cadence until shutdown or a fatal sink error.
    pub(crate) async fn run(
        self,
        cadence: Duration,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), PollError> {
        let mut interval = tokio::time::interval(cadence);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    while self.poll_once(shutdown.clone()).await? {
                        tokio::task::yield_now().await;
                        if *shutdown.borrow() {
                            return Ok(());
                        }
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn poll_once(
        &self,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<bool, PollError> {
        self.prune_and_recover().await?;
        let targets = self
            .catalog
            .active_targets()
            .await
            .map_err(|_| PollError::Fatal("journal target lookup failed"))?;
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.concurrency.get()));
        let mut tasks = tokio::task::JoinSet::new();
        let (stop_sender, stop_receiver) = tokio::sync::watch::channel(false);
        let fatal_admission = FatalAdmission::default();
        for target in targets {
            let poller = self.clone();
            let target_shutdown = shutdown.clone();
            let target_stop = stop_receiver.clone();
            let target_stop_sender = stop_sender.clone();
            let target_semaphore = semaphore.clone();
            let target_admission = fatal_admission.clone();
            tasks.spawn(async move {
                #[cfg(test)]
                let queued = if let Some(hook) = poller
                    .fatal_admission_hook
                    .as_ref()
                    .filter(|hook| target.target_id == hook.queued_target_id)
                {
                    hook.allow_queued.notified().await;
                    Some(&hook.queued)
                } else {
                    None
                };
                let permit = tokio::select! {
                    biased;
                    () = wait_for_shutdown(target_shutdown.clone()) => {
                        return Ok(false);
                    }
                    () = wait_for_shutdown(target_stop.clone()) => {
                        return Ok(false);
                    }
                    permit = wait_for_target_permit(
                        target_semaphore,
                        #[cfg(test)]
                        queued,
                    ) =>
                        permit.map_err(|_| PollError::Fatal("journal scheduler closed"))?,
                };
                let _permit = permit;
                if !target_admission.allows_target_work()
                    || *target_shutdown.borrow()
                    || *target_stop.borrow()
                {
                    return Ok(false);
                }
                let result = poller
                    .poll_target(
                        target,
                        target_shutdown,
                        target_stop,
                        target_admission.clone(),
                    )
                    .await;
                if matches!(result, Err(PollError::Fatal(_))) {
                    target_admission.close();
                    target_stop_sender.send_replace(true);
                    #[cfg(test)]
                    if let Some(hook) = &poller.fatal_admission_hook {
                        hook.published.wait().await;
                        hook.release.wait().await;
                    }
                }
                result
            });
        }
        let mut backlog = false;
        let mut fatal = None;
        let mut capacity_refusals = 0usize;
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(target_backlog)) => backlog |= target_backlog,
                Ok(Err(PollError::Transient)) => {
                    tracing::warn!("safe-journal transport temporarily unavailable");
                }
                Ok(Err(PollError::Capacity)) => {
                    capacity_refusals += 1;
                }
                Ok(Err(error @ PollError::Fatal(_))) => {
                    stop_sender.send_replace(true);
                    fatal.get_or_insert(error);
                }
                Err(_) => {
                    stop_sender.send_replace(true);
                    fatal.get_or_insert(PollError::Fatal("safe-journal worker failed"));
                }
            }
        }
        if capacity_refusals > 0 {
            tracing::warn!(
                capacity_refusals,
                "safe-journal archive capacity reached; target collection deferred"
            );
        }
        if let Some(error) = fatal {
            return Err(error);
        }
        Ok(backlog)
    }

    async fn prune_and_recover(&self) -> Result<(), PollError> {
        let retention_seconds = i64::from(self.retention_days.get() - 1)
            .checked_mul(86_400)
            .ok_or(PollError::Fatal("log retention is out of range"))?;
        let cutoff = ReceptionDay::from_unix_seconds(
            self.clock
                .now()?
                .checked_sub(retention_seconds)
                .ok_or(PollError::Fatal("log retention is out of range"))?,
        )
        .map_err(|_| PollError::Fatal("journal reception date is out of range"))?;
        if self
            .last_pruned_day
            .lock()
            .map_err(|_| PollError::Fatal("archive retention state poisoned"))?
            .as_ref()
            .is_some_and(|last| cutoff <= *last)
        {
            return Ok(());
        }
        self.catalog
            .store()
            .prune_archive_ledger(&cutoff)
            .await
            .map_err(|_| PollError::Fatal("archive retention commit failed"))?;
        tokio::task::block_in_place(|| self.catalog.archive().prune_before(&cutoff))
            .map_err(|_| PollError::Fatal("archive retention failed"))?;
        *self
            .last_pruned_day
            .lock()
            .map_err(|_| PollError::Fatal("archive retention state poisoned"))? = Some(cutoff);
        Ok(())
    }

    async fn poll_target(
        &self,
        snapshot: CollectionTarget,
        shutdown: tokio::sync::watch::Receiver<bool>,
        stop: tokio::sync::watch::Receiver<bool>,
        fatal_admission: FatalAdmission,
    ) -> Result<bool, PollError> {
        if !fatal_admission.allows_target_work() || *shutdown.borrow() || *stop.borrow() {
            return Ok(false);
        }
        let started = tokio::time::Instant::now();
        let deadline = started + MAX_TARGET_ELAPSED;
        let Some(initial_target) = self
            .catalog
            .begin_work(&snapshot)
            .await
            .map_err(|_| PollError::Fatal("journal target resolution failed"))?
        else {
            return Ok(false);
        };
        if !fatal_admission.allows_target_work() || *shutdown.borrow() || *stop.borrow() {
            return Ok(false);
        }
        let mut session = tokio::time::timeout_at(deadline, self.source.connect(&initial_target))
            .await
            .map_err(|_| PollError::Transient)??;
        if !fatal_admission.allows_target_work() || *shutdown.borrow() || *stop.borrow() {
            return Ok(false);
        }
        let listing = tokio::time::timeout_at(deadline, session.list())
            .await
            .map_err(|_| PollError::Transient)??;
        if listing.journals.len() > MAX_JOURNALS_PER_TARGET {
            return Err(PollError::Transient);
        }
        let journals = listing.journals;
        let session = Arc::new(tokio::sync::Mutex::new(session));
        let mut selectors = HashSet::with_capacity(journals.len());
        for journal in &journals {
            let selector =
                serde_json::to_vec(&journal.journal).map_err(|_| PollError::Transient)?;
            if selector.len() > MAX_SELECTOR_BYTES || !selectors.insert(selector) {
                return Err(PollError::Transient);
            }
        }
        if journals.is_empty() {
            return Ok(false);
        }
        let target_id = snapshot.target_id.clone();
        let mut next_index = self
            .stream_offsets
            .lock()
            .await
            .get(&target_id)
            .copied()
            .unwrap_or(0)
            % journals.len();
        let mut finished = vec![false; journals.len()];
        let mut fetched = 0usize;
        while fetched < MAX_BATCHES_PER_TARGET && finished.iter().any(|done| !done) {
            for _ in 0..journals.len() {
                let index = next_index;
                let journal = &journals[index];
                if finished[index] {
                    next_index = (index + 1) % journals.len();
                    continue;
                }
                if *shutdown.borrow()
                    || *stop.borrow()
                    || fetched == MAX_BATCHES_PER_TARGET
                    || self.clock.target_budget_expired(started)
                {
                    self.stream_offsets
                        .lock()
                        .await
                        .insert(target_id, next_index);
                    return Ok(finished.iter().any(|done| !done));
                }
                next_index = (index + 1) % journals.len();
                let Some(target) = self
                    .catalog
                    .begin_work(&snapshot)
                    .await
                    .map_err(|_| PollError::Fatal("journal target resolution failed"))?
                else {
                    return Ok(false);
                };
                fetched += 1;
                let now = self.clock.now()?;
                let state = match self
                    .catalog
                    .store()
                    .open_journal_stream(&target, &journal.journal, &journal.incarnation, now)
                    .await
                {
                    Ok(Some(state)) => state,
                    Ok(None) => return Ok(false),
                    Err(crate::store::StoreError::Saturated) => {
                        finished[index] = true;
                        continue;
                    }
                    Err(_) => return Err(PollError::Fatal("journal stream state failed")),
                };
                let collector = SingleBatchCollector::new(
                    self.catalog.archive().clone(),
                    session.clone(),
                    state,
                    self.clock.clone(),
                    deadline,
                );
                let commit = collector
                    .collect_journals()
                    .await
                    .map_err(|error| match error.downcast_ref::<PollError>() {
                        Some(error) => *error,
                        None => PollError::Fatal("journal collection failed"),
                    })?;
                let stop = commit.ends_current_drain();
                let outcome = self
                    .catalog
                    .commit_if_current(target, commit)
                    .await
                    .map_err(|_| PollError::Fatal("journal cursor commit failed"))?;
                if outcome == CommitOutcome::Stale || stop {
                    finished[index] = true;
                }
            }
        }
        let backlog = finished.iter().any(|done| !done);
        if backlog {
            self.stream_offsets
                .lock()
                .await
                .insert(target_id, next_index);
        } else {
            self.stream_offsets.lock().await.remove(&target_id);
        }
        Ok(backlog)
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Result<i64, PollError> {
        now()
    }
}

fn now() -> Result<i64, PollError> {
    unix_seconds().map_err(|_| PollError::Fatal("system clock is out of range"))
}

async fn wait_for_shutdown(mut shutdown: tokio::sync::watch::Receiver<bool>) {
    if !*shutdown.borrow() {
        let _ = shutdown.changed().await;
    }
}

async fn wait_for_target_permit(
    semaphore: Arc<tokio::sync::Semaphore>,
    #[cfg(test)] queued: Option<&tokio::sync::Notify>,
) -> Result<tokio::sync::OwnedSemaphorePermit, tokio::sync::AcquireError> {
    let permit = semaphore.acquire_owned();
    tokio::pin!(permit);
    #[cfg(test)]
    let mut queued = queued;
    std::future::poll_fn(|context| match permit.as_mut().poll(context) {
        Poll::Pending => {
            #[cfg(test)]
            if let Some(queued) = queued.take() {
                queued.notify_one();
            }
            Poll::Pending
        }
        Poll::Ready(permit) => Poll::Ready(permit),
    })
    .await
}

#[cfg(test)]
#[path = "journal_poller_tests.rs"]
mod tests;
