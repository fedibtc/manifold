use serde::{Deserialize, Serialize};

use crate::{DeviceInstallationId, FcmRegistrationToken, Platform};

/// Request registering one app installation for push notifications.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterInstallationRequest {
    /// Stable app-generated id for this device installation.
    pub installation_id: DeviceInstallationId,
    /// Firebase Cloud Messaging token for this installation.
    pub fcm_token: FcmRegistrationToken,
    /// Optional client platform label, such as `android` or `ios`.
    pub platform: Option<Platform>,
}
