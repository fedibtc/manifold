use serde::{Deserialize, Serialize};

/// Firebase Cloud Messaging registration token for one app installation.
#[derive(Clone, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct FcmRegistrationToken(pub String);

impl std::fmt::Debug for FcmRegistrationToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FcmRegistrationToken(<redacted>)")
    }
}
