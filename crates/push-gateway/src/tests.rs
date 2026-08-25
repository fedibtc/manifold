use fedi_decentralized_push_gateway::DeadLetterMutationArgs;

use super::*;

#[test]
fn selector_requires_explicit_ids_or_bounded_limit() {
    let err = selector_from_args(&mutation_args(false, false))
        .expect_err("selector without ids or limit is rejected");
    assert!(
        err.to_string()
            .contains("requires at least one --outbox-id")
    );
}

#[test]
fn mutation_confirmation_gate_allows_dry_run_without_yes() {
    let args = mutation_args(true, false);
    ensure_confirmed_mutation(&args, "replay").expect("dry-run does not require yes");
}

#[test]
fn mutation_confirmation_gate_rejects_non_dry_run_without_yes() {
    let args = mutation_args(false, false);
    let err = ensure_confirmed_mutation(&args, "replay").expect_err("yes required");
    assert!(err.to_string().contains("without --yes"));
}

#[test]
fn mutation_confirmation_gate_allows_non_dry_run_with_yes() {
    let args = mutation_args(false, true);
    ensure_confirmed_mutation(&args, "replay").expect("yes confirms mutation");
}

#[test]
fn table_output_values_escape_control_characters() {
    let escaped = table_value("recipient\twith\ncontrols\u{1b}");
    assert!(escaped.contains("\\t"));
    assert!(escaped.contains("\\n"));
    assert!(escaped.contains("\\u001b"));
    assert!(!escaped.contains('\t'));
    assert!(!escaped.contains('\n'));
    assert!(!escaped.contains('\u{1b}'));
}

fn mutation_args(dry_run: bool, yes: bool) -> DeadLetterMutationArgs {
    DeadLetterMutationArgs {
        outbox_ids: Vec::new(),
        limit: None,
        reason: None,
        dry_run,
        yes,
        json: false,
    }
}
