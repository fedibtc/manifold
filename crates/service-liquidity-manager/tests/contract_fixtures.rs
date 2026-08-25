//! Drift check for the committed contract fixtures under
//! `operator-ui/packages/types/fixtures/`.
//!
//! Read-only: re-serializes the same fixed values the generator binary
//! (`src/bin/gen_contract_fixtures.rs`) uses — via the shared
//! `tests/support/fixtures.rs` module — and asserts they still equal the
//! committed JSON files byte-for-byte. If this fails, the committed fixtures
//! are stale; run `just gen-contract-fixtures` and review the diff, don't
//! patch this test.

use std::path::{Path, PathBuf};

#[path = "support/fixtures.rs"]
mod fixtures;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../operator-ui/packages/types/fixtures")
}

#[test]
fn committed_fixtures_match_current_response_shapes() {
    let dir = fixtures_dir();

    for (name, mut expected) in fixtures::fixture_json() {
        expected.push('\n');
        let path = dir.join(format!("{name}.json"));
        let actual = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!("missing committed fixture {path:?} ({err}); run `just gen-contract-fixtures`")
        });
        assert_eq!(
            actual, expected,
            "{path:?} is stale; run `just gen-contract-fixtures` and commit the diff"
        );
    }
}

#[test]
fn every_generated_fixture_name_is_committed() {
    let dir = fixtures_dir();
    for name in fixtures::FIXTURE_NAMES {
        let path = dir.join(format!("{name}.json"));
        assert!(
            Path::new(&path).exists(),
            "missing committed fixture {path:?}"
        );
    }
}

#[test]
fn backup_manifest_has_exactly_the_seven_real_state_groups() {
    let manifest = fixtures::backup_manifest_fixture();
    assert_eq!(
        manifest.state_groups.len(),
        7,
        "backup manifest fixture must cover exactly the 7 real BackupStateGroup variants"
    );
}
