use crate::{DeviceInstallationId, FcmRegistrationToken, Platform, RecipientId};

/// Stored push registration used by delivery providers.
#[derive(Clone, Eq, PartialEq)]
pub struct PushRegistration {
    /// User or account receiving notifications.
    pub recipient_id: RecipientId,
    /// Stable app-generated id for this device installation.
    pub installation_id: DeviceInstallationId,
    /// Firebase Cloud Messaging token for this installation.
    pub fcm_token: FcmRegistrationToken,
    /// Optional client platform label.
    pub platform: Option<Platform>,
    /// Unix timestamp when this registration was first created.
    pub created_at: i64,
    /// Unix timestamp when this installation/token was last seen.
    pub last_seen_at: i64,
    /// Optional Unix timestamp when this registration was disabled.
    pub disabled_at: Option<i64>,
    /// Sanitized reason why this registration was disabled.
    pub disabled_reason: Option<String>,
}

impl std::fmt::Debug for PushRegistration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PushRegistration")
            .field("recipient_id", &self.recipient_id)
            .field("installation_id", &self.installation_id)
            .field("fcm_token", &self.fcm_token)
            .field("platform", &self.platform)
            .field("created_at", &self.created_at)
            .field("last_seen_at", &self.last_seen_at)
            .field("disabled_at", &self.disabled_at)
            .field("disabled_reason", &self.disabled_reason)
            .finish()
    }
}
