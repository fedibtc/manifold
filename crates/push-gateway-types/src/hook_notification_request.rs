use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{NotificationId, NotificationKind};

/// Incoming generic HTTP hook notification request.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HookNotificationRequest {
    /// Idempotency key for this notification.
    pub notification_id: NotificationId,
    /// Application-defined notification kind.
    pub kind: NotificationKind,
    /// Optional notification title.
    pub title: Option<String>,
    /// Optional notification body.
    pub body: Option<String>,
    /// Extra application-defined notification payload.
    #[serde(default)]
    pub data: Map<String, Value>,
}
