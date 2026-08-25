use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::{sync::Notify, task::JoinHandle};

use crate::{
    ClaimDueOutcome, DELIVERY_RESOLUTION_DEADLINE_SECONDS, DeliveryOutboxFailure,
    DeliveryOutboxRepository, MarkFailedOutcome, Observability, PushProvider, PushProviderError,
    PushProviderErrorKind, log_sanitizer::sanitize_log_value,
};

/// Maximum idle wakeup interval when there is no known due outbox row.
///
/// Normal operation is notification/deadline driven: HTTP invocations notify
/// the worker after enqueueing rows, and retry rows are revisited at their
/// `next_attempt_at` deadline. This bounded fallback exists only to recover
/// from missed in-process notifications, manual database changes, and coarse
/// wall-clock deadline calculations.
const FALLBACK_IDLE_POLL: Duration = Duration::from_secs(1);
/// Bounds one provider call, including custom provider implementations.
///
/// The FCM client has a tighter HTTP timeout, but the worker owns the
/// service-wide bound so a provider implementation cannot indefinitely prevent
/// durable terminal resolution.
const PROVIDER_DELIVERY_TIMEOUT: Duration = Duration::from_secs(15);

/// Background worker handle used to request graceful shutdown and await drain.
#[derive(Debug)]
pub struct DeliveryWorkerHandle {
    /// Shutdown notification shared with the worker task.
    shutdown: Arc<Notify>,
    /// Retained shutdown state so notifications cannot be missed.
    stopped: Arc<AtomicBool>,
    /// Worker task join handle.
    join_handle: JoinHandle<()>,
    /// Shared operational state updated by the handle.
    observability: Observability,
}

impl DeliveryWorkerHandle {
    /// Requests worker shutdown and waits for the loop to finish its current item.
    pub async fn shutdown(self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.observability.set_worker_shutdown_requested();
        self.shutdown.notify_waiters();
        let _ = self.join_handle.await;
    }
}

/// Starts the durable delivery worker using the process-wide enqueue wakeup.
pub(crate) fn start_delivery_worker(
    outbox: DeliveryOutboxRepository,
    provider: Arc<dyn PushProvider>,
    max_concurrency: usize,
    observability: Observability,
    wakeup: Arc<Notify>,
    database_write_lock: fedi_decentralized_push_gateway_storage::DatabaseWriteLock,
) -> DeliveryWorkerHandle {
    start_delivery_worker_inner(
        outbox,
        provider,
        max_concurrency,
        observability,
        wakeup,
        database_write_lock,
        FALLBACK_IDLE_POLL,
    )
}

/// Starts a worker with the supplied idle fallback interval.
fn start_delivery_worker_inner(
    outbox: DeliveryOutboxRepository,
    provider: Arc<dyn PushProvider>,
    max_concurrency: usize,
    observability: Observability,
    wakeup: Arc<Notify>,
    database_write_lock: fedi_decentralized_push_gateway_storage::DatabaseWriteLock,
    fallback_idle_poll: Duration,
) -> DeliveryWorkerHandle {
    let shutdown = Arc::new(Notify::new());
    let stopped = Arc::new(AtomicBool::new(false));
    let worker_shutdown = shutdown.clone();
    let worker_stopped = stopped.clone();
    let worker_observability = observability.clone();
    let join_handle = tokio::spawn(async move {
        worker_observability.set_worker_running(true);
        let reset_result = {
            let _guard = database_write_lock.acquire_worker().await;
            outbox.reset_in_progress().await
        };
        if let Err(err) = reset_result {
            eprintln!(
                "event=outbox_worker_error operation=reset_in_progress error={}",
                sanitized_log_error(&err)
            );
        }
        let max_concurrency = max_concurrency.max(1);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrency));
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            if worker_stopped.load(Ordering::SeqCst) {
                break;
            }

            let expiry_result = {
                let _guard = database_write_lock.acquire_worker().await;
                outbox.expire_delivery_resolution_deadlines().await
            };
            match expiry_result {
                Ok(expired) => worker_observability.record_dead_letters(expired),
                Err(err) => {
                    worker_observability.record_delivery_failure();
                    eprintln!(
                        "event=outbox_worker_error operation=expire_delivery_resolution_deadlines error={}",
                        sanitized_log_error(&err)
                    );
                }
            }

            while tasks.len() < max_concurrency {
                if worker_stopped.load(Ordering::SeqCst) {
                    break;
                }
                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => break,
                };
                worker_observability.record_outbox_claim_query();
                let claim_result = {
                    let _guard = database_write_lock.acquire_worker().await;
                    outbox.claim_due().await
                };
                let claimed = match claim_result {
                    Ok(ClaimDueOutcome::Claimed(claimed)) => *claimed,
                    Ok(ClaimDueOutcome::CorruptedDeadLetter) => {
                        worker_observability.record_delivery_failure();
                        worker_observability.record_dead_letter();
                        continue;
                    }
                    Ok(ClaimDueOutcome::Empty) => break,
                    Err(err) => {
                        worker_observability.record_delivery_failure();
                        eprintln!(
                            "event=outbox_worker_error operation=claim_due error={}",
                            sanitized_log_error(&err)
                        );
                        break;
                    }
                };
                worker_observability.record_outbox_claim();
                let outbox_clone = outbox.clone();
                let provider_clone = provider.clone();
                let task_observability = worker_observability.clone();
                let task_database_write_lock = database_write_lock.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    let delivery_result = deliver_provider_call(
                        claimed.created_at,
                        crate::time::unix_timestamp(),
                        provider_clone.deliver(&claimed.registration, &claimed.notification),
                    )
                    .await;
                    match delivery_result {
                        Ok(()) => {
                            let mark_result = {
                                let _guard = task_database_write_lock.acquire_worker().await;
                                outbox_clone
                                    .mark_succeeded(&claimed.outbox_id, &claimed.claim_id)
                                    .await
                            };
                            match mark_result {
                                Ok(true) => task_observability.record_delivery_success(),
                                Ok(false) => {}
                                Err(err) => eprintln!(
                                    "event=outbox_worker_error operation=mark_success error={}",
                                    sanitized_log_error(&err)
                                ),
                            }
                        }
                        Err(err) => {
                            task_observability.record_delivery_failure_reason(err.reason);
                            if err.disables_registration() {
                                let mark_result = {
                                let _guard = task_database_write_lock.acquire_worker().await;
                                    outbox_clone
                                        .mark_invalid_token_and_disable_registration(
                                            &claimed,
                                            err.reason,
                                        )
                                        .await
                                };
                                if let Err(mark_err) = mark_result {
                                    task_observability.record_invalid_token_cleanup_failure();
                                    eprintln!(
                                        "event=outbox_worker_error operation=mark_invalid_token error={}",
                                        sanitized_log_error(&mark_err)
                                    );
                                }
                            } else if let Some(outbox_error) =
                                outbox_failure_from_delivery_error(&err)
                            {
                                let mark_result = {
                                let _guard = task_database_write_lock.acquire_worker().await;
                                    outbox_clone
                                        .mark_failed(
                                            &claimed.outbox_id,
                                            &claimed.claim_id,
                                            &outbox_error,
                                        )
                                        .await
                                };
                                match mark_result {
                                    Ok(MarkFailedOutcome::DeadLettered) => {
                                        task_observability.record_dead_letter();
                                    }
                                    Ok(MarkFailedOutcome::NotUpdated | MarkFailedOutcome::Retrying) => {}
                                    Err(mark_err) => {
                                        eprintln!(
                                            "event=outbox_worker_error operation=mark_failure error={}",
                                            sanitized_log_error(&mark_err)
                                        );
                                    }
                                }
                            }
                        }
                    }
                });
            }

            let next_claim_delay = if tasks.len() < max_concurrency {
                next_claim_delay(&outbox, fallback_idle_poll).await
            } else {
                None
            };

            if tasks.is_empty() {
                worker_observability.record_outbox_idle_wait();
            }
            tokio::select! {
                () = worker_shutdown.notified() => break,
                () = wakeup.notified() => {}
                Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                    if let Err(err) = result {
                        eprintln!("push delivery worker task failed: {err}");
                    }
                }
                () = sleep_optional(next_claim_delay) => {}
            }
        }

        while let Some(result) = tasks.join_next().await {
            if let Err(err) = result {
                eprintln!("push delivery worker task failed during shutdown: {err}");
            }
        }
        worker_observability.set_worker_running(false);
    });

    DeliveryWorkerHandle {
        shutdown,
        stopped,
        join_handle,
        observability,
    }
}

/// Starts a worker with a test-selected idle fallback interval.
#[cfg(test)]
pub(crate) fn start_delivery_worker_with_fallback(
    outbox: DeliveryOutboxRepository,
    provider: Arc<dyn PushProvider>,
    max_concurrency: usize,
    observability: Observability,
    wakeup: Arc<Notify>,
    database_write_lock: fedi_decentralized_push_gateway_storage::DatabaseWriteLock,
    fallback_idle_poll: Duration,
) -> DeliveryWorkerHandle {
    start_delivery_worker_inner(
        outbox,
        provider,
        max_concurrency,
        observability,
        wakeup,
        database_write_lock,
        fallback_idle_poll,
    )
}

async fn sleep_optional(duration: Option<Duration>) {
    if let Some(duration) = duration {
        tokio::time::sleep(duration).await;
    } else {
        std::future::pending::<()>().await;
    }
}

async fn next_claim_delay(
    outbox: &DeliveryOutboxRepository,
    fallback_idle_poll: Duration,
) -> Option<Duration> {
    let now = crate::time::unix_timestamp();
    match outbox.next_claim_due_at().await {
        Ok(Some(next_attempt_at)) => {
            let delay_seconds = next_attempt_at.saturating_sub(now);
            let deadline_delay = Duration::from_secs(delay_seconds.try_into().unwrap_or(u64::MAX));
            Some(deadline_delay.min(fallback_idle_poll))
        }
        Ok(None) => Some(fallback_idle_poll),
        Err(err) => {
            eprintln!(
                "event=outbox_worker_error operation=next_claim_due_at error={}",
                sanitized_log_error(&err)
            );
            Some(fallback_idle_poll)
        }
    }
}

async fn deliver_provider_call<T>(
    created_at: i64,
    now: i64,
    delivery: impl Future<Output = Result<T, PushProviderError>>,
) -> Result<T, PushProviderError> {
    let deadline = created_at.saturating_add(DELIVERY_RESOLUTION_DEADLINE_SECONDS);
    let remaining_seconds = deadline.saturating_sub(now);
    let resolution_timeout =
        Duration::from_secs(u64::try_from(remaining_seconds).unwrap_or(u64::MAX));
    await_provider_delivery(PROVIDER_DELIVERY_TIMEOUT.min(resolution_timeout), delivery).await
}

async fn await_provider_delivery<T>(
    timeout: Duration,
    delivery: impl Future<Output = Result<T, PushProviderError>>,
) -> Result<T, PushProviderError> {
    tokio::time::timeout(timeout, delivery)
        .await
        .unwrap_or_else(|_| Err(PushProviderError::unavailable("provider_timeout")))
}

fn sanitized_log_error(error: &dyn std::fmt::Display) -> String {
    sanitize_log_value(&error.to_string())
}

fn outbox_failure_from_delivery_error(error: &PushProviderError) -> Option<DeliveryOutboxFailure> {
    match error.kind() {
        PushProviderErrorKind::InvalidToken => None,
        PushProviderErrorKind::InvalidPayload => {
            Some(DeliveryOutboxFailure::permanent_payload(error.reason))
        }
        PushProviderErrorKind::Unavailable => Some(DeliveryOutboxFailure::transient(error.reason)),
    }
}

#[cfg(test)]
mod tests {
    use std::{future::pending, sync::Arc, time::Duration};

    use super::*;
    use axum::{
        extract::{Path, State},
        http::HeaderMap,
    };
    use tokio::sync::{Mutex, oneshot};

    #[tokio::test(start_paused = true)]
    async fn provider_call_enforces_production_fifteen_second_cap() {
        let task = tokio::spawn(deliver_provider_call::<()>(100, 100, pending()));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(15)).await;
        tokio::task::yield_now().await;

        assert!(
            task.is_finished(),
            "provider call exceeded the 15-second cap"
        );
        assert_eq!(
            task.await.expect("provider task"),
            Err(PushProviderError::unavailable("provider_timeout"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn provider_call_uses_the_shorter_remaining_resolution_deadline() {
        let task = tokio::spawn(deliver_provider_call::<()>(100, 399, pending()));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;

        assert!(
            task.is_finished(),
            "provider call exceeded its remaining resolution deadline"
        );
        assert_eq!(
            task.await.expect("provider task"),
            Err(PushProviderError::unavailable("provider_timeout"))
        );
    }

    #[tokio::test]
    async fn idle_worker_wakes_from_notification_before_its_custom_fallback_poll() {
        let temporary_directory = tempfile::tempdir().expect("temporary directory");
        let database_url = format!(
            "sqlite://{}?mode=rwc",
            temporary_directory.path().join("push.sqlite").display()
        );
        let database = crate::Database::connect(&database_url)
            .await
            .expect("connect database");
        let (provider, mut delivered) = NotifyingProvider::new();
        let provider: Arc<dyn PushProvider> = Arc::new(provider);
        let state = crate::AppState::with_push_provider(
            crate::PushGatewayConfig::new(
                Some(crate::AppId("test-app".to_owned())),
                database_url,
                None,
            )
            .try_with_local_test_public_base_url("http://127.0.0.1:3000")
            .expect("local test public base URL"),
            database,
            provider,
        );
        let observability = state.observability();
        let recipient_id = crate::RecipientId("recipient".to_owned());
        crate::PushRegistrationRepository::new(state.database().pool().clone())
            .admit_installation(
                &recipient_id,
                &crate::RegisterInstallationRequest {
                    installation_id: crate::DeviceInstallationId("installation".to_owned()),
                    fcm_token: crate::FcmRegistrationToken("token".to_owned()),
                    platform: None,
                },
                crate::RegistrationEligibility {
                    cutoff_timestamp: 0,
                },
                crate::RegistrationAdmissionLimits {
                    max_active_per_recipient: 0,
                    max_active_global: 0,
                    max_total_rows: 0,
                    reclamation_batch_size: 100,
                },
            )
            .await
            .expect("register installation");
        let hook_token = crate::HookToken::from_path_segment("test-hook-token".to_owned());
        sqlx::query(
            "INSERT INTO notification_hooks (
                 hook_id, hook_secret_hash, recipient_id, installation_id, created_at, expires_at
             ) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind("hook")
        .bind(hook_token.hash_hex())
        .bind(&recipient_id.0)
        .bind("installation")
        .bind(0_i64)
        .bind(i64::MAX)
        .execute(state.database().pool())
        .await
        .expect("insert hook");
        let worker = state.start_delivery_worker_with_fallback(Duration::from_secs(60));

        tokio::time::timeout(Duration::from_secs(1), async {
            while observability.snapshot().outbox_idle_waits_total == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("worker entered notification wait");

        let _ = crate::invoke_hook::invoke_hook(
            State(state),
            None,
            HeaderMap::new(),
            Path(("hook".to_owned(), hook_token.as_str().to_owned())),
            crate::JsonPayload(crate::InvokeHookRequest::default()),
        )
        .await
        .expect("accept hook invocation");

        tokio::time::timeout(Duration::from_secs(5), &mut delivered)
            .await
            .expect("worker woke before the sixty-second fallback poll")
            .expect("provider delivery notification");
        worker.shutdown().await;
    }

    #[derive(Debug)]
    struct NotifyingProvider {
        delivered: Mutex<Option<oneshot::Sender<()>>>,
    }

    impl NotifyingProvider {
        fn new() -> (Self, oneshot::Receiver<()>) {
            let (sender, receiver) = oneshot::channel();
            (
                Self {
                    delivered: Mutex::new(Some(sender)),
                },
                receiver,
            )
        }
    }

    impl PushProvider for NotifyingProvider {
        fn validate_registration<'a>(
            &'a self,
            _token: &'a crate::FcmRegistrationToken,
        ) -> crate::ProviderFuture<'a> {
            Box::pin(async { Ok(()) })
        }

        fn deliver<'a>(
            &'a self,
            _registration: &'a crate::PushRegistration,
            _notification: &'a crate::Notification,
        ) -> crate::ProviderFuture<'a> {
            Box::pin(async move {
                if let Some(sender) = self.delivered.lock().await.take() {
                    let _ = sender.send(());
                }
                Ok(())
            })
        }
    }
}
