//! Production adapter for the FMan core's completion-callback capability.
//!
//! Core owns origin validation, durable scheduling, and sanitized outcomes. This
//! composition-layer adapter alone owns gateway DTOs and outbound HTTP/TLS.

use std::time::Duration;

use fedi_decentralized_push_gateway_types::InvokeHookRequest;
use fman_core::facts::CompletionCallbackReason;
use fman_core::push_callback::{
    CallbackAttemptOutcome, CompletionCallbackInvoker, ValidatedDkgCompletionCallback,
};
use serde_json::{Map, Value};

const CALLBACK_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const RATE_LIMIT_RESPONSE_MAX_BYTES: usize = 4 * 1024;

/// The no-proxy, no-redirect callback adapter supplied to `fman-core`.
///
/// No `Debug`: the client can carry an in-flight S6 bearer request.
pub(crate) struct PushGatewayCallbackInvoker {
    client: Option<reqwest::Client>,
}

impl PushGatewayCallbackInvoker {
    /// Build the production WebPKI-only client, recording unavailability
    /// instead of exposing dependency construction errors.
    pub(crate) fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(CALLBACK_REQUEST_TIMEOUT)
                // Cargo unifies reqwest features across bundled fedimintd.
                // Keep this bearer-bearing client on bundled WebPKI roots.
                .tls_built_in_native_certs(false)
                .tls_built_in_webpki_certs(true)
                .build()
                .ok(),
        }
    }

    #[cfg(test)]
    fn loopback() -> Self {
        Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(CALLBACK_REQUEST_TIMEOUT)
                .tls_built_in_root_certs(false)
                .build()
                .ok(),
        }
    }
}

#[async_trait::async_trait]
impl CompletionCallbackInvoker for PushGatewayCallbackInvoker {
    fn is_available(&self) -> bool {
        self.client.is_some()
    }

    async fn invoke(&self, callback: &ValidatedDkgCompletionCallback) -> CallbackAttemptOutcome {
        let Some(client) = &self.client else {
            return CallbackAttemptOutcome::Retryable(
                CompletionCallbackReason::HttpClientUnavailable,
            );
        };
        let request = InvokeHookRequest {
            idempotency_key: Some(callback.idempotency_key().to_owned()),
            data: Map::new(),
        };
        let response = match client
            .post(callback.callback_url())
            .json(&request)
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                return CallbackAttemptOutcome::Retryable(CompletionCallbackReason::Network);
            }
        };
        let status = response.status();
        let gateway_error_code = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            bounded_gateway_error_code(response).await
        } else {
            None
        };
        classify_response(status, gateway_error_code.as_deref())
    }
}

async fn bounded_gateway_error_code(mut response: reqwest::Response) -> Option<String> {
    let mut body = Vec::new();
    loop {
        let Some(chunk) = response.chunk().await.ok()? else {
            break;
        };
        if RATE_LIMIT_RESPONSE_MAX_BYTES.saturating_sub(body.len()) < chunk.len() {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice::<Value>(&body)
        .ok()?
        .pointer("/error/code")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn classify_response(
    status: reqwest::StatusCode,
    gateway_error_code: Option<&str>,
) -> CallbackAttemptOutcome {
    if status.is_success() {
        return CallbackAttemptOutcome::Delivered;
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return if gateway_error_code == Some("hook_max_uses_exceeded") {
            CallbackAttemptOutcome::Terminal(CompletionCallbackReason::MaxUsesExceeded)
        } else {
            CallbackAttemptOutcome::Retryable(CompletionCallbackReason::RateLimited)
        };
    }
    if status == reqwest::StatusCode::REQUEST_TIMEOUT || status.is_server_error() {
        return CallbackAttemptOutcome::Retryable(CompletionCallbackReason::GatewayUnavailable);
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return CallbackAttemptOutcome::Terminal(CompletionCallbackReason::HookNotFound);
    }
    if status == reqwest::StatusCode::GONE {
        return CallbackAttemptOutcome::Terminal(CompletionCallbackReason::HookExpiredOrRevoked);
    }
    CallbackAttemptOutcome::Terminal(CompletionCallbackReason::PolicyRejected)
}

#[cfg(test)]
mod tests;
