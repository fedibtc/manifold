//! Runs bounded artificial traffic against the composed federation.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};
use tokio::process::Command;

const MAX_USERS: u16 = 1_000;
const MAX_DURATION_SECS: u64 = 3_600;

struct CommonArgs {
    load_test_tool: PathBuf,
    invite_file: PathBuf,
    routes_file: PathBuf,
}

enum Traffic {
    Connections { users: u16, duration: Duration },
    Mint,
    Lightning,
}

/// Runs one artificial-traffic request.
pub(crate) async fn run(raw_args: &[OsString]) -> Result<()> {
    let (common, traffic) = parse_args(raw_args)?;
    match traffic {
        Traffic::Connections { users, duration } => {
            let invite = fs::read_to_string(&common.invite_file)
                .with_context(|| format!("read invite {}", common.invite_file.display()))?;
            let routes = fs::read_to_string(&common.routes_file)
                .with_context(|| format!("read Iroh routes {}", common.routes_file.display()))?;
            eprintln!(
                "traffic connections: exercising real federation operations; this does not cause or prove production Fedi fee accrual"
            );
            // Fedimint 0.11.2 `test-connect` assumes every endpoint URL has a
            // TCP port and panics on this federation's portless Iroh URLs.
            // Repeated config downloads use the same pinned connector registry
            // without that invalid formatting assumption.
            let deadline = tokio::time::Instant::now() + duration;
            loop {
                let mut command = Command::new(&common.load_test_tool);
                command
                    .args(["--users", &users.to_string(), "test-download"])
                    .args(["--invite-code", invite.trim()])
                    .env("FM_IROH_CONNECT_OVERRIDES", routes.trim())
                    .kill_on_drop(true);
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let timeout = remaining.saturating_add(Duration::from_secs(30));
                let status = tokio::time::timeout(timeout, command.status())
                    .await
                    .with_context(|| {
                        format!("connection traffic timed out after {}s", timeout.as_secs())
                    })?
                    .context("start connection traffic")?;
                ensure!(status.success(), "connection traffic failed with {status}");
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
            }
        }
        Traffic::Mint => bail!(
            "mint traffic is unsupported with pinned Fedimint 0.11.2: the formed federation uses mintv2 and walletv2, while fedimint-load-test-tool requires the v1 mint and funds it through a v1 wallet; a mintv2-capable upstream load path is required. This mode does not cause or prove production Fedi fee accrual"
        ),
        Traffic::Lightning => bail!(
            "lightning traffic is unsupported with pinned Fedimint 0.11.2: the composed environment has one gateway, while fedimint-load-test-tool rejects invoices created by the paying gateway; add an independent connected invoice source before enabling this mode. This mode does not cause or prove production Fedi fee accrual"
        ),
    }
    Ok(())
}

fn parse_args(raw_args: &[OsString]) -> Result<(CommonArgs, Traffic)> {
    let mut load_test_tool = None;
    let mut invite_file = None;
    let mut routes_file = None;
    let mut index = 0;
    while index < raw_args.len() {
        let argument = raw_args[index].to_string_lossy();
        let target = match argument.as_ref() {
            "--load-test-tool" => &mut load_test_tool,
            "--invite-file" => &mut invite_file,
            "--routes-file" => &mut routes_file,
            "connections" | "mint" | "lightning" => break,
            _ => bail!("unrecognized internal traffic argument: {argument}"),
        };
        index += 1;
        *target = Some(PathBuf::from(
            raw_args
                .get(index)
                .with_context(|| format!("{argument} requires a value"))?,
        ));
        index += 1;
    }
    let command = raw_args
        .get(index)
        .and_then(|value| value.to_str())
        .context("usage: traffic connections|mint|lightning [OPTIONS]")?;
    let options = &raw_args[index + 1..];
    let traffic = match command {
        "connections" => {
            validate_options(options, &["--users", "--duration-secs"])?;
            let users = option(options, "--users", 10, MAX_USERS)?;
            let duration_secs = option(options, "--duration-secs", 60, MAX_DURATION_SECS)?;
            ensure!(users > 0, "--users must be positive");
            ensure!(duration_secs > 0, "--duration-secs must be positive");
            Traffic::Connections {
                users,
                duration: Duration::from_secs(duration_secs),
            }
        }
        "mint" => {
            validate_options(options, &["--users", "--notes-per-user"])?;
            let users = option(options, "--users", 10, MAX_USERS)?;
            let notes_per_user = option(options, "--notes-per-user", 2, 20)?;
            ensure!(users > 0, "--users must be positive");
            ensure!(notes_per_user > 0, "--notes-per-user must be positive");
            Traffic::Mint
        }
        "lightning" => {
            validate_options(options, &["--users", "--invoices-per-user"])?;
            let users = option(options, "--users", 10, MAX_USERS)?;
            let invoices_per_user = option(options, "--invoices-per-user", 1, 20)?;
            ensure!(users > 0, "--users must be positive");
            ensure!(
                invoices_per_user > 0,
                "--invoices-per-user must be positive"
            );
            Traffic::Lightning
        }
        _ => unreachable!(),
    };
    Ok((
        CommonArgs {
            load_test_tool: load_test_tool.context("internal --load-test-tool is missing")?,
            invite_file: invite_file.context("internal --invite-file is missing")?,
            routes_file: routes_file.context("internal --routes-file is missing")?,
        },
        traffic,
    ))
}

fn option<T>(options: &[OsString], name: &str, default: T, maximum: T) -> Result<T>
where
    T: Copy + Ord + std::str::FromStr + std::fmt::Display,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let mut chunks = options.chunks_exact(2);
    for pair in &mut chunks {
        if pair[0] == name {
            let value = pair[1]
                .to_str()
                .context("traffic option value is not UTF-8")?
                .parse::<T>()?;
            ensure!(value <= maximum, "{name} must not exceed {maximum}");
            return Ok(value);
        }
    }
    Ok(default)
}

fn validate_options(options: &[OsString], allowed: &[&str]) -> Result<()> {
    ensure!(
        options.len().is_multiple_of(2),
        "every traffic option requires a value"
    );
    for pair in options.chunks_exact(2) {
        let name = pair[0].to_string_lossy();
        ensure!(
            allowed.contains(&name.as_ref()),
            "unsupported traffic option {name}"
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "traffic_tests.rs"]
mod tests;
