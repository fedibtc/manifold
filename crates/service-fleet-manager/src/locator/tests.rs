use fedi_iroh_rpc::iroh;

use super::*;

fn endpoint_addr() -> EndpointAddr {
    EndpointAddr::new(iroh::SecretKey::from_bytes(&[7_u8; 32]).public())
}

fn service_pubkey() -> XOnlyPublicKey {
    secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[5_u8; 32])
        .unwrap()
        .x_only_public_key()
        .0
}

#[test]
fn locator_round_trips_through_json() {
    let locator = Locator::new(endpoint_addr(), service_pubkey());
    let parsed: Locator = serde_json::from_str(&locator.to_json()).unwrap();

    assert_eq!(parsed.version, ProtocolV1);
    assert_eq!(parsed.endpoint_addr, locator.endpoint_addr);
    assert_eq!(parsed.service_pubkey, service_pubkey());
}

#[test]
fn serde_rejects_invalid_and_unsupported_locator_json() {
    let mut wrong_version =
        serde_json::to_value(Locator::new(endpoint_addr(), service_pubkey())).unwrap();
    wrong_version["version"] = serde_json::json!(2);
    assert!(serde_json::from_value::<Locator>(wrong_version).is_err());
    assert!(serde_json::from_str::<Locator>("not json").is_err());
}

#[test]
fn serde_rejects_unsupported_locators_at_top_level_and_nested() {
    let mut unsupported =
        serde_json::to_value(Locator::new(endpoint_addr(), service_pubkey())).unwrap();
    unsupported["version"] = serde_json::json!(2);
    assert!(serde_json::from_value::<Locator>(unsupported.clone()).is_err());

    #[derive(serde::Deserialize)]
    struct Stored {
        locator: Locator,
    }

    assert!(
        serde_json::from_value::<Stored>(serde_json::json!({ "locator": unsupported }))
            .map(|stored| stored.locator)
            .is_err()
    );
}
