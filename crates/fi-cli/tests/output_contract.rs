mod test_support;

use std::process::Command;

fn fi_cli(state_dir: &std::path::Path, command: &str) -> std::process::Output {
    Command::new(test_support::fi_cli_bin())
        .args([
            "--state-dir",
            state_dir.to_str().unwrap(),
            "--json",
            command,
        ])
        .output()
        .unwrap()
}

#[test]
fn init_and_idle_status_pin_json_stdout_and_empty_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");

    let init = fi_cli(&state_dir, "init");
    assert!(init.status.success());
    assert_eq!(init.stderr, b"");
    let init_json: serde_json::Value = serde_json::from_slice(&init.stdout).unwrap();
    let object = init_json.as_object().unwrap();
    assert_eq!(
        object.keys().map(String::as_str).collect::<Vec<_>>(),
        ["fiPubkey", "state"]
    );
    assert_eq!(object["state"], "idle");
    let public_key = object["fiPubkey"].as_str().unwrap();
    assert_eq!(public_key.len(), 64);
    assert!(public_key.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(
        String::from_utf8(init.stdout).unwrap(),
        format!(r#"{{"fiPubkey":"{public_key}","state":"idle"}}"#) + "\n"
    );

    let status = fi_cli(&state_dir, "status");
    assert!(status.status.success());
    assert_eq!(status.stdout, b"\"idle\"\n");
    assert_eq!(status.stderr, b"");
}
