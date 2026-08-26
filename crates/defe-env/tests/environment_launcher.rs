//! Integration coverage for the Defe environment launcher.

use std::os::unix::fs::PermissionsExt as _;

#[cfg(target_os = "linux")]
#[test]
fn environment_launcher_keeps_generated_wrapper_available_for_two_commands() {
    let root = tempfile::tempdir().expect("create launcher test root");
    let bin = root.path().join("bin");
    let work = root.path().join("work");
    std::fs::create_dir(&bin).expect("create generated bin");
    std::fs::create_dir(&work).expect("create generated work directory");
    let record = root.path().join("record");
    let fman_cli = root.path().join("fman-cli");
    std::fs::write(
        &fman_cli,
        "#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$DEFE_ENV_WRAPPER_RECORD\"\n",
    )
    .expect("write fake FMan CLI");
    std::fs::set_permissions(&fman_cli, std::fs::Permissions::from_mode(0o700))
        .expect("make fake FMan CLI executable");
    let fman = bin.join("fman-1");
    std::fs::write(
        &fman,
        format!("#!/bin/sh\nexec '{}' \"$@\"\n", fman_cli.display()),
    )
    .expect("write generated FMan wrapper");
    std::fs::set_permissions(&fman, std::fs::Permissions::from_mode(0o700))
        .expect("make generated FMan wrapper executable");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_defe-env"))
        .args([
            "--internal-environment-launcher",
            "--cwd",
            work.to_str().expect("UTF-8 work path"),
            "--bin-dir",
            bin.to_str().expect("UTF-8 bin path"),
            "--env",
            "DEFE_ENV_WRAPPER_RECORD",
            record.to_str().expect("UTF-8 record path"),
            "--",
            "/bin/sh",
            "-c",
            "fman-1 seats; fman-1 plans",
        ])
        .status()
        .expect("run environment launcher");
    assert!(status.success());
    assert_eq!(
        std::fs::read_to_string(record).expect("read delegated FMan calls"),
        "seats\nplans\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn environment_composition_restores_baseline_before_applying_defe_overlay() {
    let root = tempfile::tempdir().expect("create composition test root");
    let fake_bin = root.path().join("fake-bin");
    let generated_bin = root.path().join("generated-bin");
    let invocation_cwd = root.path().join("invocation-cwd");
    std::fs::create_dir(&fake_bin).expect("create fake binary directory");
    std::fs::create_dir(&generated_bin).expect("create generated binary directory");
    std::fs::create_dir(&invocation_cwd).expect("create invocation directory");

    let direnv = fake_bin.join("direnv");
    std::fs::write(
        &direnv,
        "#!/bin/sh\n\
         [ \"$1\" = exec ] && [ \"$2\" = / ] || exit 91\n\
         shift 2\n\
         unset DEV_ONLY\n\
         BASELINE=restored\n\
         PATH=/usr/bin:/bin\n\
         DIRENV_DIR=baseline-tracking\n\
         export BASELINE PATH DIRENV_DIR\n\
         exec \"$@\"\n",
    )
    .expect("write fake direnv");
    std::fs::set_permissions(&direnv, std::fs::Permissions::from_mode(0o700))
        .expect("make fake direnv executable");

    let generated_tool = generated_bin.join("generated-tool");
    std::fs::write(&generated_tool, "#!/bin/sh\nprintf generated-tool")
        .expect("write generated tool");
    std::fs::set_permissions(&generated_tool, std::fs::Permissions::from_mode(0o700))
        .expect("make generated tool executable");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_defe-env"))
        .args([
            "--internal-environment-composition-probe",
            "--cwd",
            invocation_cwd.to_str().expect("UTF-8 invocation cwd"),
            "--bin-dir",
            generated_bin.to_str().expect("UTF-8 generated bin"),
            "--env",
            "DEFE_OVERLAY",
            "applied",
            "--",
            "/bin/sh",
            "-c",
            "printf '%s\\n' \"$BASELINE|${DEV_ONLY-unset}|$DEFE_OVERLAY|${DIRENV_DIFF-unset}|${DIRENV_DIR-unset}|${DIRENV_WATCHES-unset}|${DIRENV_FILE-unset}|${DIRENV_IN_ENVRC-unset}|$(generated-tool)|$PWD|$PATH\"",
        ])
        .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
        .env("DIRENV_DIFF", "opaque-active-state")
        .env("DIRENV_WATCHES", "active-tracking")
        .env("DIRENV_FILE", "active-file")
        .env("DIRENV_IN_ENVRC", "1")
        .env("DEV_ONLY", "from-active-environment")
        .output()
        .expect("run production environment composition");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 composition output"),
        format!(
            "restored|unset|applied|unset|unset|unset|unset|unset|generated-tool|{}|{}:/usr/bin:/bin\n",
            invocation_cwd.display(),
            generated_bin.display()
        )
    );
}

#[cfg(target_os = "linux")]
#[test]
fn active_environment_without_direnv_fails_before_launch() {
    let root = tempfile::tempdir().expect("create missing-direnv test root");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_defe-env"))
        .args([
            "--internal-environment-composition-probe",
            "--cwd",
            root.path().to_str().expect("UTF-8 test root"),
            "--bin-dir",
            root.path().to_str().expect("UTF-8 test root"),
            "--",
            "/bin/true",
        ])
        .env("PATH", "/nonexistent")
        .env("DIRENV_DIFF", "opaque-active-state")
        .output()
        .expect("run missing-direnv composition");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cannot find executable direnv"),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn environment_file_in_runtime_cwd_ancestor_is_rejected() {
    let root = tempfile::tempdir().expect("create ancestor-env test root");
    let work = root.path().join("nested/work");
    std::fs::create_dir_all(&work).expect("create nested work directory");
    std::fs::write(root.path().join(".envrc"), "export SURPRISE=1")
        .expect("write ancestor env file");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_defe-env"))
        .args([
            "--internal-environment-composition-probe",
            "--cwd",
            work.to_str().expect("UTF-8 work path"),
            "--bin-dir",
            root.path().to_str().expect("UTF-8 test root"),
            "--",
            "/bin/true",
        ])
        .output()
        .expect("run ancestor-env composition");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("contains an environment file"),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
