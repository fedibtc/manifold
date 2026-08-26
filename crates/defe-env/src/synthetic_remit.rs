//! Prepares one conspicuously synthetic guardian-fee remittance for local QA.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail, ensure};
use serde_json::Value;
use tokio::process::Command;

const FUNDING_CUSHION_MSATS: u64 = 100_000;
const MINIMUM_FUNDING_MSATS: u64 = 1_000_000;
const GUARDIAN_FEE_READY_TIMEOUT: Duration = Duration::from_secs(120);

/// Runs the generated `fees synthetic-remit` helper after its wrapper selected a guardian.
pub async fn run(raw_args: &[OsString]) -> Result<()> {
    let args = Args::parse(raw_args)?;
    let initial_status = wait_for_guardian_fee_readiness(&args).await?;
    let initial_lifetime = initial_status["lifetime_remitted_msat"]
        .as_u64()
        .context("guardian-fees show did not return lifetime_remitted_msat")?;
    let account: stability_pool_common::Account = serde_json::from_str(
        initial_status["remittance_account"]
            .as_str()
            .context("guardian-fees show did not return remittance_account")?,
    )
    .context("parse guardian remittance account")?;
    let recipient = account
        .as_single()
        .context("guardian remittance account is not single-signature")?;

    let wallet_dir = args.root.join("synthetic-remit-payer");
    let wallet_secret_file = args.root.join("synthetic-remit-payer-secret");
    ensure_wallet_secret(&wallet_secret_file)?;
    let invite = fs::read_to_string(&args.invite_file)
        .with_context(|| format!("read federation invite {}", args.invite_file.display()))?;
    let federation_id = join_wallet(&args, &wallet_dir, &wallet_secret_file, &invite).await?;
    fund_wallet_if_needed(
        &args,
        &wallet_dir,
        &wallet_secret_file,
        &federation_id,
        args.amount_msats,
    )
    .await?;

    let metadata_file = args.root.join("synthetic-remit-metadata");
    let metadata = fman_core::remittance_metadata::encrypt(
        recipient,
        &fman_core::remittance_metadata::RemittanceMetadata {
            version: 1,
            total_msats: args.amount_msats,
            breakdown: vec![fman_core::remittance_metadata::RemittanceBreakdownItem {
                module: "mint".to_owned(),
                direction: "send".to_owned(),
                amount_msats: args.amount_msats,
            }],
            remitted_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("system clock is before the Unix epoch")?
                .as_secs(),
        },
    )
    .context("seal synthetic guardian-remittance metadata")?;
    write_private(&metadata_file, &metadata)?;

    let remit_result = payment_wallet(
        &args,
        &wallet_dir,
        &wallet_secret_file,
        [
            "remit-guardian-fee",
            "--payment-federation-id",
            &federation_id,
            "--account-id",
            &account.id().to_string(),
            "--amount-msats",
            &args.amount_msats.to_string(),
            "--metadata-file",
            metadata_file
                .to_str()
                .context("metadata path is not UTF-8")?,
        ],
        Duration::from_secs(60),
    )
    .await;
    if let Err(error) = remit_result {
        return Err(error).context(format!(
            "submit synthetic remittance; it may have committed, so inspect `fees show --guardian {}` before retrying",
            args.guardian
        ));
    }

    wait_for_remittance(&args, initial_lifetime)
        .await
        .context(format!(
            "the synthetic remittance committed; inspect `fees show --guardian {}` before retrying",
            args.guardian
        ))?;
    println!(
        "Synthetic remittance observed for guardian {} ({} msat).",
        args.guardian, args.amount_msats
    );
    println!(
        "Production payer accrual was bypassed: this directly selected the recipient and amount, and fabricated the mint/send breakdown. It did not exercise Fedi app accrual, 4:1:1 splitting, threshold accumulation, or scheduling."
    );
    println!(
        "A successful retry creates another remittance; after an uncertain failure, inspect before retrying."
    );
    println!("Next: fees show --guardian {}", args.guardian);
    println!("Then: fees collect --guardian {}", args.guardian);
    Ok(())
}

#[derive(Debug)]
struct Args {
    root: PathBuf,
    fman_cli: PathBuf,
    fman_data_dir: PathBuf,
    fi_cli: PathBuf,
    bitcoin_cli: PathBuf,
    invite_file: PathBuf,
    guardian: usize,
    seat_id: String,
    amount_msats: u64,
}

impl Args {
    fn parse(raw_args: &[OsString]) -> Result<Self> {
        let mut root = None;
        let mut fman_cli = None;
        let mut fman_data_dir = None;
        let mut fi_cli = None;
        let mut bitcoin_cli = None;
        let mut invite_file = None;
        let mut guardian = None;
        let mut seat_id = None;
        let mut amount_msats = None;
        let mut args = raw_args.iter();
        while let Some(argument) = args.next() {
            let mut value = || {
                args.next()
                    .context("synthetic remit argument is missing its value")
            };
            match argument.to_str() {
                Some("--root") => root = Some(PathBuf::from(value()?)),
                Some("--fman-cli") => fman_cli = Some(PathBuf::from(value()?)),
                Some("--fman-data-dir") => fman_data_dir = Some(PathBuf::from(value()?)),
                Some("--fi-cli") => fi_cli = Some(PathBuf::from(value()?)),
                Some("--bitcoin-cli") => bitcoin_cli = Some(PathBuf::from(value()?)),
                Some("--invite-file") => invite_file = Some(PathBuf::from(value()?)),
                Some("--guardian") => {
                    guardian = Some(
                        value()?
                            .to_str()
                            .context("--guardian is not UTF-8")?
                            .parse()
                            .context("parse --guardian")?,
                    )
                }
                Some("--seat-id") => {
                    seat_id = Some(
                        value()?
                            .to_str()
                            .context("--seat-id is not UTF-8")?
                            .to_owned(),
                    )
                }
                Some("--amount-msats") => {
                    amount_msats = Some(
                        value()?
                            .to_str()
                            .context("--amount-msats is not UTF-8")?
                            .parse()
                            .context("parse --amount-msats")?,
                    )
                }
                _ => bail!("unknown synthetic remit argument {:?}", argument),
            }
        }
        let amount_msats = amount_msats.context("--amount-msats is required")?;
        ensure!(
            amount_msats > 0,
            "--amount-msats must be at least one millisatoshi"
        );
        Ok(Self {
            root: root.context("--root is required")?,
            fman_cli: fman_cli.context("--fman-cli is required")?,
            fman_data_dir: fman_data_dir.context("--fman-data-dir is required")?,
            fi_cli: fi_cli.context("--fi-cli is required")?,
            bitcoin_cli: bitcoin_cli.context("--bitcoin-cli is required")?,
            invite_file: invite_file.context("--invite-file is required")?,
            guardian: guardian.context("--guardian is required")?,
            seat_id: seat_id.context("--seat-id is required")?,
            amount_msats,
        })
    }
}

async fn guardian_fees(args: &Args) -> Result<Value> {
    let output = run_command(
        Command::new(&args.fman_cli)
            .arg("--data-dir")
            .arg(&args.fman_data_dir)
            .args(["guardian-fees", "show", &args.seat_id, "--limit", "1"]),
        "fman-cli guardian-fees show",
        Duration::from_secs(15),
    )
    .await?;
    serde_json::from_str(&output).context("parse guardian-fees show response")
}

async fn wait_for_guardian_fee_readiness(args: &Args) -> Result<Value> {
    let mut last_observation = "guardian-fees show has not completed".to_owned();
    tokio::time::timeout(GUARDIAN_FEE_READY_TIMEOUT, async {
        loop {
            match guardian_fees(args).await {
                Ok(status)
                    if status["lifetime_remitted_msat"].as_u64().is_some()
                        && status["remittance_account"].as_str().is_some() =>
                {
                    return Ok(status);
                }
                Ok(status) => {
                    last_observation =
                        format!("guardian-fees show omitted required fields: {status}");
                }
                Err(error) => last_observation = format!("guardian-fees show failed: {error:#}"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "guardian-fee account was not ready within {} seconds; last observation: {last_observation}",
            GUARDIAN_FEE_READY_TIMEOUT.as_secs()
        )
    })?
}

async fn join_wallet(
    args: &Args,
    wallet_dir: &Path,
    wallet_secret_file: &Path,
    invite: &str,
) -> Result<String> {
    let joined = payment_wallet(
        args,
        wallet_dir,
        wallet_secret_file,
        ["join", "--payment-invite-code", invite.trim()],
        Duration::from_secs(60),
    )
    .await?;
    joined["federationId"]
        .as_str()
        .map(ToOwned::to_owned)
        .context("payment-wallet join did not return federationId")
}

async fn fund_wallet_if_needed(
    args: &Args,
    wallet_dir: &Path,
    wallet_secret_file: &Path,
    federation_id: &str,
    amount_msats: u64,
) -> Result<()> {
    let balance = payment_wallet(
        args,
        wallet_dir,
        wallet_secret_file,
        ["balance", "--payment-federation-id", federation_id],
        Duration::from_secs(30),
    )
    .await?["balanceMsats"]
        .as_u64()
        .context("payment-wallet balance did not return balanceMsats")?;
    let required = amount_msats
        .checked_add(FUNDING_CUSHION_MSATS)
        .context("amount plus funding cushion overflows")?;
    if balance >= required {
        return Ok(());
    }
    let funding_msats = (required - balance).max(MINIMUM_FUNDING_MSATS);
    let deposit = payment_wallet(
        args,
        wallet_dir,
        wallet_secret_file,
        [
            "deposit-address",
            "--payment-federation-id",
            federation_id,
            "--timeout-secs",
            "120",
        ],
        Duration::from_secs(130),
    )
    .await?;
    let address = deposit["address"]
        .as_str()
        .context("payment-wallet deposit-address did not return address")?;
    let miner_address = run_command(
        Command::new(&args.bitcoin_cli).args([
            "-rpcwallet=default",
            "getnewaddress",
            "synthetic-remit-miner",
        ]),
        "bitcoin-cli getnewaddress",
        Duration::from_secs(15),
    )
    .await?;
    let sats = funding_msats.div_ceil(1_000);
    let bitcoin = format!("{}.{:08}", sats / 100_000_000, sats % 100_000_000);
    run_command(
        Command::new(&args.bitcoin_cli).args([
            "-rpcwallet=default",
            "sendtoaddress",
            address,
            &bitcoin,
        ]),
        "bitcoin-cli fund synthetic remit wallet",
        Duration::from_secs(15),
    )
    .await?;
    run_command(
        Command::new(&args.bitcoin_cli).args([
            "-rpcwallet=default",
            "generatetoaddress",
            "7",
            miner_address.trim(),
        ]),
        "bitcoin-cli confirm synthetic remit wallet funding",
        Duration::from_secs(30),
    )
    .await?;
    payment_wallet(
        args,
        wallet_dir,
        wallet_secret_file,
        [
            "wait-balance",
            "--payment-federation-id",
            federation_id,
            "--minimum-sats",
            &required.div_ceil(1_000).to_string(),
            "--timeout-secs",
            "180",
        ],
        Duration::from_secs(190),
    )
    .await?;
    Ok(())
}

async fn payment_wallet<const N: usize>(
    args: &Args,
    wallet_dir: &Path,
    wallet_secret_file: &Path,
    command_args: [&str; N],
    timeout: Duration,
) -> Result<Value> {
    let output = run_command(
        Command::new(&args.fi_cli)
            .arg("--json")
            .arg("payment-wallet")
            .arg("--wallet-data-dir")
            .arg(wallet_dir)
            .arg("--wallet-secret-file")
            .arg(wallet_secret_file)
            .args(command_args),
        "fi-cli payment-wallet",
        timeout,
    )
    .await?;
    serde_json::from_str(&output).context("parse fi-cli payment-wallet response")
}

async fn wait_for_remittance(args: &Args, initial_lifetime: u64) -> Result<()> {
    let expected_lifetime = initial_lifetime
        .checked_add(args.amount_msats)
        .context("expected remittance lifetime overflows")?;
    let mut last_observation = "guardian-fees show has not completed".to_owned();
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match guardian_fees(args).await {
                Ok(status)
                    if status["lifetime_remitted_msat"].as_u64() >= Some(expected_lifetime) =>
                {
                    return Ok(());
                }
                Ok(status) => {
                    last_observation = format!(
                        "lifetime remitted is {:?}, expected at least {expected_lifetime}",
                        status["lifetime_remitted_msat"].as_u64()
                    );
                }
                Err(error) => last_observation = format!("guardian-fees show failed: {error:#}"),
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "synthetic remittance did not become visible within 60 seconds; last observation: {last_observation}"
        )
    })?
}

fn ensure_wallet_secret(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    // This is a disposable local wallet root, not a production identity. The private
    // environment root confines it, and stable bytes let an interrupted helper reopen it.
    write_private(path, hex::encode([71_u8; 64]).as_bytes())
}

fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("protect {}", path.display()))?;
    Ok(())
}

async fn run_command(command: &mut Command, name: &str, timeout: Duration) -> Result<String> {
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .with_context(|| format!("{name} timed out"))??;
    ensure!(
        output.status.success(),
        "{name} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).context("command output is not UTF-8")
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use super::{Args, run};

    const ACCOUNT: &str = r#"{"acc_type":"BtcDepositor","pub_keys":["031b84c5567b126440995d3ed5aaba0565d71e1834604819ff9c17f5e9d5dd078f"],"threshold":1}"#;

    fn complete_args() -> Vec<OsString> {
        [
            "--root",
            "/root",
            "--fman-cli",
            "/fman-cli",
            "--fman-data-dir",
            "/fman",
            "--fi-cli",
            "/fi-cli",
            "--bitcoin-cli",
            "/bitcoin-cli",
            "--invite-file",
            "/invite",
            "--guardian",
            "1",
            "--seat-id",
            "seat",
            "--amount-msats",
            "200000",
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }

    #[test]
    fn parse_requires_positive_amount() {
        let mut args = complete_args();
        *args.last_mut().unwrap() = "0".into();
        assert!(
            Args::parse(&args)
                .unwrap_err()
                .to_string()
                .contains("at least one")
        );
    }

    #[test]
    fn parse_rejects_unrecognized_arguments() {
        let mut args = complete_args();
        args.push("--unexpected".into());
        assert!(
            Args::parse(&args)
                .unwrap_err()
                .to_string()
                .contains("unknown")
        );
    }

    #[tokio::test]
    async fn synthetic_remit_waits_for_readiness_then_funds_and_observes() {
        let root = tempfile::tempdir().unwrap();
        let fman = root.path().join("fman");
        let fi_cli = root.path().join("fi-cli");
        let bitcoin_cli = root.path().join("bitcoin-cli");
        let invite = root.path().join("invite");
        fs::write(&invite, "fed1test-invite").unwrap();
        let initial = serde_json::json!({
            "lifetime_remitted_msat": 0,
            "remittance_account": ACCOUNT,
        });
        let remitted = serde_json::json!({
            "lifetime_remitted_msat": 200_000,
            "remittance_account": ACCOUNT,
        });
        fs::write(root.path().join("initial.json"), initial.to_string()).unwrap();
        fs::write(root.path().join("remitted.json"), remitted.to_string()).unwrap();
        let root_path = root.path().display().to_string();
        write_script(
            &fman,
            &r#"#!/bin/sh
if [ ! -f "$TEST_ROOT/ready" ]; then touch "$TEST_ROOT/ready"; echo "not ready" >&2; exit 1; fi
if [ -f "$TEST_ROOT/remitted" ] && [ ! -f "$TEST_ROOT/post-submit-show-retried" ]; then
  touch "$TEST_ROOT/post-submit-show-retried"; echo "observation restarting" >&2; exit 1
fi
if [ -f "$TEST_ROOT/remitted" ]; then cat "$TEST_ROOT/remitted.json"; else cat "$TEST_ROOT/initial.json"; fi
"#
            .replace("$TEST_ROOT", &root_path),
        );
        write_script(
            &fi_cli,
            &r#"#!/bin/sh
printf '%s\n' "$*" >>"$TEST_ROOT/fi-args"
case "$*" in
  *" join --payment-invite-code "*) printf '%s\n' '{"federationId":"fed"}' ;;
  *" balance --payment-federation-id "*) printf '%s\n' '{"balanceMsats":0}' ;;
  *" deposit-address --payment-federation-id "*) printf '%s\n' '{"address":"bcrt1qsynthetic"}' ;;
  *" wait-balance --payment-federation-id "*) printf '%s\n' '{"balanceMsats":1100000}' ;;
  *" remit-guardian-fee "*) touch "$TEST_ROOT/remitted"; printf '%s\n' '{"operationId":"synthetic"}' ;;
  *) echo "unexpected fi-cli arguments: $*" >&2; exit 1 ;;
esac
"#
            .replace("$TEST_ROOT", &root_path),
        );
        write_script(
            &bitcoin_cli,
            &r#"#!/bin/sh
printf '%s\n' "$*" >>"$TEST_ROOT/bitcoin-args"
case "$*" in
  *" getnewaddress "*) printf '%s\n' 'bcrt1qminer' ;;
  *" sendtoaddress "*) printf '%s\n' 'txid' ;;
  *" generatetoaddress "*) printf '%s\n' '["block"]' ;;
  *) echo "unexpected bitcoin-cli arguments: $*" >&2; exit 1 ;;
esac
"#
            .replace("$TEST_ROOT", &root_path),
        );
        let raw = [
            "--root",
            root.path().to_str().unwrap(),
            "--fman-cli",
            fman.to_str().unwrap(),
            "--fman-data-dir",
            root.path().to_str().unwrap(),
            "--fi-cli",
            fi_cli.to_str().unwrap(),
            "--bitcoin-cli",
            bitcoin_cli.to_str().unwrap(),
            "--invite-file",
            invite.to_str().unwrap(),
            "--guardian",
            "1",
            "--seat-id",
            "seat",
            "--amount-msats",
            "200000",
        ]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();

        run(&raw).await.unwrap();

        assert!(root.path().join("ready").exists());
        assert!(root.path().join("remitted").exists());
        assert!(root.path().join("post-submit-show-retried").exists());
        assert!(root.path().join("synthetic-remit-payer-secret").exists());
        assert_eq!(
            fs::metadata(root.path().join("synthetic-remit-payer-secret"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let fi_args = fs::read_to_string(root.path().join("fi-args")).unwrap();
        assert!(fi_args.contains("deposit-address"));
        assert!(fi_args.contains("remit-guardian-fee"));
        assert!(fi_args.contains("--metadata-file"));
        let bitcoin_args = fs::read_to_string(root.path().join("bitcoin-args")).unwrap();
        assert!(bitcoin_args.contains("-rpcwallet=default getnewaddress"));
        assert!(bitcoin_args.contains("-rpcwallet=default sendtoaddress"));
    }

    fn write_script(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}
