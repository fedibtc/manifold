//! HTTP push notification hook gateway.

mod app;
mod app_state;
mod delivery_worker;
mod error_response;
mod health;
mod health_response;
mod hook_management;
mod invoke_hook;
mod json_payload;
mod log_sanitizer;
mod nostr_http_auth;
mod notify;
mod observability;
mod push_gateway_config;
mod query_payload;
mod rate_limits;
mod register;
mod telemetry_collector;
mod telemetry_crypto;
mod telemetry_receiver;
mod time;
mod validation;

pub use app::{app, operator_app, public_app};
pub use app_state::AppState;
pub use delivery_worker::DeliveryWorkerHandle;
pub use error_response::{ApiError, ErrorBody, ErrorResponse};
pub use fedi_decentralized_push_gateway_provider::{
    FakeDelivery, FakePushProvider, NoopPushProvider, ProviderFuture, PushProvider,
    PushProviderError, PushProviderErrorKind,
};
pub use fedi_decentralized_push_gateway_provider_fcm::{
    FcmProviderConfig, FcmProviderError, FcmPushProvider, FirebaseCredentials,
    FirebaseCredentialsError,
};
pub use fedi_decentralized_push_gateway_storage::{
    ClaimDueOutcome, ClaimedDelivery, DEFAULT_RATE_LIMIT_MAX_REQUESTS,
    DEFAULT_RATE_LIMIT_WINDOW_SECONDS, DELIVERY_RESOLUTION_DEADLINE_SECONDS, Database,
    DatabaseBackend, DeliveryOutboxFailure, DeliveryOutboxFailureKind, DeliveryOutboxRepository,
    EnqueueOutcome, HookAdmissionLimits, HookAdmissionOutcome, HookIdempotencyRepository,
    HookRepository, HookRowMetrics, HookUseOutcome, MAX_RATE_LIMIT_REQUESTS_PER_WINDOW,
    MAX_RATE_LIMIT_WINDOW_SECONDS, MarkFailedOutcome, OutboxAdminRow, OutboxDeadLetterReasonCount,
    OutboxDeadLetterSelector, OutboxOperationalMetrics, OutboxStatusCounts,
    PushRegistrationRepository, RegistrationAdmissionLimits, RegistrationAdmissionOutcome,
    RegistrationEligibility, RegistrationRowMetrics, TelemetryRepository, TelemetryStorageMetrics,
};
pub use fedi_decentralized_push_gateway_types::{
    AppId, CreateHookRequest, CreateHookResponse, DeviceInstallationId, FcmRegistrationToken,
    HookId, HookNotificationRequest, HookOpenBehavior, HookPrivacy, HookRecord, HookToken,
    InvokeHookRequest, InvokeHookResponse, ListHooksResponse, Notification,
    NotificationHookResponse, NotificationId, NotificationKind, Platform, PushRegistration,
    RecipientId, RegisterInstallationRequest, RegisterInstallationResponse,
    RegistrationManagementQuery, RevokeHookResponse,
};
pub use health_response::HealthResponse;
pub use observability::{Observability, ObservabilitySnapshot};
pub use push_gateway_config::{
    DeadLetterMutationArgs, OperatorToken, PushGatewayArgs, PushGatewayCommand, PushGatewayConfig,
    PushGatewayConfigError, PushGatewayOutboxAction, PushGatewayOutboxCommand, PushProviderConfig,
    RateLimitConfig, TelemetryReceiverConfig,
};
pub use rate_limits::TrustedProxyCidrs;

pub(crate) use json_payload::JsonPayload;
pub(crate) use nostr_http_auth::{
    AuthenticatedEmptyBody, AuthenticatedJson, AuthenticatedRecipient, NostrAuthReplayError,
};
pub(crate) use query_payload::QueryPayload;
