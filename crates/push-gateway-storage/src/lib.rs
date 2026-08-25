//! Durable database state for the push gateway.
//!
//! Production dependencies are limited to the push-gateway types crate, SQLx,
//! and serde for sanitized operator/admin read-model serialization.

mod database;
mod database_write_lock;
mod delivery_outbox;
mod hook_idempotency_repository;
mod hook_repository;
mod log_sanitizer;
mod push_registration_repository;
mod telemetry_repository;
mod time;

pub use database::{Database, DatabaseBackend};
pub use database_write_lock::{
    DEFAULT_DATABASE_WRITE_REQUEST_ADMISSION, DatabaseWriteLock, RequestDatabaseWriteGuard,
    WorkerDatabaseWriteGuard, WriteAdmissionError,
};
pub use delivery_outbox::{
    ClaimDueOutcome, ClaimedDelivery, DELIVERY_RESOLUTION_DEADLINE_SECONDS, DeliveryOutboxFailure,
    DeliveryOutboxFailureKind, DeliveryOutboxRepository, EnqueueOutcome, MarkFailedOutcome,
    OutboxAdminRow, OutboxDeadLetterReasonCount, OutboxDeadLetterSelector,
    OutboxOperationalMetrics, OutboxStatusCounts, RetentionPurgeCounts,
};
pub use hook_idempotency_repository::{
    HookIdempotencyRepository, IDEMPOTENCY_CLEANUP_MARGIN_SECONDS, MAX_HOOK_LIFETIME_SECONDS,
};
pub use hook_repository::{
    CreatedHook, DEFAULT_RATE_LIMIT_MAX_REQUESTS, DEFAULT_RATE_LIMIT_WINDOW_SECONDS,
    HookAdmissionLimits, HookAdmissionOutcome, HookRepository, HookRowMetrics, HookUseOutcome,
    MAX_RATE_LIMIT_REQUESTS_PER_WINDOW, MAX_RATE_LIMIT_WINDOW_SECONDS, hook_record_from_row,
};
pub use push_registration_repository::{
    PushRegistrationRepository, RegistrationAdmissionLimits, RegistrationAdmissionOutcome,
    RegistrationEligibility, RegistrationRowMetrics,
};
pub use telemetry_repository::{
    EncryptedTelemetryTarget, StoredTelemetryTarget, TelemetryRepository, TelemetryStorageMetrics,
};
