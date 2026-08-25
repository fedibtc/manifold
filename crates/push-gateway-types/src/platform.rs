use serde::{Deserialize, Serialize};

/// Client platform label, such as `android` or `ios`.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Platform(pub String);
