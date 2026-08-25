use serde::{Deserialize, Serialize};

/// Application-defined notification kind.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct NotificationKind(pub String);
