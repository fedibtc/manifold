//! Tests for shared domain wrappers.

use super::*;

#[test]
fn domain_id_roundtrips() {
    let id = DomainId::new("example");

    assert_eq!(id.as_str(), "example");
}
