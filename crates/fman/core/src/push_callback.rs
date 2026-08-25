//! Core policy for FI-owned DKG completion hooks.
//!
//! Callback URLs are bearer capabilities. This module validates their destination,
//! owns retry policy, and exposes a narrow invocation capability to the seat
//! state machine. The binary composition layer implements the outbound transport.
//! None of the core errors or debug representations retain the URL, hook token,
//! or idempotency key.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fedi_decentralized_service_fleet_manager::DkgCompletionCallback;
use sha2::{Digest, Sha256};
use url::{Host, Url};

use tokio::sync::{Notify, watch};
use tokio::task::{Id, JoinHandle, JoinSet};

use crate::db::{CompletionCallbackOutcome, CompletionCallbackRecord, Db, now_ms};
use crate::facts::{CompletionCallbackReason, CompletionCallbackStatus};

const HOOK_PATH_COMPONENT_MAX_BYTES: usize = 512;

/// Production base delay while a completion callback is pending.
pub const DEFAULT_PUSH_CALLBACK_RETRY_INTERVAL: Duration = Duration::from_secs(15);
pub(crate) const MAX_PUSH_CALLBACK_RETRY_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Deployment-pinned origin allowed to receive FI completion callbacks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PushGatewayOrigin {
    url: Url,
}

/// Whether deployment policy permits development HTTP on numeric loopback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushGatewayOriginPolicy {
    /// Require HTTPS for every configured origin.
    HttpsOnly,
    /// Permit HTTP only for a numeric loopback origin.
    AllowInsecureLoopback,
}

impl PushGatewayOrigin {
    /// Parse a public gateway origin. HTTPS is mandatory unless the explicit
    /// development escape hatch is enabled for a loopback HTTP origin.
    pub fn parse(
        value: &str,
        policy: PushGatewayOriginPolicy,
    ) -> Result<Self, PushGatewayOriginError> {
        let url = Url::parse(value).map_err(|_| PushGatewayOriginError::InvalidUrl)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(PushGatewayOriginError::NotAnOrigin);
        }
        match url.scheme() {
            "https" => {}
            "http"
                if policy == PushGatewayOriginPolicy::AllowInsecureLoopback
                    && host_is_loopback(&url) => {}
            _ => return Err(PushGatewayOriginError::HttpsRequired),
        }
        Ok(Self { url })
    }

    /// Validate one callback without retaining or echoing its bearer value in
    /// errors. This is repeated after restart so a changed deployment origin
    /// fails closed instead of invoking an old destination.
    pub fn validate(
        &self,
        callback: &DkgCompletionCallback,
    ) -> Result<ValidatedDkgCompletionCallback, PushCallbackValidationError> {
        let url = Url::parse(callback.callback_url())
            .map_err(|_| PushCallbackValidationError::InvalidUrl)?;
        if url.origin() != self.url.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(PushCallbackValidationError::OriginOrShapeMismatch);
        }
        let components = url
            .path_segments()
            .ok_or(PushCallbackValidationError::OriginOrShapeMismatch)?
            .collect::<Vec<_>>();
        if components.len() != 3
            || components[0] != "hooks"
            || components[1].is_empty()
            || components[2].is_empty()
            || HOOK_PATH_COMPONENT_MAX_BYTES < components[1].len()
            || HOOK_PATH_COMPONENT_MAX_BYTES < components[2].len()
        {
            return Err(PushCallbackValidationError::OriginOrShapeMismatch);
        }
        Ok(ValidatedDkgCompletionCallback {
            callback: callback.clone(),
        })
    }
}

fn host_is_loopback(url: &Url) -> bool {
    match url.host() {
        // Do not trust DNS to keep a name inside the local-host boundary.
        Some(Host::Domain(_)) => false,
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        None => false,
    }
}

/// A callback whose destination matches the deployment-pinned gateway.
#[derive(Clone, Eq, PartialEq)]
pub struct ValidatedDkgCompletionCallback {
    callback: DkgCompletionCallback,
}

impl std::fmt::Debug for ValidatedDkgCompletionCallback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedDkgCompletionCallback")
            .field("callback", &"<redacted>")
            .finish()
    }
}

impl ValidatedDkgCompletionCallback {
    /// Exact origin-validated bearer URL. Capability adapters must not format it.
    pub fn callback_url(&self) -> &str {
        self.callback.callback_url()
    }

    /// Stable invocation key supplied by the FI.
    pub fn idempotency_key(&self) -> &str {
        self.callback.idempotency_key()
    }

    pub(crate) fn into_inner(self) -> DkgCompletionCallback {
        self.callback
    }
}

/// Injected outbound capability. Core owns callback durability and policy; the
/// composition root owns the network implementation and its dependencies.
#[async_trait::async_trait]
pub trait CompletionCallbackInvoker: Send + Sync {
    /// False when adapter construction failed. Core exposes this as stable
    /// operator-blocked state without consuming a delivery attempt.
    fn is_available(&self) -> bool {
        true
    }

    /// Invoke one already origin-validated capability.
    async fn invoke(&self, callback: &ValidatedDkgCompletionCallback) -> CallbackAttemptOutcome;
}

#[cfg(test)]
pub(crate) struct TestCallbackInvoker;

#[cfg(test)]
#[async_trait::async_trait]
impl CompletionCallbackInvoker for TestCallbackInvoker {
    async fn invoke(&self, _callback: &ValidatedDkgCompletionCallback) -> CallbackAttemptOutcome {
        CallbackAttemptOutcome::Delivered
    }
}

/// Sanitized result of one network invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallbackAttemptOutcome {
    /// The gateway durably accepted the idempotent callback.
    Delivered,
    /// A transient condition requires a later retry.
    Retryable(CompletionCallbackReason),
    /// A definitive response permanently ends delivery.
    Terminal(CompletionCallbackReason),
}

/// Fleet-wide reconciler for durable completion-hook work.
///
/// Formation and decommission only mutate SQLite and wake the worker. The
/// worker derives deliverability by joining callback work to the immutable
/// formed-seat row, so no seat runtime mirror or child probe participates in
/// delivery. The periodic scan is the correctness path; marks only reduce
/// latency after a durable transition.
pub(crate) struct CompletionHookWorker {
    wake: Arc<Notify>,
    runtime: Mutex<Option<CompletionHookRuntime>>,
}

/// Non-owning promptness handle handed to seat runtimes. Dropping a Fleet
/// still shuts the worker down even when a caller retains a Seat handle.
#[derive(Clone)]
pub(crate) struct CompletionHookWake(Arc<Notify>);

impl CompletionHookWake {
    pub(crate) fn mark(&self) {
        self.0.notify_one();
    }
}

struct CompletionHookRuntime {
    shutdown: watch::Sender<bool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for CompletionHookRuntime {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
        // Detach cleanup rather than aborting it. The task retains the data-root
        // lock until it observes shutdown and drops its JoinSet, which aborts
        // every bearer-holding invocation.
        self.handle.take();
    }
}

impl CompletionHookWorker {
    pub(crate) fn new(
        db: Db,
        gateway_origin: Option<PushGatewayOrigin>,
        retry_base: Duration,
        invoker: Arc<dyn CompletionCallbackInvoker>,
    ) -> Self {
        let wake = Arc::new(Notify::new());
        let (shutdown, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(run_completion_hooks(
            db,
            gateway_origin,
            retry_base,
            invoker,
            wake.clone(),
            shutdown_rx,
        ));
        Self {
            wake,
            runtime: Mutex::new(Some(CompletionHookRuntime {
                shutdown,
                handle: Some(handle),
            })),
        }
    }

    pub(crate) fn wake_handle(&self) -> CompletionHookWake {
        CompletionHookWake(self.wake.clone())
    }

    /// Cancel and join every in-process invocation before releasing bearer
    /// material or allowing the same data root to reopen.
    pub(crate) async fn shutdown(&self) {
        let Some(mut runtime) = self
            .runtime
            .lock()
            .expect("completion-hook runtime lock is never poisoned")
            .take()
        else {
            return;
        };
        runtime.shutdown.send_replace(true);
        if let Some(handle) = runtime.handle.take() {
            let _ = handle.await;
        }
    }
}

async fn run_completion_hooks(
    db: Db,
    gateway_origin: Option<PushGatewayOrigin>,
    retry_base: Duration,
    invoker: Arc<dyn CompletionCallbackInvoker>,
    wake: Arc<Notify>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tasks = JoinSet::new();
    let mut active = HashMap::new();
    let mut scan = tokio::time::interval(retry_base);
    scan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            result = tasks.join_next_with_id(), if !tasks.is_empty() => {
                let id = match &result {
                    Some(Ok((id, _))) => *id,
                    Some(Err(error)) => error.id(),
                    None => continue,
                };
                let Some(seat_id) = active.remove(&id) else {
                    continue;
                };
                match result {
                    Some(Ok((_, outcome))) => {
                        finish_callback(&db, &seat_id, outcome).await;
                    }
                    Some(Err(error)) => {
                        tracing::warn!(
                            seat_id = %seat_id,
                            cancelled = error.is_cancelled(),
                            panicked = error.is_panic(),
                            "completion-hook invocation task failed; durable work remains pending"
                        );
                    }
                    None => {}
                }
            }
            _ = scan.tick() => {
                schedule_callbacks(&db, gateway_origin.as_ref(), retry_base, &invoker, &mut tasks, &mut active).await;
            }
            () = wake.notified() => {
                schedule_callbacks(&db, gateway_origin.as_ref(), retry_base, &invoker, &mut tasks, &mut active).await;
            }
        }
    }
    // Abort and join every invocation before releasing the worker's data-root
    // lock. This is the bearer cleanup boundary that graceful shutdown waits
    // to cross; a detached worker created by bare drop retains the lock until
    // it reaches the same boundary.
    tasks.shutdown().await;
    active.clear();
}

async fn schedule_callbacks(
    db: &Db,
    gateway_origin: Option<&PushGatewayOrigin>,
    retry_base: Duration,
    invoker: &Arc<dyn CompletionCallbackInvoker>,
    tasks: &mut JoinSet<CallbackAttemptOutcome>,
    active: &mut HashMap<Id, fedi_decentralized_service_fleet_manager::SeatId>,
) {
    let callbacks = match db.deliverable_completion_callbacks().await {
        Ok(callbacks) => callbacks,
        Err(error) => {
            tracing::warn!(
                error = format_args!("{error:#}"),
                "failed to enumerate completion-hook work"
            );
            return;
        }
    };
    for record in callbacks {
        let Some(callback) = record.callback.as_ref() else {
            continue;
        };
        if active.values().any(|seat_id| *seat_id == record.seat_id) {
            continue;
        }
        if let CompletionCallbackStatus::Pending {
            next_attempt_at_ms, ..
        } = &record.status
            && now_ms() < *next_attempt_at_ms
        {
            continue;
        }
        let Some(origin) = gateway_origin else {
            mark_operator_blocked(db, &record, CompletionCallbackReason::GatewayOriginMissing)
                .await;
            continue;
        };
        let callback = match origin.validate(callback) {
            Ok(callback) => callback,
            Err(_) => {
                mark_operator_blocked(db, &record, CompletionCallbackReason::GatewayOriginMismatch)
                    .await;
                continue;
            }
        };
        if !invoker.is_available() {
            mark_operator_blocked(db, &record, CompletionCallbackReason::HttpClientUnavailable)
                .await;
            continue;
        }
        let attempts = record.status.attempts().saturating_add(1);
        let delay = retry_delay(retry_base, attempts, record.seat_id.as_bytes().as_slice());
        let next_attempt_at_ms =
            now_ms().saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX));
        match db
            .record_completion_callback_attempt_started(&record.seat_id, next_attempt_at_ms)
            .await
        {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                tracing::warn!(seat_id = %record.seat_id, error = format_args!("{error:#}"), "failed to persist completion-hook attempt before network I/O");
                continue;
            }
        }
        let invoker = invoker.clone();
        let task = tasks.spawn(async move { invoker.invoke(&callback).await });
        active.insert(task.id(), record.seat_id);
    }
}

async fn mark_operator_blocked(
    db: &Db,
    record: &CompletionCallbackRecord,
    reason: CompletionCallbackReason,
) {
    if matches!(
        record.status,
        CompletionCallbackStatus::OperatorBlocked { reason: current, .. } if current == reason
    ) {
        return;
    }
    match db
        .record_completion_callback_operator_blocked(&record.seat_id, reason)
        .await
    {
        Ok(true) => tracing::warn!(
            seat_id = %record.seat_id,
            reason = reason.as_str(),
            "completion-hook delivery requires operator configuration"
        ),
        Ok(false) => {}
        Err(error) => tracing::warn!(
            seat_id = %record.seat_id,
            error = format_args!("{error:#}"),
            "failed to persist operator-blocked completion-hook state"
        ),
    }
}

async fn finish_callback(
    db: &Db,
    seat_id: &fedi_decentralized_service_fleet_manager::SeatId,
    outcome: CallbackAttemptOutcome,
) {
    let result = match outcome {
        CallbackAttemptOutcome::Delivered => db
            .record_completion_callback_completed(seat_id, CompletionCallbackOutcome::Delivered)
            .await
            .map(|value| value.is_some()),
        CallbackAttemptOutcome::Terminal(reason) => db
            .record_completion_callback_completed(
                seat_id,
                CompletionCallbackOutcome::Terminal(reason),
            )
            .await
            .map(|value| value.is_some()),
        CallbackAttemptOutcome::Retryable(reason) => {
            db.record_completion_callback_retry_reason(seat_id, reason)
                .await
        }
    };
    if let Err(error) = result {
        // The gateway may already have accepted. Retaining pending work safely
        // retries the same idempotency key.
        tracing::warn!(
            seat_id = %seat_id,
            error = format_args!("{error:#}"),
            "failed to persist completion-hook outcome"
        );
    }
}

/// Deterministic per-seat jitter avoids synchronized guardian retries while
/// keeping restart/test behavior reproducible.
pub(crate) fn retry_delay(base: Duration, attempt: u32, entropy: &[u8]) -> Duration {
    let exponent = attempt.saturating_sub(1).min(16);
    let exponential_ms = base
        .as_millis()
        .saturating_mul(1_u128 << exponent)
        .min(MAX_PUSH_CALLBACK_RETRY_INTERVAL.as_millis());
    let mut hash = Sha256::new();
    hash.update(entropy);
    hash.update(attempt.to_be_bytes());
    let digest = hash.finalize();
    let jitter_percent = 80_u128 + u128::from(digest[0] % 41);
    let jittered_ms = exponential_ms
        .saturating_mul(jitter_percent)
        .saturating_div(100)
        .max(1)
        .min(MAX_PUSH_CALLBACK_RETRY_INTERVAL.as_millis());
    Duration::from_millis(u64::try_from(jittered_ms).unwrap_or(u64::MAX))
}

/// Invalid deployment origin. The input is deliberately absent from errors.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PushGatewayOriginError {
    #[error("push-gateway origin is not a valid absolute URL")]
    InvalidUrl,
    #[error(
        "push-gateway URL must contain only an origin (no credentials, path, query, or fragment)"
    )]
    NotAnOrigin,
    #[error("push-gateway origin must use HTTPS; development HTTP is allowed only for loopback")]
    HttpsRequired,
}

/// Invalid callback capability. The bearer input is deliberately absent.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PushCallbackValidationError {
    #[error("DKG completion callback URL is invalid")]
    InvalidUrl,
    #[error("DKG completion callback must be an exact hook under the configured gateway origin")]
    OriginOrShapeMismatch,
}

#[cfg(test)]
#[path = "../tests/push_callback.rs"]
mod tests;
