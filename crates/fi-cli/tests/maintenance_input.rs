mod test_support;

use std::process::Command;

enum InvalidInput {
    Metadata,
    Fee,
}

#[test]
fn invalid_maintenance_input_precedes_cli_state_and_runtime_resources() {
    for invalid_input in [InvalidInput::Metadata, InvalidInput::Fee] {
        let directory = tempfile::tempdir().expect("temporary parent");
        let state_dir = directory.path().join("must-not-exist");
        let setup_event = directory.path().join("must-not-read-setup-event");
        let mut command = Command::new(test_support::fi_cli_bin());
        command
            .arg("--state-dir")
            .arg(&state_dir)
            .args(["--setup-payment-publisher", "invalid"])
            .arg("--setup-payment-event-file")
            .arg(&setup_event);

        let expected_error = match invalid_input {
            InvalidInput::Metadata => {
                command.args(["maintenance", "set-name", "--value", "no"]);
                "validate federation metadata name"
            }
            InvalidInput::Fee => {
                command
                    .args(["maintenance", "configure-guardian-fees"])
                    .args(["--send-ppm", "210001"]);
                "--send-ppm must not exceed 210000"
            }
        };

        let output = command.output().expect("run fi-cli");

        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected_error),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !state_dir.exists(),
            "invalid maintenance input must not create CLI identity or database state"
        );
    }
}
