//! Keeps a formed local federation and its supporting services alive for manual use.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};
use defe_client::{
    AsyncDefeClient, FlipRequest, FmanInfo, FmanRequest, GatewaydInfo, GatewaydRequest,
    ResourceDescriptor, SharingMode,
};
use iroh_base_035::{NodeAddr, NodeId, SecretKey, ticket::NodeTicket};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::process::Command;

mod flip_setup;

const GUARDIAN_COUNT: usize = 7;
const FI_ACCOUNT: &[u8] = br#"{"acc_type":"BtcDepositor","pub_keys":["031b84c5567b126440995d3ed5aaba0565d71e1834604819ff9c17f5e9d5dd078f"],"threshold":1}"#;
const FMAN_OPERATOR_UI_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../operator-ui");
const FMAN_OPERATOR_UI_URL: &str = "http://127.0.0.1:5174";
const PGID_FD_ENV: &str = "DEV_DEFE_ENV_PGID_FD";

#[derive(Debug)]
struct Args {
    root: PathBuf,
    logs_dir: PathBuf,
    fi_cli: PathBuf,
    fman_cli: PathBuf,
    gateway_cli: PathBuf,
    bitcoin_cli: PathBuf,
    complete_liquidity: bool,
    command: Vec<OsString>,
    pgid_fd: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest<'a> {
    schema_version: u8,
    ready: bool,
    state: &'static str,
    federation: FederationManifest<'a>,
    fmans: Vec<FmanManifest<'a>>,
    bitcoin: BitcoinManifest<'a>,
    nostr_relay_url: &'a str,
    gateway: GatewayManifest<'a>,
    flip: FlipManifest<'a>,
    logs_dir: &'a Path,
    secrets_file: &'a Path,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FederationManifest<'a> {
    invite_file: &'a Path,
    fi_state_dir: &'a Path,
    fi_account_file: &'a Path,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FmanManifest<'a> {
    number: usize,
    seat_id: &'a str,
    locator: &'a str,
    api_base_url: &'a str,
    auth_url: String,
    admin_url: String,
    data_dir: &'a Path,
    safe_journal_dir: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BitcoinManifest<'a> {
    rpc_url: &'a str,
    data_dir: &'a Path,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GatewayManifest<'a> {
    api_url: &'a str,
    state: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FlipManifest<'a> {
    admin_url: &'a str,
    data_dir: &'a Path,
    state: &'static str,
    public_endpoint_id: &'a str,
}

#[tokio::main]
async fn main() {
    let raw_args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if raw_args
        .first()
        .is_some_and(|arg| arg == "--internal-child-gate")
    {
        run_internal_child_gate(&raw_args);
    }
    let result = parse_args(raw_args).and_then(|args| {
            if args.complete_liquidity {
                bail!(
                    "--complete-liquidity is not implemented yet; basic environment remains available without it"
                );
            }
            Ok(args)
        });
    let status = match result {
        Ok(args) => run(args).await,
        Err(error) => Err(error),
    };
    match status {
        Ok(status) => std::process::exit(exit_code(status)),
        Err(error) => {
            eprintln!("defe env: {error:#}");
            std::process::exit(1);
        }
    }
}

fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}

fn run_internal_child_gate(args: &[OsString]) -> ! {
    let fd = |index: usize| {
        args.get(index)
            .and_then(|arg| arg.to_str())
            .and_then(|arg| arg.parse::<i32>().ok())
            .unwrap_or_else(|| std::process::exit(127))
    };
    let read_fd = fd(1);
    unsafe { libc::close(fd(2)) };
    let mut byte = 0_u8;
    if unsafe { libc::read(read_fd, (&raw mut byte).cast(), 1) } != 1 {
        std::process::exit(127);
    }
    unsafe { libc::close(read_fd) };
    let command = args.get(4).unwrap_or_else(|| std::process::exit(127));
    let error = std::process::Command::new(command).args(&args[5..]).exec();
    eprintln!("defe env: execute {}: {error}", command.to_string_lossy());
    std::process::exit(127);
}

async fn run(args: Args) -> Result<ExitStatus> {
    fs::create_dir_all(&args.root)
        .with_context(|| format!("create environment root {}", args.root.display()))?;
    set_private(&args.root)?;
    let mut defe = AsyncDefeClient::connect_from_env()
        .await
        .context("connect to the defe server")?;

    status("allocating regtest Bitcoin and Nostr relay");
    let bitcoin_lease = defe.request_bitcoind(SharingMode::Exclusive).await?;
    let ResourceDescriptor::Bitcoind(bitcoin) = bitcoin_lease.descriptor.clone() else {
        bail!("defe returned the wrong descriptor for bitcoind");
    };
    let relay_lease = defe.request_nostr_relay(SharingMode::Exclusive).await?;
    let ResourceDescriptor::NostrRelay(relay) = relay_lease.descriptor.clone() else {
        bail!("defe returned the wrong descriptor for the Nostr relay");
    };

    let first_port_base = defe_portalloc::port_alloc(607).context("reserve FMan port grid")?;
    ensure!(
        first_port_base <= u16::MAX - 607,
        "allocated FMan port grid exceeds u16"
    );
    let routes = local_iroh_overrides(first_port_base);
    let mut fmans = Vec::with_capacity(GUARDIAN_COUNT);
    for guardian in 0..GUARDIAN_COUNT {
        status(&format!("allocating Fleet Manager {}/7", guardian + 1));
        let lease = defe
            .request_fman(FmanRequest {
                sharing: SharingMode::Exclusive,
                bitcoind: bitcoin.clone(),
                nostr_relay_url: relay.url.clone(),
                first_port_base: first_port_base + u16::try_from(guardian)? * 100,
                iroh_connect_overrides: routes.clone(),
            })
            .await?;
        let ResourceDescriptor::Fman(fman) = lease.descriptor.clone() else {
            bail!("defe returned the wrong descriptor for FMan");
        };
        fmans.push((lease, fman));
    }
    for (_, fman) in &fmans {
        run_command(
            Command::new(&args.fman_cli)
                .arg("--data-dir")
                .arg(&fman.data_dir)
                .arg("plans")
                .arg("set")
                .arg("--price-msats")
                .arg("0"),
            "fman-cli plans set",
            Duration::from_secs(15),
        )
        .await?;
    }

    status("forming seven-guardian federation");
    let fi_state_dir = args.root.join("fi-state");
    let invite = form_federation(&args.fi_cli, &fi_state_dir, &fmans, &routes).await?;
    let invite_file = args.root.join("federation-invite");
    write_private(&invite_file, invite.as_bytes())?;
    let fi_account_file = fi_state_dir.join("fi-spv2-account.json");
    let seat_ids = read_seat_ids(&args.fman_cli, &fmans).await?;

    status("starting and connecting gateway");
    let gateway_lease = defe
        .request_gatewayd(GatewaydRequest {
            sharing: SharingMode::Exclusive,
            bitcoind: bitcoin.clone(),
            iroh_connect_overrides: Some(routes.clone()),
        })
        .await?;
    let ResourceDescriptor::Gatewayd(gateway) = gateway_lease.descriptor.clone() else {
        bail!("defe returned the wrong descriptor for gatewayd");
    };
    connect_gateway(&args.gateway_cli, &gateway, &invite).await?;

    status("starting FLIP");
    let flip_lease = defe
        .request_flip(FlipRequest {
            sharing: SharingMode::Exclusive,
            iroh_connect_overrides: Some(routes.clone()),
            holder_authorization_relay_url: Some(relay.url.clone()),
        })
        .await?;
    let ResourceDescriptor::Flip(flip) = flip_lease.descriptor.clone() else {
        bail!("defe returned the wrong descriptor for FLIP");
    };
    status("configuring FLIP and publishing its advertisement");
    let public_endpoint_id =
        flip_setup::configure_and_publish(&flip, &gateway, &bitcoin, &relay.url).await?;

    let secrets_file = args.root.join("secrets.json");
    write_private(
        &secrets_file,
        serde_json::to_vec_pretty(&serde_json::json!({
            "fmans": fmans.iter().map(|(_, fman)| &fman.admin_password).collect::<Vec<_>>(),
            "gateway_password": gateway.password,
            "flip_admin_token": flip.admin_token,
        }))?
        .as_slice(),
    )?;
    let manifest = Manifest {
        schema_version: 1,
        ready: true,
        state: "ready",
        federation: FederationManifest {
            invite_file: &invite_file,
            fi_state_dir: &fi_state_dir,
            fi_account_file: &fi_account_file,
        },
        fmans: fmans
            .iter()
            .zip(&seat_ids)
            .enumerate()
            .map(|(index, ((_, fman), seat_id))| FmanManifest {
                number: index + 1,
                seat_id,
                locator: &fman.locator,
                api_base_url: &fman.admin_url,
                auth_url: fman_api_url(&fman.admin_url, "auth"),
                admin_url: fman_api_url(&fman.admin_url, "admin"),
                data_dir: &fman.data_dir,
                safe_journal_dir: fman.data_dir.join("safe-events"),
            })
            .collect(),
        bitcoin: BitcoinManifest {
            rpc_url: &bitcoin.rpc_url,
            data_dir: &bitcoin.data_dir,
        },
        nostr_relay_url: &relay.url,
        gateway: GatewayManifest {
            api_url: &gateway.api_url,
            state: "connected",
        },
        flip: FlipManifest {
            admin_url: &flip.admin_url,
            data_dir: &flip.data_dir,
            state: "advertising",
            public_endpoint_id: &public_endpoint_id,
        },
        logs_dir: &args.logs_dir,
        secrets_file: &secrets_file,
    };
    let routes_file = args.root.join("iroh-connect-overrides");
    write_private(&routes_file, routes.as_bytes())?;
    let manifest_file = args.root.join("env.json");
    let bin_dir = args.root.join("bin");
    write_tools(
        &bin_dir,
        &args,
        &manifest_file,
        &secrets_file,
        &invite_file,
        &fi_state_dir,
        &routes,
        &fmans,
        &seat_ids,
        &gateway,
        &bitcoin,
    )?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    write_json_atomic(&manifest_file, &manifest)?;
    print_ready(
        &manifest_file,
        &secrets_file,
        &args.logs_dir,
        &args.fman_cli,
        &fmans,
        &gateway,
        &flip.admin_url,
        &public_endpoint_id,
    );

    let child_result = run_child(
        &args.command,
        &args,
        &manifest_file,
        &secrets_file,
        &invite_file,
        &fi_state_dir,
        &routes_file,
        &bin_dir,
        &relay.url,
        &gateway.api_url,
        &flip.admin_url,
        &public_endpoint_id,
        &mut interrupt,
        &mut terminate,
    )
    .await;
    let stopped_manifest = stopped_manifest(&manifest)?;
    if let Err(error) = write_json_atomic(&manifest_file, &stopped_manifest) {
        // Never leave a retained ready manifest after the lease boundary.
        let _ = fs::remove_file(&manifest_file);
        return Err(error).context("invalidate environment manifest before teardown");
    }
    eprintln!("defe env: command exited; releasing all leased resources");
    let status = child_result?;
    drop((
        flip_lease,
        gateway_lease,
        fmans,
        relay_lease,
        bitcoin_lease,
        defe,
    ));
    Ok(status)
}

async fn form_federation(
    fi_cli: &Path,
    state_dir: &Path,
    fmans: &[(defe_client::ResourceLease, FmanInfo)],
    routes: &str,
) -> Result<String> {
    run_command(
        Command::new(fi_cli)
            .arg("--state-dir")
            .arg(state_dir)
            .arg("init"),
        "fi-cli init",
        Duration::from_secs(15),
    )
    .await?;
    let account = state_dir.join("fi-spv2-account.json");
    fs::write(&account, FI_ACCOUNT)?;
    let mut command = Command::new(fi_cli);
    command
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--json")
        .arg("create")
        .arg("--fi-spv2-account-file")
        .arg(account)
        .arg("--federation-size")
        .arg(GUARDIAN_COUNT.to_string())
        .arg("--poll-timeout-secs")
        .arg("120")
        .env("FMAN_E2E_LOCAL_IROH", "1")
        .env("FM_IROH_CONNECT_OVERRIDES", routes);
    for (_, fman) in fmans {
        command.arg("--locator").arg(&fman.locator);
    }
    let output = run_command(&mut command, "fi-cli create", Duration::from_secs(150)).await?;
    let json: serde_json::Value = serde_json::from_str(output.trim())?;
    ensure!(
        json["formation"]["phase"] == "formed",
        "formation failed: {json}"
    );
    json["formation"]["invite_code"]
        .as_str()
        .map(ToOwned::to_owned)
        .context("formed FI response has no invite code")
}

async fn connect_gateway(cli: &Path, gateway: &GatewaydInfo, invite: &str) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let mut command = Command::new(cli);
        command
            .arg("--address")
            .arg(&gateway.api_url)
            .arg(format!("--rpcpassword={}", gateway.password))
            .arg("connect-fed")
            .arg(invite);
        match run_command(
            &mut command,
            "gateway-cli connect-fed",
            Duration::from_secs(15),
        )
        .await
        {
            Ok(_) => return Ok(()),
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn read_seat_ids(
    fman_cli: &Path,
    fmans: &[(defe_client::ResourceLease, FmanInfo)],
) -> Result<Vec<String>> {
    let mut seat_ids = Vec::with_capacity(fmans.len());
    for (_, fman) in fmans {
        let output = run_command(
            Command::new(fman_cli)
                .arg("--data-dir")
                .arg(&fman.data_dir)
                .arg("seats")
                .arg("list"),
            "fman-cli seats list",
            Duration::from_secs(15),
        )
        .await?;
        let value: serde_json::Value = serde_json::from_str(&output)?;
        seat_ids.push(
            value["seats"][0]["seat_id"]
                .as_str()
                .context("formed FMan has no recorded seat id")?
                .to_owned(),
        );
    }
    Ok(seat_ids)
}

#[allow(clippy::too_many_arguments)]
fn write_tools(
    bin_dir: &Path,
    args: &Args,
    manifest: &Path,
    secrets: &Path,
    invite: &Path,
    fi_state_dir: &Path,
    routes: &str,
    fmans: &[(defe_client::ResourceLease, FmanInfo)],
    seat_ids: &[String],
    gateway: &GatewaydInfo,
    bitcoin: &defe_client::BitcoindInfo,
) -> Result<()> {
    fs::create_dir_all(bin_dir)?;
    set_private(bin_dir)?;

    write_wrapper(
        &bin_dir.join("defe-env-info"),
        &format!(
            "#!/bin/sh\nprintf '%s: %s\\n' 'Defe environment manifest' {} 'Secrets' {} 'Invite' {} 'Logs' {}\ncat {}\n",
            shell_escape(manifest.as_os_str()),
            shell_escape(secrets.as_os_str()),
            shell_escape(invite.as_os_str()),
            shell_escape(args.logs_dir.as_os_str()),
            shell_escape(manifest.as_os_str())
        ),
    )?;
    for (index, (_, fman)) in fmans.iter().enumerate() {
        write_wrapper(
            &bin_dir.join(format!("fman-{}", index + 1)),
            &format!(
                "#!/bin/sh\nexec {} --data-dir {} \"$@\"\n",
                shell_escape(args.fman_cli.as_os_str()),
                shell_escape(fman.data_dir.as_os_str())
            ),
        )?;
    }
    write_wrapper(
        &bin_dir.join("fi-cli"),
        &format!(
            "#!/bin/sh\nFMAN_E2E_LOCAL_IROH=1 FM_IROH_CONNECT_OVERRIDES={} exec {} --state-dir {} \"$@\"\n",
            shell_escape(OsStr::new(routes)),
            shell_escape(args.fi_cli.as_os_str()),
            shell_escape(fi_state_dir.as_os_str())
        ),
    )?;
    write_wrapper(
        &bin_dir.join("gateway"),
        &format!(
            "#!/bin/sh\nexec {} --address {} --rpcpassword={} \"$@\"\n",
            shell_escape(args.gateway_cli.as_os_str()),
            shell_escape(OsStr::new(&gateway.api_url)),
            shell_escape(OsStr::new(&gateway.password))
        ),
    )?;
    write_wrapper(
        &bin_dir.join("bitcoin-cli"),
        &format!(
            "#!/bin/sh\nexec {} -regtest -datadir={} -rpcuser={} -rpcpassword={} -rpcport={} \"$@\"\n",
            shell_escape(args.bitcoin_cli.as_os_str()),
            shell_escape(bitcoin.data_dir.as_os_str()),
            shell_escape(OsStr::new(&bitcoin.rpc_username)),
            shell_escape(OsStr::new(&bitcoin.rpc_password)),
            bitcoin.rpc_port
        ),
    )?;

    let mut ui = String::from("#!/bin/sh\ncase \"${1-}\" in\n");
    for (index, (_, fman)) in fmans.iter().enumerate() {
        writeln!(
            ui,
            "  {}) target={}; password={} ;;",
            index + 1,
            shell_escape(OsStr::new(&fman.admin_url)),
            shell_escape(OsStr::new(&fman.admin_password))
        )?;
    }
    ui.push_str(
        "  *) echo 'usage: fman-ui GUARDIAN' >&2; exit 2 ;;\nesac\n\
         echo \"FMan UI: http://127.0.0.1:5174  password: $password\" >&2\n\
         VITE_MOCKS=off FMAN_ADMIN_PROXY_TARGET=\"$target\" exec pnpm --dir ",
    );
    ui.push_str(&shell_escape(OsStr::new(FMAN_OPERATOR_UI_DIR)));
    ui.push_str(" --filter fman exec vite --host 127.0.0.1\n");
    write_wrapper(&bin_dir.join("fman-ui"), &ui)?;

    let mut fees = String::from(
        "#!/bin/sh\nusage() { echo 'usage: fees show --guardian N [ARGS...] | fees collect (--guardian N | --all)' >&2; exit 2; }\n\
         verb=${1-}; shift || usage\n\
         guardian=\nall=0\n\
         while [ \"$#\" -gt 0 ]; do case \"$1\" in --guardian) [ \"$#\" -ge 2 ] || usage; guardian=$2; shift 2 ;; --all) all=1; shift ;; *) break ;; esac; done\n\
         [ \"$all\" -eq 1 ] && [ -n \"$guardian\" ] && usage\n\
         run_one() { n=$1; shift; case \"$n\" in\n",
    );
    for (index, seat_id) in seat_ids.iter().enumerate() {
        writeln!(
            fees,
            "  {}) tool={}; seat={} ;;",
            index + 1,
            shell_escape(bin_dir.join(format!("fman-{}", index + 1)).as_os_str()),
            shell_escape(OsStr::new(seat_id))
        )?;
    }
    fees.push_str(
        "  *) usage ;; esac\n\
         if [ \"$verb\" = collect ]; then\n\
           \"$tool\" guardian-fees collect \"$seat\" \"$@\"; collect_status=$?\n\
           echo \"Post-collect guardian $n status:\" >&2\n\
           \"$tool\" guardian-fees show \"$seat\" || true\n\
           return \"$collect_status\"\n\
         fi\n\
         \"$tool\" guardian-fees show \"$seat\" \"$@\"\n}\n\
         case \"$verb\" in show) [ \"$all\" -eq 0 ] && [ -n \"$guardian\" ] || usage; run_one \"$guardian\" \"$@\" ;;\n\
         collect) if [ \"$all\" -eq 1 ]; then for n in 1 2 3 4 5 6 7; do run_one \"$n\" \"$@\" || exit; done; elif [ -n \"$guardian\" ]; then run_one \"$guardian\" \"$@\"; else usage; fi ;;\n\
         *) usage ;; esac\n",
    );
    write_wrapper(&bin_dir.join("fees"), &fees)?;
    Ok(())
}

fn write_wrapper(path: &Path, contents: &str) -> Result<()> {
    write_private(path, contents.as_bytes())?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_child(
    requested: &[OsString],
    args: &Args,
    manifest: &Path,
    secrets: &Path,
    invite: &Path,
    fi_state_dir: &Path,
    routes_file: &Path,
    bin_dir: &Path,
    relay_url: &str,
    gateway_url: &str,
    flip_url: &str,
    flip_endpoint_id: &str,
    interrupt: &mut tokio::signal::unix::Signal,
    terminate: &mut tokio::signal::unix::Signal,
) -> Result<ExitStatus> {
    let command = if requested.is_empty() {
        vec![std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"))]
    } else {
        requested.to_vec()
    };
    let mut gate = [0_i32; 2];
    if unsafe { libc::pipe(gate.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).context("create child launch gate");
    }
    #[cfg(not(test))]
    let mut child = {
        let mut child = Command::new(std::env::current_exe()?);
        child
            .arg("--internal-child-gate")
            .arg(gate[0].to_string())
            .arg(gate[1].to_string())
            .arg("--")
            .args(&command);
        child
    };
    #[cfg(test)]
    let mut child = {
        let mut child = Command::new("sh");
        let read_fd = gate[0].to_string();
        let write_fd = gate[1].to_string();
        child
            .args([
                "-c",
                "eval \"exec $2>&-\"; IFS= read -r _ <&$1; eval \"exec $1>&-\"; shift 2; exec \"$@\"",
                "sh",
                &read_fd,
                &write_fd,
            ])
            .args(&command);
        child
    };
    child
        .env_remove(PGID_FD_ENV)
        .env("DEFE_ENV", "1")
        .env("DEFE_ENV_SCHEMA_VERSION", "1")
        .env("DEFE_ENV_ROOT", &args.root)
        .env("DEFE_ENV_MANIFEST", manifest)
        .env("DEFE_ENV_SECRETS", secrets)
        .env("DEFE_ENV_LOG_DIR", &args.logs_dir)
        .env("DEFE_ENV_BIN_DIR", bin_dir)
        .env("DEFE_ENV_INVITE_FILE", invite)
        .env("DEFE_ENV_FI_STATE_DIR", fi_state_dir)
        .env("DEFE_ENV_IROH_CONNECT_OVERRIDES_FILE", routes_file)
        .env("DEFE_ENV_NOSTR_RELAY_URL", relay_url)
        .env("DEFE_ENV_GATEWAY_API_URL", gateway_url)
        .env("DEFE_ENV_FLIP_ADMIN_URL", flip_url)
        .env("DEFE_ENV_FLIP_PUBLIC_ENDPOINT_ID", flip_endpoint_id);
    let mut paths = vec![bin_dir.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    child.env("PATH", std::env::join_paths(paths)?);

    // Inspect and transfer only this process's controlling terminal.
    let has_tty = unsafe { libc::isatty(libc::STDIN_FILENO) } == 1;
    let original_foreground = has_tty
        .then(|| unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) })
        .filter(|group| *group > 0);
    if unsafe { libc::fcntl(args.pgid_fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
        return Err(std::io::Error::last_os_error()).context("protect parent PGID channel");
    }
    #[cfg(target_os = "linux")]
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error()).context("become environment child subreaper");
    }
    child.as_std_mut().process_group(0);
    let mut child = match child.spawn() {
        Ok(child) => child,
        Err(error) => {
            unsafe {
                libc::close(gate[0]);
                libc::close(gate[1]);
            }
            return Err(error).with_context(|| {
                format!("start environment command {}", command[0].to_string_lossy())
            });
        }
    };
    unsafe { libc::close(gate[0]) };
    let child_pid = i32::try_from(child.id().context("environment child has no pid")?)?;
    if let Err(error) = publish_child_pgid(child_pid, args.pgid_fd) {
        unsafe { libc::close(gate[1]) };
        let _ = terminate_child_group(&mut child, child_pid).await;
        return Err(error);
    }
    // The composer becomes a background process after this transfer. Ignore
    // SIGTTOU until it has reclaimed the terminal; the child was already spawned
    // with the default disposition.
    let previous_sigttou = has_tty.then(|| unsafe { libc::signal(libc::SIGTTOU, libc::SIG_IGN) });
    if has_tty && unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, child_pid) } != 0 {
        let _ = terminate_child_group(&mut child, child_pid).await;
        if let Some(handler) = previous_sigttou {
            unsafe { libc::signal(libc::SIGTTOU, handler) };
        }
        unsafe { libc::close(gate[1]) };
        bail!(
            "give terminal foreground to environment command: {}",
            std::io::Error::last_os_error()
        );
    }
    let gate_released = unsafe { libc::write(gate[1], b"\n".as_ptr().cast(), 1) } == 1;
    unsafe { libc::close(gate[1]) };
    let status: Result<ExitStatus> = if gate_released {
        tokio::select! {
            status = child.wait() => status.map_err(Into::into),
            _ = interrupt.recv() => terminate_child_group(&mut child, child_pid).await,
            _ = terminate.recv() => terminate_child_group(&mut child, child_pid).await,
        }
    } else {
        let _ = terminate_child_group(&mut child, child_pid).await;
        Err(anyhow::anyhow!("release environment child launch gate"))
    };
    let drain_result = drain_child_group(child_pid).await;
    let restore_result = restore_terminal(original_foreground);
    if let Some(handler) = previous_sigttou {
        unsafe { libc::signal(libc::SIGTTOU, handler) };
    }
    restore_result?;
    drain_result?;
    status
}

fn publish_child_pgid(child_pid: i32, fd: i32) -> Result<()> {
    let bytes = child_pid.to_string();
    let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
    unsafe { libc::close(fd) };
    ensure!(
        written == isize::try_from(bytes.len())?,
        "publish environment child process group"
    );
    Ok(())
}

fn restore_terminal(original_foreground: Option<i32>) -> Result<()> {
    let Some(group) = original_foreground else {
        return Ok(());
    };
    loop {
        if unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, group) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).context("restore terminal foreground process group");
        }
    }
}

async fn terminate_child_group(
    child: &mut tokio::process::Child,
    process_group: i32,
) -> Result<ExitStatus> {
    // The child leads the isolated environment-command process group.
    unsafe { libc::kill(-process_group, libc::SIGTERM) };
    match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(status) => Ok(status?),
        Err(_) => {
            // Do not allow a stuck descendant to outlive the resource leases.
            unsafe { libc::kill(-process_group, libc::SIGKILL) };
            Ok(child.wait().await?)
        }
    }
}

async fn drain_child_group(process_group: i32) -> Result<()> {
    reap_group(process_group);
    if unsafe { libc::kill(-process_group, 0) } != 0 {
        return Ok(());
    }
    unsafe { libc::kill(-process_group, libc::SIGTERM) };
    let graceful_deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    while tokio::time::Instant::now() < graceful_deadline {
        reap_group(process_group);
        if unsafe { libc::kill(-process_group, 0) } != 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    unsafe { libc::kill(-process_group, libc::SIGKILL) };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        reap_group(process_group);
        if unsafe { libc::kill(-process_group, 0) } != 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    bail!("environment process group {process_group} survived SIGKILL")
}

fn reap_group(process_group: i32) {
    let mut status = 0;
    while unsafe { libc::waitpid(-process_group, &mut status, libc::WNOHANG) } > 0 {}
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

fn local_iroh_overrides(first_port_base: u16) -> String {
    let mut overrides = Vec::with_capacity(GUARDIAN_COUNT * 2);
    for guardian in 0..u16::try_from(GUARDIAN_COUNT).expect("guardian count fits u16") {
        let base = first_port_base + guardian * 100;
        for (port, role) in [(base, b"p2p".as_slice()), (base + 1, b"api".as_slice())] {
            let secret = SecretKey::from_bytes(&iroh_key(port, role));
            let node_id: NodeId = secret.public();
            let ticket = NodeTicket::new(
                NodeAddr::new(node_id)
                    .with_direct_addresses([std::net::SocketAddr::from(([127, 0, 0, 1], port))]),
            );
            overrides.push(format!("{node_id}={ticket}"));
        }
    }
    overrides.join(",")
}

fn iroh_key(port: u16, role: &[u8]) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"fman-e2e-local-iroh-v1\0")
        .chain_update(port.to_be_bytes())
        .chain_update(role)
        .finalize()
        .into()
}

fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    write_private(&temporary, &serde_json::to_vec_pretty(value)?)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn set_private(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("protect {}", path.display()))
}

fn status(message: &str) {
    eprintln!("defe env: {message}...");
}

#[allow(clippy::too_many_arguments)]
fn print_ready(
    manifest: &Path,
    secrets: &Path,
    logs: &Path,
    fman_cli: &Path,
    fmans: &[(defe_client::ResourceLease, FmanInfo)],
    gateway: &GatewaydInfo,
    flip_url: &str,
    public_endpoint_id: &str,
) {
    print!(
        "{}",
        ready_output(
            manifest,
            secrets,
            logs,
            fman_cli,
            fmans,
            gateway,
            flip_url,
            public_endpoint_id,
        )
    );
}

#[allow(clippy::too_many_arguments)]
fn ready_output(
    manifest: &Path,
    secrets: &Path,
    logs: &Path,
    fman_cli: &Path,
    fmans: &[(defe_client::ResourceLease, FmanInfo)],
    gateway: &GatewaydInfo,
    flip_url: &str,
    public_endpoint_id: &str,
) -> String {
    let mut output = String::new();
    writeln!(output, "defe env is ready").expect("write to string");
    writeln!(output, "Manifest:      {}", manifest.display()).expect("write to string");
    writeln!(output, "Secrets (0600): {}", secrets.display()).expect("write to string");
    writeln!(output, "Logs:          {}", logs.display()).expect("write to string");
    writeln!(output, "FLIP admin:    {flip_url}").expect("write to string");
    writeln!(output, "FLIP public endpoint ID: {public_endpoint_id}").expect("write to string");
    writeln!(output, "Gateway admin: {}", gateway.api_url).expect("write to string");
    writeln!(
        output,
        "Machine readiness: jq -e '.ready == true' {}",
        shell_escape(manifest.as_os_str())
    )
    .expect("write to string");
    writeln!(
        output,
        "FMan UI dependencies: pnpm --dir {} install --frozen-lockfile",
        shell_escape(std::ffi::OsStr::new(FMAN_OPERATOR_UI_DIR))
    )
    .expect("write to string");
    for (index, (_, fman)) in fmans.iter().enumerate() {
        let number = index + 1;
        writeln!(
            output,
            "FMan {number} operator UI (start one at a time): {FMAN_OPERATOR_UI_URL}"
        )
        .expect("write to string");
        writeln!(
            output,
            "        VITE_MOCKS=off FMAN_ADMIN_PROXY_TARGET={} pnpm --dir {} --filter fman exec vite --host 127.0.0.1",
            shell_escape(std::ffi::OsStr::new(&fman.admin_url)),
            shell_escape(std::ffi::OsStr::new(FMAN_OPERATOR_UI_DIR))
        )
        .expect("write to string");
        writeln!(
            output,
            "        FMan {number} operator UI password: {}",
            fman.admin_password
        )
        .expect("write to string");
        writeln!(
            output,
            "FMan {number} auth API (POST): {}",
            fman_api_url(&fman.admin_url, "auth")
        )
        .expect("write to string");
        writeln!(
            output,
            "FMan {number} admin API (POST): {}",
            fman_api_url(&fman.admin_url, "admin")
        )
        .expect("write to string");
        writeln!(
            output,
            "FMan {}: {} --data-dir {} seats list",
            number,
            shell_escape(fman_cli.as_os_str()),
            shell_escape(fman.data_dir.as_os_str())
        )
        .expect("write to string");
        writeln!(
            output,
            "        {} --data-dir {} plans show",
            shell_escape(fman_cli.as_os_str()),
            shell_escape(fman.data_dir.as_os_str())
        )
        .expect("write to string");
        writeln!(
            output,
            "        safe journal: {}",
            fman.data_dir.join("safe-events").display()
        )
        .expect("write to string");
    }
    writeln!(
        output,
        "Exit the environment shell or command to tear the environment down."
    )
    .expect("write to string");
    output
}

fn fman_api_url(base_url: &str, endpoint: &str) -> String {
    format!("{}/api/{endpoint}", base_url.trim_end_matches('/'))
}

fn shell_escape(value: &std::ffi::OsStr) -> String {
    let value = value
        .to_str()
        .expect("Defe validates every generated-wrapper value as UTF-8");
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn stopped_manifest(manifest: &Manifest<'_>) -> Result<serde_json::Value> {
    let mut stopped = serde_json::to_value(manifest)?;
    stopped["ready"] = false.into();
    stopped["state"] = "stopped".into();
    stopped["gateway"]["state"] = "stopped".into();
    stopped["flip"]["state"] = "stopped".into();
    Ok(stopped)
}

fn parse_args(args: Vec<std::ffi::OsString>) -> Result<Args> {
    let mut root = None;
    let mut logs_dir = None;
    let mut fi_cli = None;
    let mut gateway_cli = None;
    let mut bitcoin_cli = None;
    let mut fman_cli = None;
    let mut complete_liquidity = false;
    let mut command = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--root") => root = args.next().map(PathBuf::from),
            Some("--logs-dir") => logs_dir = args.next().map(PathBuf::from),
            Some("--fi-cli") => fi_cli = args.next().map(PathBuf::from),
            Some("--fman-cli") => fman_cli = args.next().map(PathBuf::from),
            Some("--gateway-cli") => gateway_cli = args.next().map(PathBuf::from),
            Some("--bitcoin-cli") => bitcoin_cli = args.next().map(PathBuf::from),
            Some("--complete-liquidity") => complete_liquidity = true,
            Some("--") => {
                command.extend(args);
                break;
            }
            Some("--help" | "-h") => {
                println!("Usage: defe env [--complete-liquidity] [-- COMMAND...]");
                std::process::exit(0);
            }
            _ => bail!("unrecognized defe env argument: {}", arg.to_string_lossy()),
        }
    }
    Ok(Args {
        root: root.context("internal --root argument is missing")?,
        logs_dir: logs_dir.context("internal --logs-dir argument is missing")?,
        fi_cli: fi_cli.context("internal --fi-cli argument is missing")?,
        fman_cli: fman_cli.context("internal --fman-cli argument is missing")?,
        gateway_cli: gateway_cli.context("internal --gateway-cli argument is missing")?,
        bitcoin_cli: bitcoin_cli.context("internal --bitcoin-cli argument is missing")?,
        complete_liquidity,
        command,
        pgid_fd: std::env::var(PGID_FD_ENV)
            .context("internal parent PGID channel is missing")?
            .parse()
            .context("internal parent PGID channel is invalid")?,
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::io::{Read as _, Write as _};
    use std::os::fd::FromRawFd as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::process::CommandExt as _;
    use std::os::unix::process::ExitStatusExt as _;
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::time::Duration;

    use super::{
        Args, BitcoinManifest, FederationManifest, FlipManifest, GatewayManifest, Manifest,
        exit_code, fman_api_url, ready_output, run_child, shell_escape, stopped_manifest,
        write_tools,
    };
    use defe_client::{
        BitcoindInfo, GatewaydInfo, ResourceDescriptor, ResourceHandleId, ResourceLease,
    };

    #[test]
    fn shell_escape_handles_spaces_and_single_quotes() {
        assert_eq!(
            shell_escape(std::ffi::OsStr::new("/tmp/a b'c")),
            "'/tmp/a b'\\''c'"
        );
    }

    #[test]
    fn signal_status_uses_the_conventional_shell_exit_code() {
        assert_eq!(
            exit_code(std::process::ExitStatus::from_raw(libc::SIGTERM)),
            143
        );
    }

    #[test]
    fn stopped_manifest_invalidates_every_live_state() {
        let manifest = Manifest {
            schema_version: 1,
            ready: true,
            state: "ready",
            federation: FederationManifest {
                invite_file: Path::new("/invite"),
                fi_state_dir: Path::new("/fi"),
                fi_account_file: Path::new("/fi/account"),
            },
            fmans: vec![],
            bitcoin: BitcoinManifest {
                rpc_url: "http://bitcoin",
                data_dir: Path::new("/bitcoin"),
            },
            nostr_relay_url: "ws://relay",
            gateway: GatewayManifest {
                api_url: "http://gateway",
                state: "connected",
            },
            flip: FlipManifest {
                admin_url: "http://flip",
                data_dir: Path::new("/flip"),
                state: "advertising",
                public_endpoint_id: "endpoint",
            },
            logs_dir: Path::new("/logs"),
            secrets_file: Path::new("/secrets"),
        };
        let stopped = stopped_manifest(&manifest).expect("serialize stopped manifest");
        assert_eq!(stopped["ready"], false);
        assert_eq!(stopped["state"], "stopped");
        assert_eq!(stopped["gateway"]["state"], "stopped");
        assert_eq!(stopped["flip"]["state"], "stopped");
    }

    #[test]
    fn ready_output_gives_each_fman_an_attachable_ui_and_exact_api_routes() {
        let fman = defe_client::FmanInfo {
            locator: "locator".to_owned(),
            data_dir: PathBuf::from("/tmp/fman"),
            iroh_connect_overrides: String::new(),
            admin_url: "http://127.0.0.1:10612".to_owned(),
            admin_password: "fman-secret".to_owned(),
        };
        let lease = ResourceLease {
            handle_id: ResourceHandleId(1),
            descriptor: ResourceDescriptor::Fman(fman.clone()),
        };
        let output = ready_output(
            Path::new("/tmp/env/env.json"),
            Path::new("/tmp/env/secrets.json"),
            Path::new("/tmp/env/logs"),
            Path::new("/tmp/fman-cli"),
            &[(lease, fman)],
            &GatewaydInfo {
                api_url: "http://gateway".to_owned(),
                password: "gateway-secret".to_owned(),
            },
            "http://flip",
            "endpoint",
        );

        assert!(output.contains("FMan 1 operator UI (start one at a time): http://127.0.0.1:5174"));
        assert!(output.contains(
            "VITE_MOCKS=off FMAN_ADMIN_PROXY_TARGET='http://127.0.0.1:10612' pnpm --dir "
        ));
        assert!(output.contains("--filter fman exec vite --host 127.0.0.1"));
        assert!(output.contains("FMan 1 operator UI password: fman-secret"));
        assert!(output.contains("FMan 1 auth API (POST): http://127.0.0.1:10612/api/auth"));
        assert!(output.contains("FMan 1 admin API (POST): http://127.0.0.1:10612/api/admin"));
        assert!(!output.contains("FMan 1 admin: http://127.0.0.1:10612"));
        assert!(!output.contains("gateway-secret"));
    }

    #[test]
    fn fman_api_urls_name_exact_post_endpoints() {
        assert_eq!(
            fman_api_url("http://127.0.0.1:10612/", "auth"),
            "http://127.0.0.1:10612/api/auth"
        );
        assert_eq!(
            fman_api_url("http://127.0.0.1:10612", "admin"),
            "http://127.0.0.1:10612/api/admin"
        );
    }

    #[test]
    fn generated_wrappers_forward_exact_arguments_and_preserve_fee_failures() {
        let root = std::env::temp_dir().join(format!("defe-wrapper-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let recorder = root.join("record command");
        std::fs::write(
            &recorder,
            "#!/bin/sh\nprintf 'args' >>\"$RECORD\"\nprevious=\nfailed=0\nfor arg in \"$@\"; do printf '[%s]' \"$arg\" >>\"$RECORD\"; [ \"$previous $arg\" = 'guardian-fees collect' ] && failed=1; previous=$arg; done\nprintf ' env=[%s][%s]\\n' \"${FMAN_E2E_LOCAL_IROH-}\" \"${FM_IROH_CONNECT_OVERRIDES-}\" >>\"$RECORD\"\nif [ \"$failed\" -eq 1 ]; then exit 23; fi\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&recorder, std::fs::Permissions::from_mode(0o700)).unwrap();
        let record = root.join("record");
        let args = Args {
            root: root.clone(),
            logs_dir: root.join("logs"),
            fi_cli: recorder.clone(),
            fman_cli: recorder.clone(),
            gateway_cli: recorder.clone(),
            bitcoin_cli: recorder.clone(),
            complete_liquidity: false,
            command: vec![],
            pgid_fd: -1,
        };
        let fman = defe_client::FmanInfo {
            locator: "locator".into(),
            data_dir: root.join("fman data"),
            iroh_connect_overrides: "routes".into(),
            admin_url: "http://fman".into(),
            admin_password: "fman-secret".into(),
        };
        let fmans = vec![(
            ResourceLease {
                handle_id: ResourceHandleId(1),
                descriptor: ResourceDescriptor::Fman(fman.clone()),
            },
            fman,
        )];
        let gateway = GatewaydInfo {
            api_url: "http://gateway".into(),
            password: "gateway-secret".into(),
        };
        let bitcoin = BitcoindInfo {
            rpc_url: "http://bitcoin".into(),
            rpc_host: "127.0.0.1".into(),
            rpc_port: 18443,
            p2p_port: 18444,
            rpc_username: "bitcoin-user".into(),
            rpc_password: "bitcoin-password".into(),
            data_dir: root.join("bitcoin data"),
        };
        let bin = root.join("bin");
        write_tools(
            &bin,
            &args,
            &root.join("env.json"),
            &root.join("secrets.json"),
            &root.join("invite"),
            &root.join("fi state"),
            "route one,route two",
            &fmans,
            &["seat-one".into()],
            &gateway,
            &bitcoin,
        )
        .unwrap();

        let run = |tool: &str, arguments: &[&str]| {
            std::process::Command::new(bin.join(tool))
                .args(arguments)
                .env("RECORD", &record)
                .status()
                .unwrap()
        };
        assert!(run("fman-1", &["alpha", "two words"]).success());
        assert!(run("fi-cli", &["status", "two words"]).success());
        assert!(run("gateway", &["info", "two words"]).success());
        assert!(run("bitcoin-cli", &["getblockchaininfo", "two words"]).success());
        let collect = run("fees", &["collect", "--guardian", "1"]);
        assert_eq!(collect.code(), Some(23));
        let before_mixed = std::fs::read(&record).unwrap();
        assert_eq!(
            run("fees", &["collect", "--guardian", "1", "--all"]).code(),
            Some(2)
        );
        assert_eq!(std::fs::read(&record).unwrap(), before_mixed);

        let recorded = std::fs::read_to_string(&record).unwrap();
        assert!(recorded.contains("args[--data-dir]["));
        assert!(recorded.contains("][alpha][two words]"));
        assert!(recorded.contains("args[--state-dir]["));
        assert!(recorded.contains("][status][two words] env=[1][route one,route two]"));
        assert!(recorded.contains(
            "args[--address][http://gateway][--rpcpassword=gateway-secret][info][two words]"
        ));
        assert!(recorded.contains("args[-regtest][-datadir=",));
        assert!(recorded.contains(
            "][-rpcuser=bitcoin-user][-rpcpassword=bitcoin-password][-rpcport=18443][getblockchaininfo][two words]"
        ));
        assert!(recorded.contains("args[--data-dir]["));
        assert!(recorded.contains("][guardian-fees][collect][seat-one]"));
        assert!(recorded.contains("][guardian-fees][show][seat-one]"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pty_foreground_interrupt_returns_to_shell_and_preserves_shell_status() {
        let mut master = 0;
        let mut slave = 0;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        assert_ne!(
            unsafe { libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC) },
            -1
        );
        let mut master = unsafe { std::fs::File::from_raw_fd(master) };
        let slave = unsafe { std::fs::File::from_raw_fd(slave) };
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "tests::pty_launcher_helper",
                "--nocapture",
            ])
            .env("DEFE_ENV_PTY_HELPER", "1")
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut helper = command.spawn().unwrap();
        drop(command);
        std::thread::sleep(Duration::from_millis(300));
        master.write_all(b"sleep 30\n").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        master.write_all(b"\x03").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        master
            .write_all(b"echo DEFE_SHELL_SURVIVED\nexit 7\n")
            .unwrap();
        let status = helper.wait().unwrap();
        let mut output = String::new();
        match master.read_to_string(&mut output) {
            Ok(_) => {}
            Err(error) if error.raw_os_error() == Some(libc::EIO) => {}
            Err(error) => panic!("read PTY output: {error}"),
        }
        assert_eq!(status.code(), Some(7), "helper output:\n{output}");
        assert!(
            output.contains("DEFE_SHELL_SURVIVED"),
            "helper output:\n{output}"
        );
    }

    #[cfg(target_os = "linux")]
    #[ignore = "spawned by pty_foreground_interrupt_returns_to_shell_and_preserves_shell_status"]
    #[tokio::test]
    async fn pty_launcher_helper() {
        if std::env::var_os("DEFE_ENV_PTY_HELPER").is_none() {
            return;
        }
        let root = std::env::temp_dir().join(format!("defe-env-pty-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut pgid_pipe = [0_i32; 2];
        assert_eq!(unsafe { libc::pipe(pgid_pipe.as_mut_ptr()) }, 0);
        assert_ne!(
            unsafe { libc::fcntl(pgid_pipe[0], libc::F_SETFD, libc::FD_CLOEXEC) },
            -1
        );
        let args = Args {
            root: root.clone(),
            logs_dir: root.join("logs"),
            fi_cli: "/bin/false".into(),
            fman_cli: "/bin/false".into(),
            gateway_cli: "/bin/false".into(),
            bitcoin_cli: "/bin/false".into(),
            complete_liquidity: false,
            command: vec![OsString::from("sh"), OsString::from("-i")],
            pgid_fd: pgid_pipe[1],
        };
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        let status = run_child(
            &args.command,
            &args,
            &root.join("env.json"),
            &root.join("secrets.json"),
            &root.join("invite"),
            &root.join("fi"),
            &root.join("routes"),
            &root.join("bin"),
            "ws://relay",
            "http://gateway",
            "http://flip",
            "flip-id",
            &mut interrupt,
            &mut terminate,
        )
        .await
        .unwrap();
        let mut pgid_reader = unsafe { std::fs::File::from_raw_fd(pgid_pipe[0]) };
        let mut published_pgid = String::new();
        pgid_reader.read_to_string(&mut published_pgid).unwrap();
        assert!(!published_pgid.is_empty());
        std::process::exit(exit_code(status));
    }

    #[tokio::test]
    async fn external_termination_reaps_the_environment_process_group() {
        #[cfg(target_os = "linux")]
        assert_eq!(
            unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
            0
        );
        let mut child = tokio::process::Command::new("sh");
        child
            .args([
                "-c",
                "trap 'exit 0' TERM; (trap '' TERM; while :; do sleep 1; done) & wait",
            ])
            .as_std_mut()
            .process_group(0);
        let mut child = child.spawn().unwrap();
        let group = i32::try_from(child.id().unwrap()).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let status = super::terminate_child_group(&mut child, group)
            .await
            .unwrap();
        super::drain_child_group(group).await.unwrap();
        assert!(status.success());
        assert_eq!(unsafe { libc::kill(-group, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}
