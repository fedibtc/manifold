use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{DeviceInstallationId, HookOpenBehavior, HookPrivacy, NotificationKind};

/// Request creating a shareable notification hook.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateHookRequest {
    /// Installation that alone receives notifications from this hook.
    pub installation_id: DeviceInstallationId,
    /// Optional user-visible label or caller description.
    pub label: Option<String>,
    /// Notification presentation fixed by the hook owner.
    #[serde(default)]
    pub notification: HookNotificationSettings,
    /// App-open routing fixed by the hook owner.
    #[serde(default)]
    pub open: HookOpenSettings,
    /// Extra app-defined context fixed on this hook.
    ///
    /// Reserved routing keys such as `pg.*`, `event_id`, `kind`, and
    /// `deep_link` are rejected; use the typed fields above instead.
    #[serde(default)]
    pub data: Map<String, Value>,
    /// Gateway enforcement policy fixed by the hook owner.
    #[serde(default)]
    pub policy: HookPolicySettings,
}

/// Notification presentation fields fixed at hook creation time.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookNotificationSettings {
    /// Optional app-defined notification kind fixed on this hook.
    pub kind: Option<NotificationKind>,
    /// Optional default notification title.
    pub title: Option<String>,
    /// Optional default notification body.
    pub body: Option<String>,
    /// Privacy posture for title/body handling.
    ///
    /// Defaults to `display_text` when omitted.
    pub privacy: Option<HookPrivacy>,
}

/// App-open routing fields fixed at hook creation time.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookOpenSettings {
    /// App-open behavior requested when the user taps the notification.
    ///
    /// Defaults to `open_app` when omitted.
    #[serde(rename = "behavior")]
    pub open_behavior: Option<HookOpenBehavior>,
    /// Optional app-owned workflow name opened by the mobile app.
    pub workflow: Option<String>,
    /// Optional app-owned action name within the workflow.
    pub action: Option<String>,
    /// Optional app-owned mobile deep link.
    ///
    /// Accepted values are non-empty `fedi://...` links or absolute in-app paths
    /// beginning with one `/`.
    pub deep_link: Option<String>,
}

/// Gateway enforcement policy fixed at hook creation time.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookPolicySettings {
    /// Optional TTL in seconds from creation.
    #[serde(rename = "ttl_seconds")]
    pub expires_in_seconds: Option<i64>,
    /// Optional maximum number of accepted invocations.
    pub max_uses: Option<i64>,
    /// Optional fixed-window rate limit.
    pub rate_limit: Option<HookRateLimitSettings>,
}

/// Fixed-window rate-limit policy for one hook.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookRateLimitSettings {
    /// Optional fixed-window rate-limit duration in seconds.
    ///
    /// Omitting this field uses the server default rather than disabling rate
    /// limiting.
    pub window_seconds: Option<i64>,
    /// Optional maximum accepted invocations per rate-limit window.
    ///
    /// Omitting this field uses the server default rather than disabling rate
    /// limiting.
    pub max_requests: Option<i64>,
}
