use serde_json::json;

use super::*;

#[test]
fn response_wire_shape() {
    let response = GetVersionResponse {
        mg_version: "0.2.0".parse().unwrap(),
        protocols: SupportedProtocols {
            guardian_transfer: vec![GUARDIAN_TRANSFER_PROTOCOL_V2],
        },
    };

    assert_eq!(
        serde_json::to_value(&response).unwrap(),
        json!({
            "mg_version": "0.2.0",
            "protocols": {"guardian_transfer": [2]},
        }),
    );
    assert!(response.supports_guardian_transfer(2));
    assert!(!response.supports_guardian_transfer(1));
}

/// A later release may list protocols this reader does not know.
#[test]
fn a_reader_ignores_protocols_it_does_not_know() {
    let response: GetVersionResponse = serde_json::from_value(json!({
        "mg_version": "0.3.0",
        "protocols": {"guardian_transfer": [2, 3], "uptime": [1]},
        "build": "abc",
    }))
    .unwrap();

    assert!(response.supports_guardian_transfer(2));
}
