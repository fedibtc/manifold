use super::PayoutRequestId;

#[test]
fn request_ids_are_bounded_opaque_non_control_strings() {
    assert_eq!(
        PayoutRequestId::parse("caller:01/attempt")
            .unwrap()
            .as_str(),
        "caller:01/attempt"
    );
    assert!(PayoutRequestId::parse("").is_err());
    assert!(PayoutRequestId::parse(&"x".repeat(129)).is_err());
    assert!(PayoutRequestId::parse("line\nbreak").is_err());
}
