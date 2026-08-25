#![cfg(unix)]

mod test_support;

use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::process::Command;
use std::time::{Duration, Instant};

fn secret_file(directory: &tempfile::TempDir, name: &str, contents: &[u8]) -> std::path::PathBuf {
    let path = directory.path().join(name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    std::io::Write::write_all(
        &mut options.open(&path).expect("create secret file"),
        contents,
    )
    .expect("write secret file");
    path
}

fn run(path: &std::path::Path) -> std::process::Output {
    Command::new(test_support::fi_cli_bin())
        .args(["resume", "--wallet-secret-file"])
        .arg(path)
        .output()
        .expect("run fi-cli")
}

#[test]
fn wallet_secret_contents_never_enter_output_errors() {
    const SENTINEL: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1\
                            a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
    let directory = tempfile::tempdir().expect("temporary directory");
    let output = run(&secret_file(
        &directory,
        "wallet-secret",
        SENTINEL.as_bytes(),
    ));
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SENTINEL));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(SENTINEL));
}

#[test]
fn explicit_file_wins_over_environment_fallback() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let explicit = secret_file(&directory, "explicit", &[b'z'; 128]);
    let fallback = secret_file(&directory, "fallback", &[0xff]);
    let output = Command::new(test_support::fi_cli_bin())
        .args(["resume", "--wallet-secret-file"])
        .arg(&explicit)
        .env("FI_CLI_WALLET_SECRET_FILE", fallback)
        .output()
        .expect("run fi-cli");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not valid hexadecimal"));
    assert!(!stderr.contains("not valid UTF-8"));
}

#[test]
fn environment_file_path_is_used_as_fallback() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = secret_file(&directory, "fallback", b"invalid-secret-sentinel");
    let output = Command::new(test_support::fi_cli_bin())
        .args(["resume"])
        .env("FI_CLI_WALLET_SECRET_FILE", path)
        .output()
        .expect("run fi-cli");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("must encode exactly 64 bytes"));
    assert!(!stderr.contains("invalid-secret-sentinel"));
}

#[test]
fn wallet_secret_file_security_and_bounds_are_enforced() {
    let directory = tempfile::tempdir().expect("temporary directory");

    let permissive = secret_file(&directory, "permissive", &[]);
    std::fs::set_permissions(&permissive, std::fs::Permissions::from_mode(0o640))
        .expect("change permissions");
    assert!(String::from_utf8_lossy(&run(&permissive).stderr).contains("exactly 0600"));

    let target = secret_file(&directory, "target", &[]);
    let link = directory.path().join("link");
    std::os::unix::fs::symlink(target, &link).expect("create symlink");
    assert!(
        String::from_utf8_lossy(&run(&link).stderr)
            .contains("could not open wallet root secret file")
    );

    let fifo = directory.path().join("fifo");
    let fifo_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes())
        .expect("FIFO path contains no NUL");
    // SAFETY: `fifo_path` is a valid NUL-terminated pathname.
    assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
    let mut child = Command::new(test_support::fi_cli_bin())
        .args(["resume", "--wallet-secret-file"])
        .arg(&fifo)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run fi-cli");
    let deadline = Instant::now() + Duration::from_secs(2);
    while child.try_wait().expect("poll fi-cli").is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        child.try_wait().expect("poll fi-cli").is_some(),
        "FIFO blocked"
    );
    let output = child.wait_with_output().expect("collect fi-cli output");
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a regular file"));

    let oversized = secret_file(&directory, "oversized", &[b'0'; 131]);
    assert!(String::from_utf8_lossy(&run(&oversized).stderr).contains("input is too long"));
}

#[test]
fn environment_fallback_does_not_affect_init_or_status() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let missing = directory.path().join("missing-secret");
    for subcommand in ["init", "status"] {
        let output = Command::new(test_support::fi_cli_bin())
            .arg("--state-dir")
            .arg(directory.path().join(subcommand))
            .arg(subcommand)
            .env("FI_CLI_WALLET_SECRET_FILE", &missing)
            .output()
            .expect("run fi-cli");
        assert!(
            !String::from_utf8_lossy(&output.stderr)
                .contains("could not open wallet root secret file")
        );
    }
}

#[test]
fn valid_secret_files_accept_plain_lf_and_crlf_endings() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let valid = "01".repeat(64);
    for (name, ending) in [("plain", ""), ("lf", "\n"), ("crlf", "\r\n")] {
        let path = secret_file(&directory, name, format!("{valid}{ending}").as_bytes());
        let stderr = String::from_utf8_lossy(&run(&path).stderr).into_owned();
        assert!(!stderr.contains("must encode exactly 64 bytes"));
        assert!(!stderr.contains(&valid));
    }
}

#[test]
fn retired_secret_value_and_stdin_options_are_rejected_without_echoing_values() {
    const SENTINEL: &str = "wallet-secret-sentinel";
    for arguments in [
        vec!["resume", "--wallet-secret-hex", SENTINEL],
        vec!["resume", "--wallet-secret-stdin"],
    ] {
        let output = Command::new(test_support::fi_cli_bin())
            .args(arguments)
            .output()
            .expect("run fi-cli");
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(SENTINEL));
    }
}
