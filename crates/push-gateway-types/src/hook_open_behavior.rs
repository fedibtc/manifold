use serde::{Deserialize, Serialize};

/// Mobile app behavior requested when a notification is opened.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookOpenBehavior {
    /// Open the application without additional routing requirements.
    #[default]
    OpenApp,
    /// Open the workflow identified by the hook's `workflow` field.
    OpenWorkflow,
    /// Open the hook-owned mobile deep link.
    OpenDeepLink,
}

impl HookOpenBehavior {
    /// Returns the database and wire representation.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenApp => "open_app",
            Self::OpenWorkflow => "open_workflow",
            Self::OpenDeepLink => "open_deep_link",
        }
    }
}

impl From<Option<String>> for HookOpenBehavior {
    fn from(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("open_workflow") => Self::OpenWorkflow,
            Some("open_deep_link") => Self::OpenDeepLink,
            _ => Self::OpenApp,
        }
    }
}
