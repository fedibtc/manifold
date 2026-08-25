//! Generates the committed FMan contract fixtures under
//! `operator-ui/packages/types/fixtures/`.
//!
//! Each fixture is a real admin-API response, encoded by the same
//! `crate::admin` shaper the daemon answers with, from fixed representative
//! values (no clocks, no randomness). The request fixture is every
//! `AdminRequest` variant, serialized by the enum's own serde impl. TypeScript
//! tests and the MSW mock catalogue consume these files so the frontend cannot
//! drift from what the Rust admin surface actually produces.
//!
//! Run via `just gen-contract-fixtures`, which also regenerates the
//! liquidity-manager set. The paired test `tests/contract_fixtures.rs`
//! re-serializes the same fixture values (from the shared
//! `tests/support/contract_fixtures.rs` module this binary also uses) and
//! asserts equality with the committed files, so CI fails on drift without
//! writing anything.

use std::path::PathBuf;

#[path = "../../tests/support/contract_fixtures.rs"]
mod fixtures;

/// Directory the fixture JSON files are written to / read from, resolved
/// relative to this crate's manifest so it is independent of the invocation
/// working directory.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../operator-ui/packages/types/fixtures")
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
