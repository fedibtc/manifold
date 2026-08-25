use fedi_decentralized_service_fleet_manager::DkgCompletionCallbackInput;

use super::*;

fn callback(url: &str) -> DkgCompletionCallback {
    DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: url.to_owned(),
        idempotency_key: "formation-dkg-complete".to_owned(),
    })
    .unwrap()
}

#[test]
fn origin_and_exact_hook_shape_are_required() {
    let origin =
        PushGatewayOrigin::parse("https://push.example/", PushGatewayOriginPolicy::HttpsOnly)
            .unwrap();
    assert!(
        origin
            .validate(&callback("https://push.example/hooks/hook-id/hook-secret"))
            .is_ok()
    );
    for invalid in [
        "https://other.example/hooks/hook-id/hook-secret",
        "https://push.example/hooks/hook-id/hook-secret?forward=1",
        "https://push.example/not-hooks/hook-id/hook-secret",
        "https://push.example/hooks/hook-id",
    ] {
        assert!(origin.validate(&callback(invalid)).is_err(), "{invalid}");
    }
}

#[test]
fn insecure_origin_escape_hatch_is_loopback_only() {
    assert!(
        PushGatewayOrigin::parse(
            "http://127.0.0.1:3000/",
            PushGatewayOriginPolicy::AllowInsecureLoopback
        )
        .is_ok()
    );
    assert!(
        PushGatewayOrigin::parse(
            "http://[::1]:3000/",
            PushGatewayOriginPolicy::AllowInsecureLoopback
        )
        .is_ok()
    );
    assert!(
        PushGatewayOrigin::parse(
            "http://localhost:3000/",
            PushGatewayOriginPolicy::AllowInsecureLoopback
        )
        .is_err()
    );
    assert!(
        PushGatewayOrigin::parse(
            "http://push.example/",
            PushGatewayOriginPolicy::AllowInsecureLoopback
        )
        .is_err()
    );
    assert!(
        PushGatewayOrigin::parse("http://127.0.0.1:3000/", PushGatewayOriginPolicy::HttpsOnly)
            .is_err()
    );
}

#[test]
fn callback_debug_is_redacted() {
    let callback = callback("https://push.example/hooks/hook-id/super-secret");
    let validated =
        PushGatewayOrigin::parse("https://push.example/", PushGatewayOriginPolicy::HttpsOnly)
            .unwrap()
            .validate(&callback)
            .unwrap();
    let formatted = format!("{callback:?} {validated:?}");
    assert!(!formatted.contains("super-secret"));
    assert!(!formatted.contains("formation-dkg-complete"));
}

#[test]
fn retry_delay_is_bounded_jittered_and_deterministic() {
    let base = Duration::from_secs(15);
    let first = retry_delay(base, 1, b"seat-a");
    assert!((Duration::from_secs(12)..=Duration::from_secs(18)).contains(&first));
    assert_eq!(first, retry_delay(base, 1, b"seat-a"));
    assert_ne!(first, retry_delay(base, 1, b"seat-b"));
    assert!(first <= retry_delay(base, 10, b"seat-a"));
    assert!(retry_delay(base, u32::MAX, b"seat-a") <= MAX_PUSH_CALLBACK_RETRY_INTERVAL);
}
