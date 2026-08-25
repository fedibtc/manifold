//! Drift check for the committed FMan contract fixtures under
//! `operator-ui/packages/types/fixtures/`.
//!
//! Read-only: re-serializes the same fixed values the generator binary
//! (`src/bin/gen_fman_contract_fixtures.rs`) uses — via the shared
//! `tests/support/contract_fixtures.rs` module — and asserts they still equal
//! the committed JSON files byte-for-byte. If this fails, the committed
//! fixtures are stale; run `just gen-contract-fixtures` and review the diff,
//! don't patch this test.
//!
//! Note that request-variant coverage is not asserted here. It is enforced by
//! the compiler: the walk behind `fixtures::request_fixtures` is a `match` over
//! `AdminRequest`, so a new verb breaks the build of this test binary until it
//! has a fixture.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[path = "support/contract_fixtures.rs"]
mod fixtures;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../operator-ui/packages/types/fixtures")
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
fn the_generated_name_list_matches_what_is_generated() {
    let generated: BTreeSet<&str> = fixtures::fixture_json()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let declared: BTreeSet<&str> = fixtures::FIXTURE_NAMES.iter().copied().collect();
    assert_eq!(
        generated, declared,
        "FIXTURE_NAMES and fixture_json() disagree; a fixture would be written but never checked"
    );
}

#[test]
fn the_request_inventory_covers_every_admin_request_variant_once() {
    let requests = fixtures::request_fixtures();
    let names: BTreeSet<&str> = requests.iter().map(fixtures::request_name).collect();
    assert_eq!(
        names.len(),
        requests.len(),
        "the AdminRequest walk covers a variant twice: {names:?}"
    );

    // The walk is what the compiler enforces; this pins the count so a reviewer
    // sees the inventory grow in the diff when a verb is added.
    assert_eq!(
        requests.len(),
        23,
        "AdminRequest gained or lost a verb — update this count and regenerate the fixtures"
    );
}
