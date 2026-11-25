use std::path::PathBuf;

use clap::{CommandFactory, Parser, error::ErrorKind};
use fedi_decentralized_service_manifold_guardian::SeatId;

use super::*;

const SEAT_1: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const SEAT_2: &str = "0000000000000000000000000000000000000000000000000000000000000002";

#[derive(Parser)]
#[command(name = "fman-cli", about = "Manifold Guardian operator admin CLI")]
struct Args {
    /// The daemon's data dir (the admin socket lives inside it).
    #[arg(long)]
    data_dir: PathBuf,
    #[command(subcommand)]
    verb: AdminVerb,
}

#[test]
fn holder_authorization_refresh_is_exposed_as_an_admin_verb() {
    let args = Args::try_parse_from([
        "fman-cli",
        "--data-dir",
        "/tmp/mg",
        "refresh-holder-authorizations",
    ])
    .unwrap();

    assert!(matches!(args.verb, AdminVerb::RefreshHolderAuthorizations));
}

#[test]
fn telemetry_reenrollment_is_exposed_as_a_global_admin_verb() {
    let args =
        Args::try_parse_from(["fman-cli", "--data-dir", "/tmp/mg", "reenroll-telemetry"]).unwrap();

    assert!(matches!(args.verb, AdminVerb::ReenrollTelemetry));
}

#[test]
fn sweeps_default_to_lightning_and_take_ecash_by_method() {
    let method = |arguments: &[&str]| match guardian_fee_verb(arguments) {
        GuardianFeesVerb::Sweep { method, .. } => method,
        other => panic!("expected a sweep, got {other:?}"),
    };
    let sweep = [
        "fman-cli",
        "--data-dir",
        "/tmp/mg",
        "guardian-fees",
        "sweep",
        SEAT_1,
        "--request-id",
        "r",
    ];
    assert_eq!(method(&sweep), mg_core::wallet::PayoutRail::Lightning);
    assert_eq!(
        method(&[&sweep[..], &["--method", "ecash"]].concat()),
        mg_core::wallet::PayoutRail::Ecash
    );
    assert!(Args::try_parse_from([&sweep[..], &["--method", "cash"]].concat()).is_err());

    let args = Args::try_parse_from([
        "fman-cli",
        "--data-dir",
        "/tmp/mg",
        "payment-federations",
        "sweep",
        "fed1",
        "--request-id",
        "r",
        "--method",
        "ecash",
    ])
    .unwrap();
    assert!(matches!(
        args.verb,
        AdminVerb::PaymentFederations {
            verb: PaymentFederationsVerb::Sweep {
                method: mg_core::wallet::PayoutRail::Ecash,
                ..
            }
        }
    ));
}

fn guardian_fee_verb(arguments: &[&str]) -> GuardianFeesVerb {
    let args = Args::try_parse_from(arguments).expect("parse guardian-fee command");
    let AdminVerb::GuardianFees { verb } = args.verb else {
        panic!("parse guardian-fee verb");
    };
    verb
}

fn guardian_fee_verb_seat_id(verb: GuardianFeesVerb) -> SeatId {
    match verb {
        GuardianFeesVerb::Show { seat_id, .. }
        | GuardianFeesVerb::Collect { seat_id }
        | GuardianFeesVerb::Sweep { seat_id, .. } => seat_id.into_seat_id(),
    }
}

#[test]
fn guardian_fee_commands_accept_positional_and_flag_seat_ids() {
    for command in ["show", "collect", "sweep"] {
        let mut positional_arguments = vec![
            "fman-cli",
            "--data-dir",
            "/tmp/mg",
            "guardian-fees",
            command,
            SEAT_1,
        ];
        if command == "sweep" {
            positional_arguments.extend(["--request-id", "test-request"]);
        }
        let positional = guardian_fee_verb(&positional_arguments);
        assert_eq!(guardian_fee_verb_seat_id(positional).to_string(), SEAT_1);

        let mut flag_arguments = vec![
            "fman-cli",
            "--data-dir",
            "/tmp/mg",
            "guardian-fees",
            command,
            "--seat-id",
            SEAT_2,
        ];
        if command == "sweep" {
            flag_arguments.extend(["--request-id", "test-request"]);
        }
        let flag = guardian_fee_verb(&flag_arguments);
        assert_eq!(guardian_fee_verb_seat_id(flag).to_string(), SEAT_2);
    }
}

#[test]
fn guardian_fee_commands_reject_both_seat_id_forms() {
    for command in ["show", "collect", "sweep"] {
        let mut arguments = vec![
            "fman-cli",
            "--data-dir",
            "/tmp/mg",
            "guardian-fees",
            command,
            SEAT_1,
            "--seat-id",
            SEAT_2,
        ];
        if command == "sweep" {
            arguments.extend(["--request-id", "test-request"]);
        }
        let result = Args::try_parse_from(arguments);
        let Err(error) = result else {
            panic!("positional and flag seat IDs conflict");
        };
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }
}

#[test]
fn guardian_fee_commands_require_a_seat_id() {
    for command in ["show", "collect", "sweep"] {
        let mut arguments = vec![
            "fman-cli",
            "--data-dir",
            "/tmp/mg",
            "guardian-fees",
            command,
        ];
        if command == "sweep" {
            arguments.extend(["--request-id", "test-request"]);
        }
        let result = Args::try_parse_from(arguments);
        let Err(error) = result else {
            panic!("guardian-fee command requires a seat ID");
        };
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }
}

#[test]
fn guardian_fee_help_presents_one_seat_id_choice() {
    let mut command = Args::command();
    let guardian_fees = command
        .find_subcommand_mut("guardian-fees")
        .expect("guardian-fees subcommand");
    let show = guardian_fees
        .find_subcommand_mut("show")
        .expect("show subcommand");
    let help = show.render_long_help().to_string();

    assert!(help.contains("Usage: show [OPTIONS] <SEAT_ID|--seat-id <SEAT_ID>>"));
    assert!(help.contains("--seat-id <SEAT_ID>"));
}

#[test]
fn a_data_budget_below_one_seat_warns_without_refusing() {
    let capacity = |budget: &str, seats: Option<u32>| {
        serde_json::json!({
            "max_seats": null,
            "monthly_data_budget": budget,
            "seat_monthly_data_gb": 65,
            "data_max_seats": seats,
            "available_slots": 0,
        })
    };

    assert_eq!(
        data_budget_warning(&capacity("50", Some(0))).as_deref(),
        Some(
            "50 GB is less than one seat (about 65 GB a month). \
             This guardian will not sell new seats."
        )
    );
    // 65 GB covers exactly one seat, and a plan without a cap covers all.
    assert_eq!(data_budget_warning(&capacity("65", Some(1))), None);
    assert_eq!(data_budget_warning(&capacity("unlimited", None)), None);
    // Responses without capacity, such as a price change, say nothing.
    assert_eq!(data_budget_warning(&serde_json::json!({"plans": []})), None);
}
