//! Generates the committed contract fixtures under
//! `operator-ui/packages/types/fixtures/`.
//!
//! Each fixture is a real admin-API response struct, serialized by the same
//! serde impls the daemon uses on the wire, with fixed representative values
//! (no clocks, no randomness). TypeScript tests and MSW mocks consume these
//! files so the frontend can never drift from what the Rust types actually
//! produce.
//!
//! Run via `just gen-contract-fixtures`. The paired test
//! `tests/contract_fixtures.rs` re-serializes the same fixture values (from
//! the shared `tests/support/fixtures.rs` module this binary also uses) and
//! asserts equality with the committed files, so CI fails on drift without
//! writing anything.

use std::path::PathBuf;

#[path = "../../tests/support/fixtures.rs"]
mod fixtures;

/// Directory the fixture JSON files are written to / read from, resolved
/// relative to this crate's manifest so it is independent of the invocation
/// working directory.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../operator-ui/packages/types/fixtures")
}

fn main() {
    let dir = fixtures_dir();
    std::fs::create_dir_all(&dir).expect("create fixtures dir");

    for (name, mut json) in fixtures::fixture_json() {
        let path = dir.join(format!("{name}.json"));
        json.push('\n');
        std::fs::write(&path, json).unwrap_or_else(|err| panic!("write {path:?}: {err}"));
        println!("wrote {}", path.display());
    }
}
