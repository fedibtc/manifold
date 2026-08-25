use serde::{Deserialize, Serialize};

/// Idempotency key for a notification hook call.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct NotificationId(pub String);
