use serde_json::json;

use super::*;

#[test]
fn response_has_no_payment_federation_list() {
    let response = GetAvailabilityResponse {
        accepting_seats: true,
        fedimintd_versions: vec!["1.2.3".parse().unwrap()],
        federation_sizes: vec![FederationSize(7)],
        plans: vec![Plan::InfiniteBestEffort {
            price_msats: 250000,
        }],
        additional_info: vec![],
    };

    assert_eq!(
        serde_json::to_value(response).unwrap(),
        json!({
            "accepting_seats": true,
            "fedimintd_versions": ["1.2.3"],
            "federation_sizes": [7],
            "plans": [{"InfiniteBestEffort": {"price_msats": 250_000}}],
            "additional_info": [],
        }),
    );
}
