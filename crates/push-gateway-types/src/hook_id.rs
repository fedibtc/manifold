use serde::{Deserialize, Serialize};

/// Public, non-secret identifier for a notification hook.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct HookId(pub String);
