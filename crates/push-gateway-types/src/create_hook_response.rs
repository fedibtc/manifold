use serde::{Deserialize, Serialize};

use crate::{HookRecord, HookToken};

/// Response returned when a new notification hook is created.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct CreateHookResponse {
    /// Hook metadata that is safe to list later.
    pub hook: HookRecord,
    /// Full bearer-capability invocation URL, returned once.
    pub invocation_url: String,
    /// Raw hook bearer secret, returned once and never stored in plaintext.
    pub hook_secret: String,
}

impl CreateHookResponse {
    /// Builds a create response from hook metadata and the generated bearer token.
    #[must_use]
    pub fn new(hook: HookRecord, hook_token: &HookToken, public_base_url: &str) -> Self {
        let path = format!("/hooks/{}/{}", hook.hook_id.0, hook_token.as_str());
        let invocation_url = if public_base_url.is_empty() {
            path
        } else {
            format!("{}{}", public_base_url.trim_end_matches('/'), path)
        };

        Self {
            hook,
            invocation_url,
            hook_secret: hook_token.as_str().to_owned(),
        }
    }
}
