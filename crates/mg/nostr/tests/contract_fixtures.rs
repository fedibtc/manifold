//! Drift check for the committed support-chat contract fixtures. If this
//! fails, run `just gen-contract-fixtures` and review the diff.

use std::path::PathBuf;

#[path = "support/contract_fixtures.rs"]
mod fixtures;

#[test]
fn committed_fixtures_match_current_response_shapes() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../operator-ui/packages/types/fixtures");
    for (name, mut expected) in fixtures::fixture_json() {
        expected.push('\n');
        let path = dir.join(format!("{name}.json"));
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("missing committed fixture {path:?} ({err})"));
        assert_eq!(
            actual, expected,
            "{path:?} is stale; run `just gen-contract-fixtures` and commit the diff"
        );
    }
}
