use crate::allocation_store::ACTIVE_ITEM_STATUSES;
use crate::wallet::PENDING_SETTLEMENT_STATUSES;

/// The baseline schema, read at compile time from the same directory the
/// migrator embeds, so this test cannot drift from what actually runs.
const BASELINE_SCHEMA: &str = include_str!("../migrations/20260716000000_initial_schema.sql");

/// Each partial index restates a Rust constant as SQL literals, and SQLite
/// cannot reference the constant, so the two are genuinely separate
/// definitions and only a test can hold them together.
///
/// Adding a status to the Rust side and not the index is the direction that
/// hurts. The index silently stops covering rows in the new status, so the
/// queries that scan for active work fall back to a table scan — slower with
/// every row the deployment accumulates, and correct throughout, which is
/// exactly the kind of regression nothing else would catch.
///
/// Generating the migration from the constants would remove the duplication
/// outright, at the price of a build step in place of a reviewable static
/// schema. One test is the better trade.
#[test]
fn partial_indexes_list_exactly_the_statuses_their_rust_constants_do() {
    assert_eq!(
        index_status_predicate("idx_allocation_items_active"),
        sorted_literals(ACTIVE_ITEM_STATUSES.iter()),
        "idx_allocation_items_active must cover exactly ACTIVE_ITEM_STATUSES"
    );
    assert_eq!(
        index_status_predicate("idx_wallet_operations_active"),
        sorted_literals(PENDING_SETTLEMENT_STATUSES.iter()),
        "idx_wallet_operations_active must cover exactly PENDING_SETTLEMENT_STATUSES"
    );
}

/// A guard on the guard: if the predicate parser stops finding anything, the
/// comparison above would pass two empty lists against each other and report
/// agreement it never checked.
#[test]
fn the_predicate_parser_finds_a_non_empty_list() {
    assert!(!index_status_predicate("idx_allocation_items_active").is_empty());
    assert!(!index_status_predicate("idx_wallet_operations_active").is_empty());
}

/// Statuses as the sorted SQL literals they persist as. Sorted because `IN` is
/// a set: the two sides must agree on membership, not on order.
fn sorted_literals<T: ToString>(values: impl Iterator<Item = T>) -> Vec<String> {
    let mut literals: Vec<String> = values.map(|value| value.to_string()).collect();
    literals.sort();
    literals
}

/// The quoted values of the named index's `WHERE status IN (...)` predicate.
fn index_status_predicate(index_name: &str) -> Vec<String> {
    let declaration = BASELINE_SCHEMA
        .find(&format!("CREATE INDEX {index_name}"))
        .unwrap_or_else(|| panic!("the baseline schema declares {index_name}"));
    let statement = BASELINE_SCHEMA[declaration..]
        .split_once(';')
        .unwrap_or_else(|| panic!("{index_name} is terminated"))
        .0;
    let list = statement
        .split_once("WHERE status IN (")
        .unwrap_or_else(|| panic!("{index_name} is a partial index over status"))
        .1
        .split_once(')')
        .unwrap_or_else(|| panic!("{index_name}'s status list is closed"))
        .0;
    let mut literals: Vec<String> = list
        .split(',')
        .map(|value| value.trim().trim_matches('\'').to_owned())
        .collect();
    literals.sort();
    literals
}
