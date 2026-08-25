use super::*;
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use fedi_decentralized_service_fleet_manager::{DkgCompletionCallback, DkgCompletionCallbackInput};
use fman_core::push_callback::{PushGatewayOrigin, PushGatewayOriginPolicy};
use std::sync::{Arc, Mutex};

fn callback(url: String) -> ValidatedDkgCompletionCallback {
    PushGatewayOrigin::parse(
        url.split("/hooks/").next().expect("origin"),
        PushGatewayOriginPolicy::AllowInsecureLoopback,
    )
    .unwrap()
    .validate(
        &DkgCompletionCallback::new(DkgCompletionCallbackInput {
            callback_url: url,
            idempotency_key: "formation-dkg-complete".to_owned(),
        })
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn client_construction_failure_is_retained_without_panicking() {
    let invalid_user_agent = reqwest::Client::builder().user_agent("invalid\nuser-agent");
    assert!(invalid_user_agent.build().is_err());
}

#[test]
fn production_client_ignores_invalid_native_root_discovery() {
    if std::env::var_os("FMAN_CALLBACK_TLS_PROBE").is_some() {
        assert!(PushGatewayCallbackInvoker::new().is_available());
        return;
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("push_callback::tests::production_client_ignores_invalid_native_root_discovery")
        .arg("--nocapture")
        .env("FMAN_CALLBACK_TLS_PROBE", "1")
        .env("SSL_CERT_FILE", "/does/not/exist")
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn gateway_statuses_have_explicit_retry_and_terminal_classes() {
    assert_eq!(
        classify_response(StatusCode::OK, None),
        CallbackAttemptOutcome::Delivered
    );
    assert_eq!(
        classify_response(StatusCode::REQUEST_TIMEOUT, None),
        CallbackAttemptOutcome::Retryable(CompletionCallbackReason::GatewayUnavailable)
    );
    assert_eq!(
        classify_response(StatusCode::SERVICE_UNAVAILABLE, None),
        CallbackAttemptOutcome::Retryable(CompletionCallbackReason::GatewayUnavailable)
    );
    assert_eq!(
        classify_response(StatusCode::TOO_MANY_REQUESTS, None),
        CallbackAttemptOutcome::Retryable(CompletionCallbackReason::RateLimited)
    );
    assert_eq!(
        classify_response(
            StatusCode::TOO_MANY_REQUESTS,
            Some("hook_max_uses_exceeded")
        ),
        CallbackAttemptOutcome::Terminal(CompletionCallbackReason::MaxUsesExceeded)
    );
    assert_eq!(
        classify_response(StatusCode::NOT_FOUND, None),
        CallbackAttemptOutcome::Terminal(CompletionCallbackReason::HookNotFound)
    );
    assert_eq!(
        classify_response(StatusCode::GONE, None),
        CallbackAttemptOutcome::Terminal(CompletionCallbackReason::HookExpiredOrRevoked)
    );
    assert_eq!(
        classify_response(StatusCode::BAD_REQUEST, None),
        CallbackAttemptOutcome::Terminal(CompletionCallbackReason::PolicyRejected)
    );
}

#[tokio::test]
async fn request_has_exact_shape_and_response_body_is_bounded() {
    #[derive(Clone, Default)]
    struct Seen(Arc<Mutex<Option<InvokeHookRequest>>>);
    async fn receive(
        State(seen): State<Seen>,
        Json(request): Json<InvokeHookRequest>,
    ) -> (StatusCode, Vec<u8>) {
        *seen.0.lock().unwrap() = Some(request);
        (
            StatusCode::TOO_MANY_REQUESTS,
            vec![b'x'; RATE_LIMIT_RESPONSE_MAX_BYTES + 1],
        )
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Seen::default();
    let server_seen = seen.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/hooks/{hook_id}/{hook_secret}", post(receive))
                .with_state(server_seen),
        )
        .await
        .unwrap();
    });
    let callback = callback(format!("http://{address}/hooks/id/bearer"));
    assert_eq!(
        PushGatewayCallbackInvoker::loopback()
            .invoke(&callback)
            .await,
        CallbackAttemptOutcome::Retryable(CompletionCallbackReason::RateLimited)
    );
    let request = seen.0.lock().unwrap().clone().unwrap();
    assert_eq!(
        request.idempotency_key.as_deref(),
        Some("formation-dkg-complete")
    );
    assert!(request.data.is_empty());
    server.abort();
}
