use serde_json::json;

use super::*;

#[test]
fn response_wire_shape() {
    let response = GetVersionResponse {
        mg_version: "0.2.0".parse().unwrap(),
        api_version: MgApiVersion(1),
    };

    assert_eq!(
        serde_json::to_value(&response).unwrap(),
        json!({"mg_version": "0.2.0", "api_version": 1}),
    );
}

/// A feature is there from the version that added it on.
#[test]
fn a_feature_is_supported_from_its_api_version_on() {
    let at = |api_version| GetVersionResponse {
        mg_version: "0.2.0".parse().unwrap(),
        api_version: MgApiVersion(api_version),
    };
    assert!(!at(0).supports(MgApiVersion(1)));
    assert!(at(1).supports(MgApiVersion(1)));
    assert!(at(2).supports(MgApiVersion(1)));
    assert!(!at(2).supports(MgApiVersion(3)));
}

/// A later release may add fields this reader does not know.
#[test]
fn a_reader_ignores_fields_it_does_not_know() {
    let response: GetVersionResponse = serde_json::from_value(json!({
        "mg_version": "0.3.0",
        "api_version": 4,
        "build": "abc",
    }))
    .unwrap();

    assert!(response.supports(GUARDIAN_TRANSFER_V2_API_VERSION));
}
