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
    let parsed = Locator::parse(&locator.to_json()).unwrap();

    assert_eq!(parsed.version, 2);
    assert_eq!(parsed.endpoint_addr, locator.endpoint_addr);
    assert_eq!(parsed.service_pubkey, service_pubkey());
}

#[test]
fn parse_rejects_formats_this_crate_does_not_speak() {
    let mut wrong_version = Locator::new(endpoint_addr(), service_pubkey());
    wrong_version.version = 1;
    assert!(matches!(
        Locator::parse(&wrong_version.to_json()),
        Err(LocatorError::UnsupportedVersion(1))
    ));

    assert!(matches!(
        Locator::parse("not json"),
        Err(LocatorError::Json(_))
    ));
}

#[test]
fn serde_rejects_old_locators_at_top_level_and_nested() {
    let mut old = serde_json::to_value(Locator::new(endpoint_addr(), service_pubkey())).unwrap();
    old["version"] = serde_json::json!(1);
    assert!(serde_json::from_value::<Locator>(old.clone()).is_err());

    #[derive(serde::Deserialize)]
    struct Stored {
        locator: Locator,
    }

    assert!(
        serde_json::from_value::<Stored>(serde_json::json!({ "locator": old }))
            .map(|stored| stored.locator)
            .is_err()
    );
}
