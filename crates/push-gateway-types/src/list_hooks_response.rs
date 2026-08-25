use serde::Serialize;

use crate::HookRecord;

/// Response returned when listing notification hooks.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ListHooksResponse {
    /// Hook metadata records visible to the owner.
    pub hooks: Vec<HookRecord>,
}
