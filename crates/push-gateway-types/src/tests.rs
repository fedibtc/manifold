use serde_json::json;

use crate::{
    CreateHookRequest, CreateHookResponse, InvokeHookRequest, RegisterInstallationRequest,
    RegisterInstallationResponse,
};

#[test]
fn management_dtos_are_bidirectional_without_exposing_bearers() {
    let registration_json = json!({
        "installation_id": "device-1",
        "fcm_token": "fcm-secret-token",
        "platform": "android"
    });
    let registration: RegisterInstallationRequest =
        serde_json::from_value(registration_json.clone()).expect("registration request");
    assert_eq!(
        serde_json::to_value(&registration).expect("serialize registration request"),
        registration_json
    );
    assert!(!format!("{registration:?}").contains("fcm-secret-token"));

    let registration_response_json = json!({
        "registered": true,
        "unregistered": false,
        "disabled": false
    });
    let registration_response: RegisterInstallationResponse =
        serde_json::from_value(registration_response_json.clone()).expect("registration response");
    assert_eq!(
        serde_json::to_value(registration_response).expect("serialize registration response"),
        registration_response_json
    );

    let create_json = json!({
        "installation_id": "device-1",
        "label": "formation completion",
        "notification": {"kind": "federation.setup", "privacy": "display_text"},
        "open": {"behavior": "open_workflow", "workflow": "federation_setup"},
        "data": {},
        "policy": {"ttl_seconds": 2_592_000, "max_uses": 20}
    });
    let create: CreateHookRequest =
        serde_json::from_value(create_json.clone()).expect("create request");
    let serialized_create = serde_json::to_value(&create).expect("serialize create request");
    assert_eq!(
        serde_json::from_value::<CreateHookRequest>(serialized_create)
            .expect("deserialize serialized create request"),
        create
    );

    let response_json = json!({
        "hook": {
            "hook_id": "hook-1",
            "recipient_id": "recipient-1",
            "installation_id": "device-1",
            "label": null,
            "notification": {"kind": null, "title": null, "body": null, "privacy": "display_text"},
            "open": {"workflow": null, "action": null, "deep_link": null, "behavior": "open_app"},
            "data": {},
            "policy": {"expires_at": 123, "max_uses": null, "rate_limit": null},
            "created_at": 1,
            "revoked_at": null,
            "use_count": 0,
            "last_used_at": null
        },
        "invocation_url": "https://push.example/hooks/hook-1/bearer-secret",
        "hook_secret": "bearer-secret"
    });
    let response: CreateHookResponse =
        serde_json::from_value(response_json.clone()).expect("create response");
    assert_eq!(
        serde_json::to_value(response).expect("serialize create response"),
        response_json
    );
}

#[test]
fn invoke_request_debug_redacts_idempotency_and_payload_values() {
    let request_json = json!({
        "idempotency_key": "stable-formation-key",
        "data": {"reference": "private-reference"}
    });
    let request: InvokeHookRequest =
        serde_json::from_value(request_json.clone()).expect("invoke request");

    assert_eq!(
        serde_json::to_value(&request).expect("serialize invoke request"),
        request_json
    );

    let debug = format!("{request:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("stable-formation-key"));
    assert!(!debug.contains("private-reference"));
}
