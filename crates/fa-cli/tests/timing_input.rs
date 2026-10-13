mod test_support;

use std::process::Command;

#[test]
fn invalid_timing_precedes_cli_state_and_runtime_resources() {
    for (argument, value) in [
        ("--poll-interval-secs", "0"),
        ("--poll-timeout-secs", "0"),
        ("--poll-interval-secs", "2147484"),
        ("--poll-timeout-secs", "2147484"),
    ] {
        let directory = tempfile::tempdir().expect("temporary parent");
        let state_dir = directory.path().join("must-not-exist");
        let wallet_secret = directory.path().join("must-not-read-wallet-secret");
        let fa_account = directory.path().join("must-not-read-fi-account");
        let setup_event = directory.path().join("must-not-read-setup-event");
        let output = Command::new(test_support::fa_cli_bin())
            .arg("--state-dir")
            .arg(&state_dir)
            .args(["--setup-payment-publisher", "invalid"])
            .arg("--setup-payment-event-file")
            .arg(&setup_event)
            .args(["create", "--fa-spv2-account-file"])
            .arg(&fa_account)
            .args([
                "--locator",
                "{}",
                "--federation-size",
                "7",
                "--fedimintd-version-minimum",
                "0.11.1",
                "--fedimintd-version-maximum-exclusive",
                "0.11.2",
                "--wallet-secret-file",
            ])
            .arg(&wallet_secret)
            .args([argument, value])
            .output()
            .expect("run fa-cli");

        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("invalid formation options"),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !state_dir.exists(),
            "invalid timing must not create CLI identity or database state"
        );
    }
}
