use super::*;
use crate::setup_payment_federations::{
    admit_setup_payment_federations_event, setup_payment_federations_event_builder,
};
use nostr::Keys;

#[test]
fn each_author_controls_only_one_snapshot_across_replacements() {
    let cold = Keys::generate();
    let hot = Keys::generate();
    let t = Timestamp::from_secs(100);
    let legacy = serde_json::from_str(r#"{"version":1,"federations":[],"telemetry_registration_url":"https://enroll.example","min_fee_ppm":3210,"fman_version":"1.0.0","guardian_link":true}"#).unwrap();
    let legacy_event = setup_payment_federations_event_builder(&legacy)
        .unwrap()
        .custom_created_at(t)
        .sign_with_keys(&cold)
        .unwrap();
    let payment =
        admit_setup_payment_federations_event(&legacy_event, cold.public_key(), t, None).unwrap();
    assert_eq!(payment.set().features().guardian_link(), Some(true));
    assert!(FeatureConfigEvent::admit(legacy_event, hot.public_key(), t, None).is_err());
    let content: FeatureConfigContent =
        serde_json::from_str(r#"{"version":1,"fman_version":"2.0.0"}"#).unwrap();
    let builder = feature_config_event_builder(&content).unwrap();
    let first = builder
        .clone()
        .custom_created_at(t)
        .sign_with_keys(&hot)
        .unwrap();
    assert!(admit_setup_payment_federations_event(&first, cold.public_key(), t, None).is_err());
    assert!(FeatureConfigEvent::admit(first.clone(), cold.public_key(), t, None).is_err());
    let admitted = FeatureConfigEvent::admit(first.clone(), hot.public_key(), t, None).unwrap();
    assert_eq!(admitted.policy.mg_version().to_string(), "2.0.0");
    assert_eq!(
        admitted.policy.guardian_link(),
        None,
        "omission is not inheritance"
    );
    let later = builder
        .clone()
        .custom_created_at(Timestamp::from_secs(101))
        .sign_with_keys(&hot)
        .unwrap();
    let later = FeatureConfigEvent::admit(later, hot.public_key(), t, Some(&admitted)).unwrap();
    let restored = FeatureConfigEvent::restore(later.event().clone(), hot.public_key()).unwrap();
    assert_eq!(
        FeatureConfigEvent::admit(first, hot.public_key(), t, Some(&restored)),
        Err(Error::Rollback)
    );
    let future = builder
        .custom_created_at(Timestamp::from_secs(90_000))
        .sign_with_keys(&hot)
        .unwrap();
    assert_eq!(
        FeatureConfigEvent::admit(future, hot.public_key(), t, None),
        Err(Error::CreatedTooFarInFuture)
    );
    assert_eq!(payment.set().min_fee_ppm(), 3210);
}

#[test]
fn unsupported_authentic_content_is_a_floor_not_a_startup_error() {
    let keys = Keys::generate();
    let event = |content: &str, time| {
        EventBuilder::new(Kind::Custom(FEATURE_CONFIG_KIND), content)
            .tag(Tag::identifier(FEATURE_CONFIG_D_TAG))
            .custom_created_at(Timestamp::from_secs(time))
            .sign_with_keys(&keys)
            .unwrap()
    };
    let valid = r#"{"version":1,"fman_version":"1.0.0"}"#;
    let now = Timestamp::from_secs(100);
    for raw in [
        " ".repeat(FEATURE_CONFIG_MAX_BYTES + 1),
        r#"{"version":1,"fman_version":"1.0.0","seat_memory_budget_mib_v2":0}"#.to_string(),
    ] {
        let floor = event(&raw, 99);
        assert!(
            FeatureConfigEvent::select(
                Some(floor.clone()),
                keys.public_key(),
                [event(valid, 98)],
                now
            )
            .unwrap()
            .is_none()
        );
        assert!(
            FeatureConfigEvent::select(Some(floor.clone()), Keys::generate().public_key(), [], now)
                .is_err()
        );
        let next = event(valid, 100);
        assert_eq!(
            FeatureConfigEvent::select(Some(floor), keys.public_key(), [next.clone()], now)
                .unwrap()
                .unwrap()
                .event(),
            &next
        );
    }
}

#[test]
fn admits_fixed_signed_feature_wire_fixture() {
    let event: Event = serde_json::from_str(include_str!("fixture.json")).unwrap();
    let snapshot =
        FeatureConfigEvent::admit(event.clone(), event.pubkey, Timestamp::from_secs(100), None)
            .unwrap()
            .into_policy();
    assert_eq!(snapshot.mg_version().to_string(), "1.2.3");
    assert_eq!(snapshot.guardian_link(), Some(true));
}
