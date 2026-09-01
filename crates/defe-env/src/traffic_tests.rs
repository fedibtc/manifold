use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt as _;

use super::*;

fn common(command: &[&str]) -> Vec<OsString> {
    [
        "--load-test-tool",
        "/load",
        "--invite-file",
        "/invite",
        "--routes-file",
        "/routes",
    ]
    .into_iter()
    .chain(command.iter().copied())
    .map(OsString::from)
    .collect()
}

#[test]
fn parses_bounded_connection_options() {
    let (_, Traffic::Connections { users, duration }) = parse_args(&common(&[
        "connections",
        "--users",
        "25",
        "--duration-secs",
        "12",
    ]))
    .unwrap() else {
        panic!("wrong traffic mode");
    };
    assert_eq!(users, 25);
    assert_eq!(duration, Duration::from_secs(12));
}

#[test]
fn accepts_duration_maximum_and_rejects_zero_or_over_maximum() {
    assert!(parse_args(&common(&["connections", "--duration-secs", "3600"])).is_ok());
    assert!(parse_args(&common(&["connections", "--duration-secs", "0"])).is_err());
    assert!(parse_args(&common(&["connections", "--duration-secs", "3601"])).is_err());
}

#[test]
fn accepts_discoverable_unsupported_mode_options() {
    assert!(matches!(
        parse_args(&common(&["mint", "--notes-per-user", "3", "--users", "4"])),
        Ok((_, Traffic::Mint))
    ));
    assert!(matches!(
        parse_args(&common(&[
            "lightning",
            "--users",
            "4",
            "--invoices-per-user",
            "2"
        ])),
        Ok((_, Traffic::Lightning))
    ));
}

#[tokio::test]
async fn unsupported_modes_fail_without_running_a_tool() {
    let mint = run(&common(&["mint"])).await.unwrap_err().to_string();
    assert!(mint.contains("unsupported with pinned Fedimint 0.11.2"));
    assert!(mint.contains("does not cause or prove production Fedi fee accrual"));
    let lightning = run(&common(&["lightning"])).await.unwrap_err().to_string();
    assert!(lightning.contains("unsupported with pinned Fedimint 0.11.2"));
    assert!(lightning.contains("does not cause or prove production Fedi fee accrual"));
}

#[tokio::test]
async fn connection_mode_passes_invite_routes_and_user_count_to_tool() {
    let temp = tempfile::tempdir().unwrap();
    let tool = temp.path().join("load-tool");
    let record = temp.path().join("record");
    std::fs::write(
        &tool,
        "#!/bin/sh\nprintf '%s\\n' \"$FM_IROH_CONNECT_OVERRIDES\" \"$@\" >\"$(dirname \"$0\")/record\"\nsleep 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
    let invite = temp.path().join("invite");
    let routes = temp.path().join("routes");
    std::fs::write(&invite, "invite-code\n").unwrap();
    std::fs::write(&routes, "iroh-routes\n").unwrap();
    let args = [
        "--load-test-tool",
        tool.to_str().unwrap(),
        "--invite-file",
        invite.to_str().unwrap(),
        "--routes-file",
        routes.to_str().unwrap(),
        "connections",
        "--users",
        "3",
        "--duration-secs",
        "1",
    ]
    .map(OsString::from);
    run(&args).await.unwrap();
    let recorded = std::fs::read_to_string(record).unwrap();
    assert_eq!(
        recorded,
        "iroh-routes\n--users\n3\ntest-download\n--invite-code\ninvite-code\n"
    );
}

#[test]
fn rejects_unbounded_or_ambiguous_load() {
    assert!(parse_args(&common(&["connections", "--users", "0"])).is_err());
    assert!(parse_args(&common(&["connections", "--users", "1001"])).is_err());
    assert!(parse_args(&common(&["mint", "--notes-per-user", "21"])).is_err());
    assert!(parse_args(&common(&["mint", "--users"])).is_err());
    assert!(parse_args(&common(&["lightning", "--unknown", "1"])).is_err());
}
