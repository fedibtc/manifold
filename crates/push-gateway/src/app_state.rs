use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
use crate::delivery_worker::start_delivery_worker_with_fallback;
use crate::{
    ApiError, Database, DeliveryOutboxRepository, DeliveryWorkerHandle, FcmPushProvider,
    HookRepository, NoopPushProvider, NostrAuthReplayError, Observability, PushGatewayConfig,
    PushProvider, PushProviderConfig, PushRegistrationRepository, RegistrationEligibility,
    delivery_worker::start_delivery_worker, rate_limits::RateLimiters,
};
use fedi_decentralized_push_gateway_storage::{RequestDatabaseWriteGuard, WriteAdmissionError};

/// Shared Axum application state.
#[derive(Clone)]
pub struct AppState {
    config: PushGatewayConfig,
    database: Database,
    push_provider: Arc<dyn PushProvider>,
    delivery_worker_wakeup: Arc<tokio::sync::Notify>,
    observability: Observability,
    rate_limiters: RateLimiters,
    nostr_auth_replay_cache: crate::nostr_http_auth::NostrAuthReplayCache,
    telemetry: Option<Arc<crate::telemetry_receiver::TelemetryRuntime>>,
}

impl AppState {
    /// Returns the current signed-refresh cutoff for active registrations.
    pub(crate) fn registration_eligibility(&self) -> RegistrationEligibility {
        RegistrationEligibility {
            cutoff_timestamp: crate::time::unix_timestamp().saturating_sub(
                i64::try_from(self.config.registration_ttl_seconds()).unwrap_or(i64::MAX),
            ),
        }
    }

    /// Creates application state.
    ///
    /// # Panics
    ///
    /// Panics if the configured push provider cannot be initialized. Use
    /// [`Self::connect`] when constructing state from fallible runtime config.
    #[must_use]
    pub fn new(config: PushGatewayConfig, database: Database) -> Self {
        let push_provider = provider_from_config(&config).expect("initialize push provider");
        Self::with_push_provider(config, database, push_provider)
    }

    /// Creates application state with an explicit push provider.
    #[must_use]
    pub fn with_push_provider(
        config: PushGatewayConfig,
        database: Database,
        push_provider: Arc<dyn PushProvider>,
    ) -> Self {
        Self {
            config,
            database,
            push_provider,
            delivery_worker_wakeup: Arc::new(tokio::sync::Notify::new()),
            observability: Observability::default(),
            rate_limiters: RateLimiters::default(),
            nostr_auth_replay_cache: crate::nostr_http_auth::NostrAuthReplayCache::default(),
            telemetry: None,
        }
    }

    /// Connects to the configured database and creates application state.
    pub async fn connect(config: PushGatewayConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let database = Database::connect(config.database_url()).await?;
        purge_sensitive_terminal_data(&database, &config).await?;
        purge_expired_admission_data(&database, &config).await?;

        let push_provider = provider_from_config(&config)?;

        let telemetry = match config.telemetry() {
            Some(telemetry) => Some(Arc::new(
                crate::telemetry_receiver::TelemetryRuntime::new(telemetry, &database).await?,
            )),
            None => None,
        };
        let mut state = Self::with_push_provider(config, database, push_provider);
        state.telemetry = telemetry;
        Ok(state)
    }

    /// Returns gateway config.
    #[must_use]
    pub fn config(&self) -> &PushGatewayConfig {
        &self.config
    }

    /// Returns database handle.
    #[must_use]
    pub fn database(&self) -> &Database {
        &self.database
    }

    /// Returns the configured push delivery provider.
    #[must_use]
    pub fn push_provider(&self) -> &dyn PushProvider {
        &*self.push_provider
    }

    /// Starts the background durable delivery worker for this application state.
    pub fn start_delivery_worker(&self) -> DeliveryWorkerHandle {
        start_delivery_worker(
            DeliveryOutboxRepository::new(self.database.pool().clone(), self.database.backend()),
            self.push_provider.clone(),
            self.config.outbox_worker_concurrency(),
            self.observability.clone(),
            self.delivery_worker_wakeup.clone(),
            self.database.write_lock(),
        )
    }

    /// Wakes the delivery worker after this process commits newly due outbox rows.
    pub fn notify_delivery_worker(&self) {
        // Retain one wakeup permit if the worker is between deadline queries and
        // waiting on the notify, so a just-committed due row is not missed.
        self.delivery_worker_wakeup.notify_one();
    }

    #[cfg(test)]
    /// Starts a worker with a test-selected fallback interval.
    pub(crate) fn start_delivery_worker_with_fallback(
        &self,
        fallback_idle_poll: Duration,
    ) -> DeliveryWorkerHandle {
        start_delivery_worker_with_fallback(
            DeliveryOutboxRepository::new(self.database.pool().clone(), self.database.backend()),
            self.push_provider.clone(),
            self.config.outbox_worker_concurrency(),
            self.observability.clone(),
            self.delivery_worker_wakeup.clone(),
            self.database.write_lock(),
            fallback_idle_poll,
        )
    }

    /// Acquires the single-process lock for every request and worker database mutation.
    ///
    /// Hook creation and revocation, registration upsert, deletion and disabling,
    /// and hook invocation acceptance hold this lock. The worker also holds it for
    /// recovery, expiry, claims, and terminal/retry mutations. Provider calls must
    /// remain outside the lock so their configured concurrency is unaffected.
    pub(crate) async fn acquire_database_write_lock(
        &self,
    ) -> Result<RequestDatabaseWriteGuard, WriteAdmissionError> {
        self.database.write_lock().acquire_request().await
    }

    /// Returns shared operational counters and worker state.
    #[must_use]
    pub fn observability(&self) -> Observability {
        self.observability.clone()
    }

    #[must_use]
    pub fn rate_limiters(&self) -> RateLimiters {
        self.rate_limiters.clone()
    }

    pub(crate) async fn record_nostr_auth_event(
        &self,
        event_id: &str,
        created_at: u64,
    ) -> Result<(), NostrAuthReplayError> {
        self.nostr_auth_replay_cache
            .record_unused(event_id, created_at)
            .await
    }

    pub(crate) fn telemetry_runtime(&self) -> Option<&crate::telemetry_receiver::TelemetryRuntime> {
        self.telemetry.as_deref()
    }
}

/// Maps a bounded request-side write admission failure to a retryable response.
pub(crate) fn database_write_admission_error(_error: WriteAdmissionError) -> ApiError {
    ApiError::new(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "database_write_queue_full",
        "database write queue is full",
    )
}

async fn purge_expired_admission_data(
    database: &Database,
    config: &PushGatewayConfig,
) -> Result<(), sqlx::Error> {
    let ttl_seconds = i64::try_from(config.registration_ttl_seconds()).unwrap_or(i64::MAX);
    let now = crate::time::unix_timestamp();
    let registration_cutoff = now.saturating_sub(ttl_seconds);
    let stale_registrations = PushRegistrationRepository::new(database.pool().clone())
        .purge_stale(registration_cutoff)
        .await?;
    let terminal_hooks = HookRepository::new(database.pool().clone())
        .purge_terminal_unreferenced(now)
        .await?;
    if 0 < stale_registrations || 0 < terminal_hooks {
        eprintln!(
            "event=push_gateway_admission_gc stale_registration_rows={} terminal_hook_rows={} registration_ttl_seconds={}",
            stale_registrations,
            terminal_hooks,
            config.registration_ttl_seconds()
        );
    }
    Ok(())
}

async fn purge_sensitive_terminal_data(
    database: &Database,
    config: &PushGatewayConfig,
) -> Result<(), sqlx::Error> {
    let retention_seconds = i64::try_from(config.retention_seconds()).unwrap_or(i64::MAX);
    let now = crate::time::unix_timestamp();
    let cutoff = now.saturating_sub(retention_seconds);
    let counts = DeliveryOutboxRepository::new(database.pool().clone(), database.backend())
        .purge_retained_sensitive_data(cutoff, now)
        .await?;
    if counts != Default::default() {
        eprintln!(
            "event=push_gateway_retention_purge delivery_outbox_rows={} disabled_registration_rows={} notification_event_rows={} idempotency_tombstone_rows={} retention_seconds={}",
            counts.delivery_outbox_rows,
            counts.disabled_registration_rows,
            counts.notification_event_rows,
            counts.idempotency_tombstone_rows,
            config.retention_seconds()
        );
    }
    Ok(())
}

fn provider_from_config(
    config: &PushGatewayConfig,
) -> Result<Arc<dyn PushProvider>, Box<dyn std::error::Error>> {
    let push_provider: Arc<dyn PushProvider> = match config.provider() {
        PushProviderConfig::Noop => Arc::new(NoopPushProvider),
        PushProviderConfig::Fcm(fcm_config) => Arc::new(FcmPushProvider::new(fcm_config)?),
    };
    Ok(push_provider)
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppState")
            .field("config", &"<config>")
            .field("database", &"<database>")
            .field("push_provider", &"<push-provider>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use sqlx::Executor;

    use super::*;
    use crate::AppId;

    #[tokio::test]
    async fn connect_runs_retention_purge_before_serving_state() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_url = format!(
            "sqlite://{}?mode=rwc",
            tempdir.path().join("push.sqlite").display()
        );
        let database = Database::connect(&database_url)
            .await
            .expect("connect database");
        database
            .pool()
            .execute(
                "INSERT INTO notification_hooks (
                    hook_id, hook_secret_hash, recipient_id, open_behavior, privacy, data_json,
                    created_at, rate_limit_window_seconds, rate_limit_max_requests
                 ) VALUES ('hook', 'token-hash', 'recipient', 'open_app', 'display_text', '{}',
                    1, 3600, 2)",
            )
            .await
            .expect("insert hook");
        database
            .pool()
            .execute(
                "INSERT INTO notification_events (
                    event_id, hook_id, recipient_id, notification_json, target_count, created_at
                 ) VALUES ('event', 'hook', 'recipient', '{\"title\":\"sensitive\"}', 1, 1)",
            )
            .await
            .expect("insert event");
        database
            .pool()
            .execute(
                "INSERT INTO delivery_outbox (
                    outbox_id, event_id, recipient_id, installation_id, fcm_token, platform,
                    notification_json, status, attempts, next_attempt_at, created_at, updated_at
                 ) VALUES (
                    'outbox', 'event', 'recipient', 'installation', 'secret-token', 'android',
                    '{\"title\":\"sensitive\"}', 'succeeded', 1, 1, 1, 1
                 )",
            )
            .await
            .expect("insert outbox");
        drop(database);

        let config =
            PushGatewayConfig::new(Some(AppId("test-app".to_owned())), &database_url, None)
                .with_retention_seconds(1);
        let state = AppState::connect(config).await.expect("connect app state");

        let outbox_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delivery_outbox")
            .fetch_one(state.database().pool())
            .await
            .expect("count outbox");
        let event_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_events")
            .fetch_one(state.database().pool())
            .await
            .expect("count events");
        assert_eq!(outbox_rows, 0);
        assert_eq!(event_rows, 0);
    }

    #[tokio::test]
    async fn connect_garbage_collects_stale_registrations_and_terminal_hooks() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_url = format!(
            "sqlite://{}?mode=rwc",
            tempdir.path().join("push.sqlite").display()
        );
        let database = Database::connect(&database_url)
            .await
            .expect("connect database");
        database
            .pool()
            .execute(
                "INSERT INTO push_registrations (
                    recipient_id, installation_id, fcm_token, created_at, updated_at, last_seen_at
                 ) VALUES ('recipient', 'stale', 'stale-token', 1, 1, 1)",
            )
            .await
            .expect("insert stale registration");
        database
            .pool()
            .execute(
                "INSERT INTO notification_hooks (
                    hook_id, hook_secret_hash, recipient_id, open_behavior, privacy, data_json,
                    created_at, expires_at, rate_limit_window_seconds, rate_limit_max_requests
                 ) VALUES ('expired', 'token-hash', 'recipient', 'open_app', 'display_text', '{}',
                    1, 2, 3600, 2)",
            )
            .await
            .expect("insert expired hook");
        drop(database);

        let config =
            PushGatewayConfig::new(None, &database_url, None).with_registration_ttl_seconds(1);
        let state = AppState::connect(config).await.expect("connect app state");

        let registrations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_registrations")
            .fetch_one(state.database().pool())
            .await
            .expect("count registrations");
        let hooks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_hooks")
            .fetch_one(state.database().pool())
            .await
            .expect("count hooks");
        assert_eq!(registrations, 0);
        assert_eq!(hooks, 0);
    }
}
