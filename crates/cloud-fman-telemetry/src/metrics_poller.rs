//! Low-frequency authenticated guardian metrics polling.

use std::{
    collections::BTreeSet,
    str::FromStr as _,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use fedi_decentralized_service_fleet_manager::{
    GUARDIAN_TELEMETRY_ALPN, GuardianTelemetryApi as _, GuardianTelemetryApiClient,
    ListGuardianTelemetrySeatsRequest, MAX_GUARDIAN_METRICS_BODY_BYTES,
    ScrapeGuardianMetricsRequest,
};
use fedi_iroh_rpc::{
    RpcClient,
    iroh::{Endpoint, EndpointAddr, EndpointId},
};

use crate::{
    metrics_observability::{AdmissionOutcome, MetricsObservability},
    metrics_policy::{MetricsIdentity, MetricsPolicy},
    metrics_types::{MetricsCommit, SeatObservation},
    metrics_worker::{
        CollectionTarget, CommitOutcome, MetricsCollector, TargetCatalog, WorkTarget,
    },
    store::Store,
};

const MAX_SEATS: usize = 64;
const MAX_TARGET_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_TARGET_BUDGET: Duration = Duration::from_secs(60);

/// Metrics-specific catalog adapter over the shared target fencing store.
#[derive(Clone)]
struct MetricsCatalog {
    store: Store,
    cadence_seconds: i64,
}

#[derive(Debug)]
pub(crate) enum MetricsPollError {
    Clock,
    DueTargets,
    NextDue,
    ReserveAttempt,
    BeginWork,
    BeginDeadline,
    Commit,
    Semaphore,
    TargetTask,
}

impl std::fmt::Display for MetricsPollError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Clock => formatter.write_str("metrics clock failed"),
            Self::DueTargets => formatter.write_str("metrics due-target lookup failed"),
            Self::NextDue => formatter.write_str("metrics next-deadline lookup failed"),
            Self::ReserveAttempt => formatter.write_str("metrics attempt reservation failed"),
            Self::BeginWork => formatter.write_str("metrics target resolution failed"),
            Self::BeginDeadline => formatter.write_str("metrics target resolution timed out"),
            Self::Commit => formatter.write_str("metrics snapshot commit failed"),
            Self::Semaphore => formatter.write_str("metrics concurrency gate closed"),
            Self::TargetTask => formatter.write_str("metrics target task failed"),
        }
    }
}
impl std::error::Error for MetricsPollError {}

impl MetricsCatalog {
    async fn due_targets(&self) -> Result<Vec<CollectionTarget>, MetricsPollError> {
        let now = unix_seconds().map_err(|_| MetricsPollError::Clock)?;
        self.store
            .due_metric_targets(now)
            .await
            .map_err(|_| MetricsPollError::DueTargets)
    }

    async fn reserve(&self, target: &CollectionTarget) -> Result<bool, MetricsPollError> {
        let now = unix_seconds().map_err(|_| MetricsPollError::Clock)?;
        self.store
            .reserve_metric_attempt(target, now, self.cadence_seconds)
            .await
            .map_err(|_| MetricsPollError::ReserveAttempt)
    }

    async fn resolve(
        &self,
        target: &CollectionTarget,
    ) -> Result<Option<WorkTarget>, MetricsPollError> {
        let now = unix_seconds().map_err(|_| MetricsPollError::Clock)?;
        self.store
            .begin_collection_work(target, now)
            .await
            .map_err(|_| MetricsPollError::BeginWork)
    }

    async fn commit(
        &self,
        target: WorkTarget,
        commit: MetricsCommit,
    ) -> Result<CommitOutcome, MetricsPollError> {
        let now = unix_seconds().map_err(|_| MetricsPollError::Clock)?;
        self.store
            .commit_metrics(&target, commit, now)
            .await
            .map_err(|_| MetricsPollError::Commit)
    }
}

#[async_trait::async_trait]
impl TargetCatalog for MetricsCatalog {
    type Commit = MetricsCommit;

    async fn active_targets(
        &self,
    ) -> Result<Vec<CollectionTarget>, Box<dyn std::error::Error + Send + Sync>> {
        self.due_targets()
            .await
            .map_err(|error| Box::new(error) as _)
    }

    async fn begin_work(
        &self,
        target: &CollectionTarget,
    ) -> Result<Option<WorkTarget>, Box<dyn std::error::Error + Send + Sync>> {
        if !self
            .reserve(target)
            .await
            .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?
        {
            return Ok(None);
        }
        self.resolve(target)
            .await
            .map_err(|error| Box::new(error) as _)
    }

    async fn commit_if_current(
        &self,
        target: WorkTarget,
        commit: Self::Commit,
    ) -> Result<CommitOutcome, Box<dyn std::error::Error + Send + Sync>> {
        self.commit(target, commit)
            .await
            .map_err(|error| Box::new(error) as _)
    }
}

/// Authenticated direct-Iroh poller for all registered FMan targets.
#[derive(Clone)]
pub(crate) struct MetricsPoller {
    /// Durable target and snapshot state.
    store: Store,
    /// Process-wide Iroh client endpoint.
    endpoint: Endpoint,
    /// Exact expected source release version.
    source_version: String,
    /// Exact expected source release hash.
    source_version_hash: String,
    /// Explicit readiness gate for upstream method canonicalization.
    canonical_method_labels: bool,
    /// Maximum concurrent FMan polls.
    concurrency: std::num::NonZeroUsize,
    cadence: Duration,
    connect_address: Option<EndpointAddr>,
    /// Fixed-cardinality admission counters and rate-limited diagnostics.
    observability: MetricsObservability,
}

impl MetricsPoller {
    /// Construct a bounded poller.
    pub(crate) fn new(
        store: Store,
        endpoint: Endpoint,
        source_version: String,
        source_version_hash: String,
        canonical_method_labels: bool,
        concurrency: std::num::NonZeroUsize,
        cadence: Duration,
    ) -> Self {
        Self {
            store,
            endpoint,
            source_version,
            source_version_hash,
            canonical_method_labels,
            concurrency,
            cadence,
            connect_address: None,
            observability: MetricsObservability::default(),
        }
    }

    /// Share this poller's process-local admission observability with private exposition.
    pub(crate) fn observability(&self) -> MetricsObservability {
        self.observability.clone()
    }

    /// Route one matching endpoint directly for deterministic Defe validation.
    pub(crate) fn with_address_override(mut self, address: Option<EndpointAddr>) -> Self {
        self.connect_address = address;
        self
    }

    /// Poll immediately and then no more often than the configured cadence.
    pub(crate) async fn run(
        self,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), MetricsPollError> {
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            let cycle = self.poll_once(&mut shutdown).await;
            if *shutdown.borrow() {
                cycle?;
                return Ok(());
            }
            let delay = match cycle {
                Ok(()) => {
                    let now = unix_seconds().map_err(|_| MetricsPollError::Clock)?;
                    match self.store.next_metric_due_at(now).await {
                        Ok(next_due) => wake_delay(now, next_due, self.cadence),
                        Err(_) => {
                            tracing::warn!(
                                error = %MetricsPollError::NextDue,
                                "metrics polling cycle deferred"
                            );
                            self.cadence
                        }
                    }
                }
                Err(error @ (MetricsPollError::DueTargets | MetricsPollError::ReserveAttempt)) => {
                    // No remote attempt begins until its cadence fence commits. Local
                    // contention therefore backs off in-process without contacting FMan.
                    tracing::warn!(error = %error, "metrics polling cycle deferred");
                    self.cadence
                }
                Err(error) => return Err(error),
            };
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { return Ok(()); }
                }
                () = tokio::time::sleep(delay) => {}
            }
        }
    }

    async fn poll_once(
        &self,
        shutdown: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), MetricsPollError> {
        let cadence_seconds =
            i64::try_from(self.cadence.as_secs()).map_err(|_| MetricsPollError::Clock)?;
        let catalog = MetricsCatalog {
            store: self.store.clone(),
            cadence_seconds,
        };
        let targets = catalog.due_targets().await?;
        let target_budget = fair_target_budget(self.cadence, self.concurrency.get(), targets.len());
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(self.concurrency.get()));
        let mut tasks = tokio::task::JoinSet::new();
        let mut targets = targets.into_iter();
        let mut accepting = true;
        let mut fatal = None;
        loop {
            while accepting && fatal.is_none() && tasks.len() < self.concurrency.get() {
                if *shutdown.borrow() {
                    accepting = false;
                    break;
                }
                let Some(target) = targets.next() else {
                    accepting = false;
                    break;
                };
                let permit = permits
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| MetricsPollError::Semaphore)?;
                let poller = self.clone();
                let catalog = catalog.clone();
                let task_shutdown = shutdown.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    if !catalog.reserve(&target).await? {
                        return Ok::<_, MetricsPollError>(());
                    }
                    let started = tokio::time::Instant::now();
                    let deadline = started + target_budget;
                    let phase_budget = target_budget / 4;
                    let Some(work) = tokio::time::timeout(phase_budget, catalog.resolve(&target))
                        .await
                        .map_err(|_| MetricsPollError::BeginDeadline)??
                    else {
                        return Ok::<_, MetricsPollError>(());
                    };
                    let collection_deadline = deadline.checked_sub(phase_budget).unwrap_or(started);
                    let commit = if *task_shutdown.borrow() {
                        failed_commit()
                    } else {
                        poller.collect_target(&work, collection_deadline).await
                    };
                    // Once started, durability is a join barrier just like journal
                    // cursor commit. Shutdown and sibling errors cannot cancel it.
                    catalog.commit(work, commit).await?;
                    Ok(())
                });
            }
            if tasks.is_empty() {
                break;
            }
            let result = if accepting && fatal.is_none() {
                tokio::select! {
                    biased;
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            accepting = false;
                        }
                        continue;
                    }
                    result = tasks.join_next() => result,
                }
            } else {
                tasks.join_next().await
            };
            let Some(result) = result else {
                break;
            };
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error @ (MetricsPollError::Clock | MetricsPollError::ReserveAttempt))) => {
                    accepting = false;
                    fatal.get_or_insert(error);
                }
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "metrics target work failed");
                }
                Err(_) => {
                    accepting = false;
                    fatal.get_or_insert(MetricsPollError::TargetTask);
                }
            }
        }
        fatal.map_or(Ok(()), Err)
    }

    async fn collect_target(
        &self,
        target: &WorkTarget,
        deadline: tokio::time::Instant,
    ) -> MetricsCommit {
        let Some(client) = self.connect(target, deadline).await else {
            return failed_commit();
        };
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            return failed_commit();
        };
        let Ok(Ok(response)) = tokio::time::timeout(
            REQUEST_TIMEOUT.min(remaining),
            client.list_guardian_telemetry_seats(ListGuardianTelemetrySeatsRequest {
                capability: target.capability().clone(),
            }),
        )
        .await
        else {
            return failed_commit();
        };
        if response.seats.len() > MAX_SEATS {
            return failed_commit();
        }
        let listed: BTreeSet<String> = response
            .seats
            .iter()
            .map(|seat| seat.seat_id.to_string())
            .collect();
        if listed.len() != response.seats.len() {
            return failed_commit();
        }
        let mut snapshots = Vec::new();
        let mut complete = true;
        let mut total_bytes = 0usize;
        for seat in response.seats {
            let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now())
            else {
                complete = false;
                break;
            };
            let Ok(Ok(upstream)) = tokio::time::timeout(
                REQUEST_TIMEOUT.min(remaining),
                client.scrape_guardian_metrics(ScrapeGuardianMetricsRequest {
                    seat_id: seat.seat_id.clone(),
                    capability: target.capability().clone(),
                }),
            )
            .await
            else {
                complete = false;
                continue;
            };
            if upstream.status_code != 200
                || upstream.content_encoding.is_some()
                || !upstream
                    .content_type
                    .as_deref()
                    .is_some_and(|value| value.starts_with("text/plain"))
                || upstream.body.len() > MAX_GUARDIAN_METRICS_BODY_BYTES
            {
                complete = false;
                continue;
            }
            let seat_id = seat.seat_id.to_string();
            let policy = MetricsPolicy {
                version: &self.source_version,
                version_hash: &self.source_version_hash,
                canonical_method_labels: self.canonical_method_labels,
            };
            let admitted = match policy.admit_until(
                &upstream.body,
                MetricsIdentity {
                    fman_id: target.fman_id(),
                    fman_name: target.fman_name(),
                    guardian_seat_id: &seat_id,
                },
                Some(deadline.into_std()),
            ) {
                Ok(admitted) => admitted,
                Err(_) => {
                    self.observability.record(AdmissionOutcome::Rejected);
                    complete = false;
                    continue;
                }
            };
            self.observability.record(AdmissionOutcome::Admitted);
            if admitted.discarded_known_deny {
                self.observability
                    .record(AdmissionOutcome::KnownDenyDiscarded);
            }
            if admitted.discarded_unknown {
                self.observability
                    .record(AdmissionOutcome::UnknownDiscarded);
            }
            if admitted.discarded_invalid_admitted {
                self.observability
                    .record(AdmissionOutcome::InvalidAdmittedDiscarded);
            }
            if tokio::time::Instant::now() >= deadline {
                complete = false;
                break;
            }
            let mut bytes = 0usize;
            for sample in &admitted.samples {
                if tokio::time::Instant::now() >= deadline {
                    complete = false;
                    break;
                }
                bytes = bytes.saturating_add(sample.len() + 1);
            }
            if !complete {
                break;
            }
            total_bytes = total_bytes.saturating_add(bytes);
            if total_bytes > MAX_TARGET_SNAPSHOT_BYTES {
                complete = false;
                continue;
            }
            let Ok(observed_at_ms) = unix_millis() else {
                complete = false;
                continue;
            };
            if tokio::time::Instant::now() >= deadline {
                complete = false;
                break;
            }
            snapshots.push(SeatObservation {
                guardian_seat_id: seat_id,
                observed_at_ms,
                samples: admitted.samples,
            });
        }
        MetricsCommit {
            listed_seats: Some(listed),
            snapshots,
            complete,
        }
    }

    async fn connect(
        &self,
        target: &WorkTarget,
        deadline: tokio::time::Instant,
    ) -> Option<GuardianTelemetryApiClient> {
        let endpoint_id = EndpointId::from_str(target.endpoint_id()).ok()?;
        let address = self
            .connect_address
            .clone()
            .filter(|address| address.id == endpoint_id)
            .unwrap_or_else(|| EndpointAddr::new(endpoint_id));
        let remaining = deadline.checked_duration_since(tokio::time::Instant::now())?;
        let connection = tokio::time::timeout(
            Duration::from_secs(10).min(remaining),
            self.endpoint.connect(address, GUARDIAN_TELEMETRY_ALPN),
        )
        .await
        .ok()?
        .ok()?;
        Some(GuardianTelemetryApiClient::from_rpc_client(
            RpcClient::with_limits(
                connection,
                4 * 1024,
                MAX_GUARDIAN_METRICS_BODY_BYTES + 64 * 1024,
            ),
        ))
    }
}

fn fair_target_budget(cadence: Duration, concurrency: usize, targets: usize) -> Duration {
    let waves = targets.div_ceil(concurrency).max(1);
    (cadence / u32::try_from(waves).unwrap_or(u32::MAX)).min(MAX_TARGET_BUDGET)
}

fn wake_delay(now: i64, next_due: Option<i64>, idle: Duration) -> Duration {
    next_due.map_or(idle, |deadline| {
        Duration::from_secs(u64::try_from(deadline.saturating_sub(now)).unwrap_or(0))
    })
}

#[cfg(test)]
#[path = "metrics_poller_tests.rs"]
mod tests;

#[async_trait::async_trait]
impl MetricsCollector for MetricsPoller {
    type Commit = MetricsCommit;

    async fn collect_metrics(
        &self,
        target: &WorkTarget,
    ) -> Result<Self::Commit, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self
            .collect_target(target, tokio::time::Instant::now() + MAX_TARGET_BUDGET)
            .await)
    }
}

fn failed_commit() -> MetricsCommit {
    MetricsCommit {
        listed_seats: None,
        snapshots: Vec::new(),
        complete: false,
    }
}

fn unix_seconds() -> Result<i64, std::time::SystemTimeError> {
    Ok(i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()).unwrap_or(i64::MAX))
}

fn unix_millis() -> Result<i64, std::time::SystemTimeError> {
    Ok(
        i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
            .unwrap_or(i64::MAX),
    )
}
