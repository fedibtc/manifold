use serde::{Deserialize, Serialize};

/// Response returned after registering a push installation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RegisterInstallationResponse {
    /// Whether the registration was accepted.
    pub registered: bool,
    /// Whether the registration was deleted.
    pub unregistered: bool,
    /// Whether the registration was disabled.
    pub disabled: bool,
}

impl RegisterInstallationResponse {
    /// Creates a registered response.
    #[must_use]
    pub fn registered() -> Self {
        Self {
            registered: true,
            unregistered: false,
            disabled: false,
        }
    }

    /// Creates an unregistered response.
    #[must_use]
    pub fn unregistered() -> Self {
        Self {
            registered: false,
            unregistered: true,
            disabled: false,
        }
    }

    /// Creates a disabled response.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            registered: false,
            unregistered: false,
            disabled: true,
        }
    }
}
