use super::{Observability, provider_reason_class, request_id, route_label};

#[test]
fn request_id_is_gateway_owned_not_client_supplied() {
    let hook_token_shaped_value =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    for _ in 0..16 {
        assert_ne!(request_id(), hook_token_shaped_value);
    }
}

#[test]
fn route_labels_redact_bearer_paths() {
    assert_eq!(
        route_label("/hooks/hook-id/raw-secret"),
        "/hooks/{hook_id}/{hook_secret}"
    );
    assert_eq!(
        route_label("/registrations/device/disable"),
        "/registrations/{installation_id}/disable"
    );
    assert_eq!(
        route_label(
            "/v1/telemetry/fmans/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/seats/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/metrics"
        ),
        "/v1/telemetry/fmans/{fman_pubkey}/seats/{seat_id}/metrics"
    );
}

#[test]
fn provider_reason_classes_are_low_cardinality() {
    assert_eq!(provider_reason_class("provider_auth"), "auth");
    assert_eq!(provider_reason_class("provider_quota"), "quota");
    assert_eq!(provider_reason_class("provider_network"), "network");
    assert_eq!(provider_reason_class("invalid_token"), "invalid_token");
    assert_eq!(provider_reason_class("invalid_payload"), "invalid_payload");
    assert_eq!(provider_reason_class("provider_transient"), "transient");
    assert_eq!(provider_reason_class("provider_unavailable"), "transient");
}

#[test]
fn provider_reason_class_counters_are_recorded_separately() {
    let observability = Observability::default();
    for reason in [
        "provider_auth",
        "provider_quota",
        "provider_network",
        "invalid_token",
        "invalid_payload",
        "provider_unavailable",
    ] {
        observability.record_delivery_failure_reason(reason);
    }
    let snapshot = observability.snapshot();
    assert_eq!(snapshot.outbox_delivery_failure_total, 6);
    assert_eq!(snapshot.outbox_delivery_failure_auth_total, 1);
    assert_eq!(snapshot.outbox_delivery_failure_quota_total, 1);
    assert_eq!(snapshot.outbox_delivery_failure_network_total, 1);
    assert_eq!(snapshot.outbox_delivery_failure_invalid_token_total, 1);
    assert_eq!(snapshot.outbox_delivery_failure_invalid_payload_total, 1);
    assert_eq!(snapshot.outbox_delivery_failure_transient_total, 1);
}
