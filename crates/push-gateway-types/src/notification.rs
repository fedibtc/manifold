use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{HookNotificationRequest, NotificationId, NotificationKind, RecipientId};

/// Normalized notification intended for webapp delivery.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Notification {
    /// User or account receiving the notification.
    pub recipient_id: RecipientId,
    /// Idempotency key for this notification.
    pub notification_id: NotificationId,
    /// Application-defined notification kind.
    pub kind: NotificationKind,
    /// Optional notification title.
    pub title: Option<String>,
    /// Optional notification body.
    pub body: Option<String>,
    /// Extra application-defined notification payload.
    pub data: Map<String, Value>,
}

impl Notification {
    pub fn from_authenticated(recipient_id: RecipientId, request: HookNotificationRequest) -> Self {
        Self {
            recipient_id,
            notification_id: request.notification_id,
            kind: request.kind,
            title: request.title,
            body: request.body,
            data: request.data,
        }
    }
}
