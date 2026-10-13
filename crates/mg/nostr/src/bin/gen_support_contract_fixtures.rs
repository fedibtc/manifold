//! Generates the committed support-chat contract fixtures under
//! `operator-ui/packages/types/fixtures/`. Run via
//! `just gen-contract-fixtures`; `tests/contract_fixtures.rs` fails on drift.

use std::path::PathBuf;

#[path = "../../tests/support/contract_fixtures.rs"]
mod fixtures;

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../operator-ui/packages/types/fixtures");
    for (name, mut json) in fixtures::fixture_json() {
        let path = dir.join(format!("{name}.json"));
        json.push('\n');
        std::fs::write(&path, json).unwrap_or_else(|err| panic!("write {path:?}: {err}"));
        println!("wrote {}", path.display());
    }
}
