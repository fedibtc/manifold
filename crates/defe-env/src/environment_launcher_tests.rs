use super::{
    Direnv, LAUNCHER_ARGUMENT, Launcher, build_with_executable, ensure_neutral_cwd,
    prepare_with_context,
};
use std::ffi::OsString;
use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;

fn launcher() -> Launcher {
    Launcher {
        cwd: PathBuf::from("/private/defe/work"),
        bin_dir: PathBuf::from("/private/defe/bin"),
        environment: vec![(OsString::from("DEFE_ENV"), OsString::from("1"))],
        command: vec![OsString::from("/bin/sh")],
    }
}

#[test]
fn inactive_direnv_launches_the_overlay_directly() {
    let command = build_with_executable(PathBuf::from("/defe-env"), launcher(), Direnv::Inactive);
    assert_eq!(command[0], "/defe-env");
    assert_eq!(command[1], LAUNCHER_ARGUMENT);
    assert_eq!(command.iter().filter(|arg| *arg == "--bin-dir").count(), 1);
}

#[test]
fn active_direnv_wraps_the_overlay_in_shell_neutral_restoration() {
    let command = build_with_executable(
        PathBuf::from("/defe-env"),
        launcher(),
        Direnv::Active {
            executable: PathBuf::from("/fake/direnv"),
        },
    );
    assert_eq!(
        command[..5],
        [
            OsString::from("/fake/direnv"),
            OsString::from("exec"),
            OsString::from("/"),
            OsString::from("/defe-env"),
            OsString::from(LAUNCHER_ARGUMENT),
        ]
    );
    assert_eq!(command.iter().filter(|arg| *arg == "--bin-dir").count(), 1);
}

#[test]
fn launch_policy_selects_private_work_cwd_or_preserves_explicit_cwd() {
    let root = tempfile::tempdir().unwrap();
    let invocation_cwd = root.path().join("caller");
    std::fs::create_dir(&invocation_cwd).unwrap();
    let shell = OsString::from("/bin/sh");
    let shell_plan = prepare_with_context(
        &[],
        root.path(),
        Some(shell),
        invocation_cwd.clone(),
        PathBuf::from("/defe-env"),
        Direnv::Inactive,
    )
    .unwrap();
    assert_eq!(shell_plan.cwd, root.path().join("work"));
    assert_eq!(
        std::fs::metadata(&shell_plan.cwd).unwrap().mode() & 0o777,
        0o700
    );

    let explicit = prepare_with_context(
        &[OsString::from("/bin/true")],
        root.path(),
        None,
        invocation_cwd.clone(),
        PathBuf::from("/defe-env"),
        Direnv::Inactive,
    )
    .unwrap();
    assert_eq!(explicit.cwd, invocation_cwd);
}

#[test]
fn neutral_cwd_rejects_environment_files_in_any_ancestor() {
    for name in [".envrc", ".env"] {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("child/work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(root.path().join(name), "export SURPRISE=1").unwrap();
        assert!(
            ensure_neutral_cwd(&work)
                .unwrap_err()
                .to_string()
                .contains("contains an environment file")
        );
    }
}
