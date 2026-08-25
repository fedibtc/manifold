use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Public hook invocation payload supplied by an external caller.
#[derive(Clone, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvokeHookRequest {
    /// Optional caller-supplied idempotency key scoped to this hook.
    ///
    /// This is not notification identity. The gateway generates internal event and
    /// notification identities separately. Apps that need an external event id can
    /// include one in non-reserved `data` under an app-owned key.
    pub idempotency_key: Option<String>,
    /// Constrained caller-supplied data payload.
    ///
    /// Reserved routing keys such as `pg.*`, `event_id`, `kind`, and
    /// `deep_link` are rejected because app-open context is owned by the hook
    /// record.
    #[serde(default)]
    pub data: Map<String, Value>,
}

impl std::fmt::Debug for InvokeHookRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InvokeHookRequest")
            .field(
                "idempotency_key",
                &self.idempotency_key.as_ref().map(|_| "<redacted>"),
            )
            .field("data", &format_args!("<{} keys>", self.data.len()))
            .finish()
    }
}
