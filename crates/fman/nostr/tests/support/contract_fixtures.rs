//! Fixed representative values for the support-chat contract fixtures
//! committed under `operator-ui/packages/types/fixtures/`.
//!
//! The support verbs are answered here, not in `fman-core`, so their fixtures
//! come from this crate's shapers. Shared, via `#[path]`, by the generator
//! binary (`src/bin/gen_support_contract_fixtures.rs`) and the drift test
//! (`tests/contract_fixtures.rs`), like the core module it mirrors
//! (`crates/fman/core/tests/support/contract_fixtures.rs`).

use fman_core::db::SupportRow;
use serde_json::Value;

fn support_message(id: char, from_fedi: bool, body: &str, created_at: u64) -> SupportRow {
    SupportRow {
        rumor_id: id.to_string().repeat(64),
        from_fedi,
        body: body.to_owned(),
        created_at,
        unread: from_fedi,
    }
}

/// Every fixture this module produces, as `(name, pretty JSON)`.
pub fn fixture_json() -> Vec<(&'static str, String)> {
    let asked = support_message(
        'a',
        false,
        "Seat 2 stopped after the update.",
        1_700_000_000,
    );
    let answered = support_message(
        'b',
        true,
        "Thanks. Does the seat log show a DKG error?",
        1_700_000_600,
    );
    [
        (
            "fman_support_chat",
            fman_nostr::support_chat_json(true, &[asked.clone(), answered], 1),
        ),
        (
            "fman_send_support_message",
            fman_nostr::support_message_json(&asked),
        ),
        ("fman_mark_support_read", fman_nostr::support_read_json(0)),
    ]
    .into_iter()
    .map(|(name, value): (_, Value)| {
        (
            name,
            serde_json::to_string_pretty(&value).expect("fixture serializes"),
        )
    })
    .collect()
}
