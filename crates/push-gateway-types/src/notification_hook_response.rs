use serde::Serialize;

use crate::Notification;

/// Response returned after accepting a notification hook.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NotificationHookResponse {
    /// Whether the gateway accepted the hook.
    pub accepted: bool,
    /// Normalized notification accepted by the gateway.
    pub notification: Notification,
}

impl NotificationHookResponse {
    /// Creates an accepted response.
    #[must_use]
    pub fn accepted(notification: Notification) -> Self {
        Self {
            accepted: true,
            notification,
        }
    }
}
