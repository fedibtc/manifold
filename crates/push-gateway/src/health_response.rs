use serde::Serialize;

use crate::{ObservabilitySnapshot, OutboxStatusCounts};

/// Health endpoint response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HealthResponse {
    /// True when the process is alive and serving requests.
    pub ok: bool,
    /// Health/readiness check name.
    pub check: &'static str,
    /// Configured push provider mode.
    pub provider_mode: &'static str,
    /// True when database readiness checks passed.
    pub database_ready: bool,
    /// True when provider configuration was loaded for the selected mode.
    pub provider_config_ready: bool,
    /// Configured outbox worker concurrency.
    pub outbox_worker_concurrency: usize,
    /// True when this process' delivery worker is running.
    pub outbox_worker_running: bool,
    /// Number of outbox rows in each status, when the database can be queried.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outbox: Option<OutboxStatusCounts>,
}

impl HealthResponse {
    /// Creates a healthy response.
    #[must_use]
    pub fn ok(
        check: &'static str,
        provider_mode: &'static str,
        outbox_worker_concurrency: usize,
        observability: &ObservabilitySnapshot,
        outbox: Option<OutboxStatusCounts>,
    ) -> Self {
        Self {
            ok: true,
            check,
            provider_mode,
            database_ready: true,
            provider_config_ready: true,
            outbox_worker_concurrency,
            outbox_worker_running: observability.worker_running,
            outbox,
        }
    }

    /// Creates an unhealthy response.
    #[must_use]
    pub fn not_ready(
        check: &'static str,
        provider_mode: &'static str,
        database_ready: bool,
        outbox_worker_concurrency: usize,
        observability: &ObservabilitySnapshot,
        outbox: Option<OutboxStatusCounts>,
    ) -> Self {
        Self {
            ok: false,
            check,
            provider_mode,
            database_ready,
            provider_config_ready: true,
            outbox_worker_concurrency,
            outbox_worker_running: observability.worker_running,
            outbox,
        }
    }
}
