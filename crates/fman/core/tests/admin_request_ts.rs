//! The committed TypeScript `AdminRequest` union is what this build generates.
//!
//! Paired with `src/bin/gen_fman_admin_request_ts.rs`, which writes the file
//! this reads. Adding, renaming or reshaping a verb changes the generated
//! declaration, so leaving the committed file behind fails here — the same
//! arrangement `tests/contract_fixtures.rs` uses for the response fixtures,
//! rather than a `git diff --exit-code` step that only runs where someone
//! remembered to add it.

// The generator's own source, included so both sides compare the same bytes
// from one definition. Its `main` is not a test entry point, hence the allow.
#[allow(dead_code)]
#[path = "../src/bin/gen_fman_admin_request_ts.rs"]
mod generator;

#[test]
fn the_committed_typescript_union_matches_this_build() {
    let path = generator::output_path();
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read the committed union at {path:?}: {err}"));

    assert_eq!(
        committed,
        generator::generated(),
        "the committed TypeScript AdminRequest union is stale — run `just gen-contract-fixtures`"
    );
}

#[test]
fn the_committed_seat_readiness_types_match_this_build() {
    assert_eq!(
        std::fs::read_to_string(generator::seat_readiness_path()).unwrap(),
        generator::seat_readiness_generated(),
        "the committed TypeScript seat readiness types are stale — run `just gen-contract-fixtures`"
    );
}
