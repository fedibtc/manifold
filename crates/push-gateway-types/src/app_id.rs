use serde::{Deserialize, Serialize};

/// Application id identifying the web/mobile app receiving notifications.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct AppId(pub String);
