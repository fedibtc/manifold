use serde::{Deserialize, Serialize};

/// Privacy posture for push notification display text.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPrivacy {
    /// Allow notification title/body in the push payload.
    #[default]
    DisplayText,
    /// Suppress notification title/body and send routing/data only.
    DataOnly,
}

impl HookPrivacy {
    /// Returns the database and wire representation.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DisplayText => "display_text",
            Self::DataOnly => "data_only",
        }
    }
}

impl From<Option<String>> for HookPrivacy {
    fn from(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("data_only") => Self::DataOnly,
            _ => Self::DisplayText,
        }
    }
}
