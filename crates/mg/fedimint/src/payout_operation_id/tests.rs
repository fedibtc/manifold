use super::*;

#[test]
fn accepts_only_canonical_native_operation_ids() {
    assert!(PayoutOperationId::parse(&"01".repeat(32)).is_ok());
    assert!(PayoutOperationId::parse(&"A1".repeat(32)).is_err());
    assert!(PayoutOperationId::parse(&"01".repeat(31)).is_err());
    assert!(PayoutOperationId::parse(&"gg".repeat(32)).is_err());
    assert!(
        serde_json::from_str::<PayoutOperationId>(&format!("\"{}\"", "A1".repeat(32))).is_err()
    );
}
