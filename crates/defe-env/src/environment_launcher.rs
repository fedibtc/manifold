//! Builds and dispatches the shell-independent Defe runtime environment launcher.
//!
//! Preparation validates the command and neutral cwd before resource setup. At
//! launch, direnv first restores the pre-development baseline; the internal
//! launcher then removes transient tracking and applies the Defe overlay.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};

const LAUNCHER_ARGUMENT: &str = "--internal-environment-launcher";
const COMPOSITION_PROBE_ARGUMENT: &str = "--internal-environment-composition-probe";
const TRANSIENT_DIRENV_KEYS: [&str; 5] = [
    "DIRENV_DIFF",
    "DIRENV_DIR",
    "DIRENV_FILE",
    "DIRENV_WATCHES",
    "DIRENV_IN_ENVRC",
];

/// A command and cwd validated before Defe starts composing expensive resources.
#[derive(Debug)]
pub(crate) struct LaunchPlan {
    /// Absolute path to the internal launcher executable.
    executable: PathBuf,
    /// Final command and arguments.
    command: Vec<OsString>,
    /// Cwd selected for the final command.
    cwd: PathBuf,
    /// Pre-resolved direnv restoration state.
    direnv: Direnv,
}

impl LaunchPlan {
    /// Builds the complete command after adding Defe's generated runtime values.
    pub(crate) fn build(self, bin_dir: &Path, environment: &[(&str, OsString)]) -> Vec<OsString> {
        build_with_executable(
            self.executable,
            Launcher {
                cwd: self.cwd,
                bin_dir: bin_dir.to_path_buf(),
                environment: environment
                    .iter()
                    .map(|(key, value)| (OsString::from(*key), value.clone()))
                    .collect(),
                command: self.command,
            },
            self.direnv,
        )
    }
}

/// Dispatches an internal environment-launcher operation when `args` requests one.
///
/// The composition-probe operation is a deterministic integration-test seam for
/// the same baseline-restoration and overlay path used by a composed environment.
pub(crate) fn dispatch(args: &[OsString]) {
    match args.first().and_then(|argument| argument.to_str()) {
        Some(LAUNCHER_ARGUMENT) => execute_launcher(args),
        Some(COMPOSITION_PROBE_ARGUMENT) => {
            let launcher = parse_launcher(args).unwrap_or_else(|_| std::process::exit(127));
            ensure_neutral_cwd(&launcher.cwd).unwrap_or_else(|error| {
                eprintln!("defe env: {error:#}");
                std::process::exit(1);
            });
            let command = build_with_executable(
                std::env::current_exe().unwrap_or_else(|_| std::process::exit(127)),
                launcher,
                detect_direnv().unwrap_or_else(|error| {
                    eprintln!("defe env: {error:#}");
                    std::process::exit(1);
                }),
            );
            exec_argv(&command)
        }
        _ => {}
    }
}

/// Validates the final command and neutralization mechanism before resource composition.
pub(crate) fn prepare(requested: &[OsString], root: &Path) -> Result<LaunchPlan> {
    prepare_with_context(
        requested,
        root,
        std::env::var_os("SHELL"),
        std::env::current_dir().context("record explicit environment command cwd")?,
        std::env::current_exe().context("locate the running defe-env binary")?,
        detect_direnv()?,
    )
}

fn prepare_with_context(
    requested: &[OsString],
    root: &Path,
    shell: Option<OsString>,
    invocation_cwd: PathBuf,
    executable: PathBuf,
    direnv: Direnv,
) -> Result<LaunchPlan> {
    let default_shell = requested.is_empty();
    let command = if default_shell {
        let shell = PathBuf::from(shell.unwrap_or_else(|| OsString::from("/bin/sh")));
        ensure!(
            shell.is_absolute(),
            "defe env requires an absolute SHELL for its private runtime directory"
        );
        let metadata = fs::metadata(&shell)
            .with_context(|| format!("inspect environment SHELL {}", shell.display()))?;
        ensure!(
            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
            "defe env requires an executable SHELL at {}",
            shell.display()
        );
        vec![shell.into_os_string()]
    } else {
        requested.to_vec()
    };
    let work_dir = root.join("work");
    fs::create_dir_all(&work_dir)
        .with_context(|| format!("create private runtime cwd {}", work_dir.display()))?;
    fs::set_permissions(&work_dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("make runtime cwd private {}", work_dir.display()))?;
    let cwd = if default_shell {
        let canonical_work_dir = fs::canonicalize(&work_dir)
            .with_context(|| format!("canonicalize runtime cwd {}", work_dir.display()))?;
        ensure_neutral_cwd(&canonical_work_dir)?;
        canonical_work_dir
    } else {
        invocation_cwd
    };
    Ok(LaunchPlan {
        executable,
        command,
        cwd,
        direnv,
    })
}

#[derive(Debug)]
struct Launcher {
    cwd: PathBuf,
    bin_dir: PathBuf,
    environment: Vec<(OsString, OsString)>,
    command: Vec<OsString>,
}

#[derive(Debug)]
enum Direnv {
    Inactive,
    Active { executable: PathBuf },
}

fn detect_direnv() -> Result<Direnv> {
    if std::env::var_os("DIRENV_DIFF").is_none() {
        return Ok(Direnv::Inactive);
    }
    ensure!(
        !Path::new("/.envrc").exists() && !Path::new("/.env").exists(),
        "defe env refuses to neutralize direnv because / contains an environment file"
    );
    let executable = resolve_required_executable("direnv")
        .context("defe env found DIRENV_DIFF but cannot find executable direnv")?;
    Ok(Direnv::Active { executable })
}

fn ensure_neutral_cwd(cwd: &Path) -> Result<()> {
    for ancestor in cwd.ancestors() {
        ensure!(
            !ancestor.join(".envrc").exists() && !ancestor.join(".env").exists(),
            "defe env refuses runtime cwd {} because ancestor {} contains an environment file",
            cwd.display(),
            ancestor.display()
        );
    }
    Ok(())
}

fn build_with_executable(executable: PathBuf, launcher: Launcher, direnv: Direnv) -> Vec<OsString> {
    let mut command = vec![
        executable.into_os_string(),
        OsString::from(LAUNCHER_ARGUMENT),
        OsString::from("--cwd"),
        launcher.cwd.into_os_string(),
        OsString::from("--bin-dir"),
        launcher.bin_dir.into_os_string(),
    ];
    for (key, value) in launcher.environment {
        command.extend([OsString::from("--env"), key, value]);
    }
    command.push(OsString::from("--"));
    command.extend(launcher.command);

    match direnv {
        Direnv::Inactive => command,
        Direnv::Active { executable } => {
            let mut outer = vec![
                executable.into_os_string(),
                OsString::from("exec"),
                OsString::from("/"),
            ];
            outer.extend(command);
            outer
        }
    }
}

fn parse_launcher(args: &[OsString]) -> Result<Launcher> {
    let mut args = args.iter().skip(1);
    let mut cwd = None;
    let mut bin_dir = None;
    let mut environment = Vec::new();
    loop {
        match args.next() {
            Some(argument) if argument == "--cwd" => cwd = args.next().map(PathBuf::from),
            Some(argument) if argument == "--bin-dir" => bin_dir = args.next().map(PathBuf::from),
            Some(argument) if argument == "--env" => {
                let key = args.next().context("missing environment key")?;
                let value = args.next().context("missing environment value")?;
                environment.push((key.clone(), value.clone()));
            }
            Some(argument) if argument == "--" => break,
            _ => anyhow::bail!("invalid internal environment launcher arguments"),
        }
    }
    let command = args.cloned().collect::<Vec<_>>();
    ensure!(!command.is_empty(), "missing environment command");
    Ok(Launcher {
        cwd: cwd.context("missing environment cwd")?,
        bin_dir: bin_dir.context("missing environment bin directory")?,
        environment,
        command,
    })
}

fn execute_launcher(args: &[OsString]) -> ! {
    let launcher = parse_launcher(args).unwrap_or_else(|_| std::process::exit(127));
    let mut command = std::process::Command::new(&launcher.command[0]);
    command
        .args(&launcher.command[1..])
        .current_dir(launcher.cwd)
        .envs(launcher.environment);
    for key in TRANSIENT_DIRENV_KEYS {
        command.env_remove(key);
    }
    let mut paths = vec![launcher.bin_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let error = match std::env::join_paths(paths) {
        Ok(path) => command.env("PATH", path).exec(),
        Err(error) => {
            eprintln!("defe env: construct runtime PATH: {error}");
            std::process::exit(127);
        }
    };
    eprintln!(
        "defe env: execute environment command {}: {error}",
        launcher.command[0].to_string_lossy()
    );
    std::process::exit(127);
}

fn exec_argv(command: &[OsString]) -> ! {
    let error = std::process::Command::new(&command[0])
        .args(&command[1..])
        .exec();
    eprintln!(
        "defe env: execute environment launcher {}: {error}",
        command[0].to_string_lossy()
    );
    std::process::exit(127);
}

/// Resolves a required runtime executable from the caller's pre-neutralization PATH.
pub(crate) fn resolve_required_executable(name: &str) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("record cwd while searching PATH")?;
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|path| {
            if path.is_absolute() {
                path.join(name)
            } else {
                cwd.join(path).join(name)
            }
        })
        .find(|path| {
            fs::metadata(path).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
        .context("search PATH")
}

#[cfg(test)]
#[path = "environment_launcher_tests.rs"]
mod tests;
