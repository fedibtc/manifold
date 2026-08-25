use std::path::PathBuf;

use clap::{CommandFactory, Parser, error::ErrorKind};
use fedi_decentralized_service_fleet_manager::SeatId;

use super::*;

const SEAT_1: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const SEAT_2: &str = "0000000000000000000000000000000000000000000000000000000000000002";

#[derive(Parser)]
#[command(name = "fman-cli", about = "Fleet Manager operator admin CLI")]
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
        "/tmp/fman",
        "refresh-holder-authorizations",
    ])
    .unwrap();

    assert!(matches!(args.verb, AdminVerb::RefreshHolderAuthorizations));
}

#[test]
fn telemetry_reenrollment_is_exposed_as_a_global_admin_verb() {
    let args = Args::try_parse_from(["fman-cli", "--data-dir", "/tmp/fman", "reenroll-telemetry"])
        .unwrap();

    assert!(matches!(args.verb, AdminVerb::ReenrollTelemetry));
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
            "/tmp/fman",
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
            "/tmp/fman",
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
            "/tmp/fman",
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
            "/tmp/fman",
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
