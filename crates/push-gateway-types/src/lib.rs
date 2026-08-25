//! Service-local DTOs and value types for the push gateway.
//!
//! This crate also owns opaque URL-safe token/id generation helpers used by the
//! gateway's hook and durable outbox records.

mod app_id;
mod create_hook_request;
mod create_hook_response;
mod device_installation_id;
mod fcm_registration_token;
mod hook_id;
mod hook_notification_request;
mod hook_open_behavior;
mod hook_privacy;
mod hook_record;
mod hook_token;
mod invoke_hook_request;
mod invoke_hook_response;
mod list_hooks_response;
mod notification;
mod notification_hook_response;
mod notification_id;
mod notification_kind;
mod platform;
mod push_registration;
mod recipient_id;
mod register_installation_request;
mod register_installation_response;
mod registration_management_query;
mod revoke_hook_response;

pub use app_id::AppId;
pub use create_hook_request::{
    CreateHookRequest, HookNotificationSettings, HookOpenSettings, HookPolicySettings,
    HookRateLimitSettings,
};
pub use create_hook_response::CreateHookResponse;
pub use device_installation_id::DeviceInstallationId;
pub use fcm_registration_token::FcmRegistrationToken;
pub use hook_id::HookId;
pub use hook_notification_request::HookNotificationRequest;
pub use hook_open_behavior::HookOpenBehavior;
pub use hook_privacy::HookPrivacy;
pub use hook_record::{
    HookNotificationRecord, HookOpenRecord, HookPolicyRecord, HookRateLimitRecord, HookRecord,
};
pub use hook_token::{HookToken, random_url_token};
pub use invoke_hook_request::InvokeHookRequest;
pub use invoke_hook_response::InvokeHookResponse;
pub use list_hooks_response::ListHooksResponse;
pub use notification::Notification;
pub use notification_hook_response::NotificationHookResponse;
pub use notification_id::NotificationId;
pub use notification_kind::NotificationKind;
pub use platform::Platform;
pub use push_registration::PushRegistration;
pub use recipient_id::RecipientId;
pub use register_installation_request::RegisterInstallationRequest;
pub use register_installation_response::RegisterInstallationResponse;
pub use registration_management_query::RegistrationManagementQuery;
pub use revoke_hook_response::RevokeHookResponse;

#[cfg(test)]
mod tests;
