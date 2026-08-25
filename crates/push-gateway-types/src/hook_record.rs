use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    DeviceInstallationId, HookId, HookOpenBehavior, HookPrivacy, NotificationKind, RecipientId,
};

/// Stored metadata for a notification hook, excluding the raw secret.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HookRecord {
    /// Public, non-secret hook identifier.
    pub hook_id: HookId,
    /// User or account that owns this hook.
    pub recipient_id: RecipientId,
    /// Installation that alone receives notifications from this hook.
    pub installation_id: DeviceInstallationId,
    /// Optional user-visible label or caller description.
    pub label: Option<String>,
    /// Notification presentation fixed by the hook owner.
    pub notification: HookNotificationRecord,
    /// App-open routing fixed by the hook owner.
    pub open: HookOpenRecord,
    /// Extra app-defined context fixed on this hook.
    pub data: Map<String, Value>,
    /// Gateway enforcement policy and counters.
    pub policy: HookPolicyRecord,
    /// Unix timestamp when this hook was created.
    pub created_at: i64,
    /// Optional Unix timestamp when this hook was revoked.
    pub revoked_at: Option<i64>,
    /// Number of accepted invocations.
    pub use_count: i64,
    /// Optional Unix timestamp of the last accepted invocation.
    pub last_used_at: Option<i64>,
}

/// Notification presentation fields for a stored hook.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HookNotificationRecord {
    /// Optional app-defined notification kind fixed on this hook.
    pub kind: Option<NotificationKind>,
    /// Optional default notification title.
    pub title: Option<String>,
    /// Optional default notification body.
    pub body: Option<String>,
    /// Privacy posture for title/body handling.
    pub privacy: HookPrivacy,
}

/// App-open routing fields for a stored hook.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HookOpenRecord {
    /// App-open behavior requested when the user taps the notification.
    #[serde(rename = "behavior")]
    pub open_behavior: HookOpenBehavior,
    /// Optional app-owned workflow name opened by the mobile app.
    pub workflow: Option<String>,
    /// Optional app-owned action name within the workflow.
    pub action: Option<String>,
    /// Optional app-owned mobile deep link.
    pub deep_link: Option<String>,
}

/// Gateway enforcement policy and mutable rate-limit state for a stored hook.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HookPolicyRecord {
    /// Optional Unix timestamp after which this hook is rejected.
    pub expires_at: Option<i64>,
    /// Optional maximum accepted invocation count.
    pub max_uses: Option<i64>,
    /// Fixed-window rate-limit policy and state.
    pub rate_limit: Option<HookRateLimitRecord>,
}

/// Fixed-window rate-limit policy and state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HookRateLimitRecord {
    /// Fixed-window rate limit duration in seconds.
    pub window_seconds: i64,
    /// Maximum accepted invocations per fixed rate-limit window.
    pub max_requests: i64,
    /// Unix timestamp when the current rate-limit window started.
    pub window_started_at: Option<i64>,
    /// Number of accepted invocations in the current rate-limit window.
    pub count: i64,
}
