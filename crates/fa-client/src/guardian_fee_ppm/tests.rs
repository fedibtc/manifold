use super::*;

#[test]
fn conversion_enforces_parts_per_million_domain() {
    assert_eq!(
        GuardianFeePpm::try_from(1_000_000).unwrap().value(),
        1_000_000
    );
    assert_eq!(
        GuardianFeePpm::try_from(1_000_001).unwrap_err(),
        InvalidGuardianFeePpm
    );
}

#[test]
fn serde_preserves_numeric_representation_and_enforces_domain() {
    assert_eq!(
        serde_json::to_value(GuardianFeePpm::ZERO).unwrap(),
        serde_json::json!(0)
    );
    assert_eq!(
        serde_json::from_value::<GuardianFeePpm>(serde_json::json!(1_000_000))
            .unwrap()
            .value(),
        1_000_000
    );
    assert!(serde_json::from_value::<GuardianFeePpm>(serde_json::json!(1_000_001)).is_err());
}

#[test]
fn manifold_default_is_half_a_percent() {
    assert_eq!(GuardianFeePpm::MANIFOLD_DEFAULT.value(), 5_000);
}
