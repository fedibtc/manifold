//! Keeps a formed local federation and its supporting services alive for manual use.

use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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

#[derive(Debug)]
struct Args {
    root: PathBuf,
    logs_dir: PathBuf,
    fi_cli: PathBuf,
    fman_cli: PathBuf,
    gateway_cli: PathBuf,
    complete_liquidity: bool,
}

#[derive(Serialize)]
struct Manifest<'a> {
    ready: bool,
    state: &'static str,
    federation: FederationManifest<'a>,
    fmans: Vec<FmanManifest<'a>>,
    gateway: GatewayManifest<'a>,
    flip: FlipManifest<'a>,
    logs_dir: &'a Path,
    secrets_file: &'a Path,
}

#[derive(Serialize)]
struct FederationManifest<'a> {
    invite_file: &'a Path,
    fi_state_dir: &'a Path,
}

#[derive(Serialize)]
struct FmanManifest<'a> {
    api_base_url: &'a str,
    auth_url: String,
    admin_url: String,
    data_dir: &'a Path,
    safe_journal_dir: PathBuf,
}

#[derive(Serialize)]
struct GatewayManifest<'a> {
    api_url: &'a str,
    state: &'static str,
}

#[derive(Serialize)]
struct FlipManifest<'a> {
    admin_url: &'a str,
    data_dir: &'a Path,
    state: &'static str,
    public_endpoint_id: &'a str,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args(std::env::args_os().skip(1).collect())?;
    if args.complete_liquidity {
        bail!(
            "--complete-liquidity is not implemented yet; basic staging remains available without it"
        );
    }
    run(args).await
}

async fn run(args: Args) -> Result<()> {
    fs::create_dir_all(&args.root)
        .with_context(|| format!("create staging root {}", args.root.display()))?;
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
            iroh_connect_overrides: Some(routes),
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
        ready: true,
        state: "ready",
        federation: FederationManifest {
            invite_file: &invite_file,
            fi_state_dir: &fi_state_dir,
        },
        fmans: fmans
            .iter()
            .map(|(_, fman)| FmanManifest {
                api_base_url: &fman.admin_url,
                auth_url: fman_api_url(&fman.admin_url, "auth"),
                admin_url: fman_api_url(&fman.admin_url, "admin"),
                data_dir: &fman.data_dir,
                safe_journal_dir: fman.data_dir.join("safe-events"),
            })
            .collect(),
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
    let manifest_file = args.root.join("env.json");
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

    tokio::signal::ctrl_c().await.context("wait for Ctrl-C")?;
    let stopped_manifest = stopped_manifest(&manifest)?;
    write_json_atomic(&manifest_file, &stopped_manifest)?;
    eprintln!("defe staging: Ctrl-C received; releasing all leased resources");
    drop((
        flip_lease,
        gateway_lease,
        fmans,
        relay_lease,
        bitcoin_lease,
        defe,
    ));
    Ok(())
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
    eprintln!("defe staging: {message}...");
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
    writeln!(output, "defe staging is ready").expect("write to string");
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
    writeln!(output, "Press Ctrl-C to tear the environment down.").expect("write to string");
    output
}

fn fman_api_url(base_url: &str, endpoint: &str) -> String {
    format!("{}/api/{endpoint}", base_url.trim_end_matches('/'))
}

fn shell_escape(value: &std::ffi::OsStr) -> String {
    let value = value.to_string_lossy();
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
    let mut fman_cli = None;
    let mut complete_liquidity = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--root") => root = args.next().map(PathBuf::from),
            Some("--logs-dir") => logs_dir = args.next().map(PathBuf::from),
            Some("--fi-cli") => fi_cli = args.next().map(PathBuf::from),
            Some("--fman-cli") => fman_cli = args.next().map(PathBuf::from),
            Some("--gateway-cli") => gateway_cli = args.next().map(PathBuf::from),
            Some("--complete-liquidity") => complete_liquidity = true,
            Some("--help" | "-h") => {
                println!("Usage: defe staging [--complete-liquidity]");
                std::process::exit(0);
            }
            _ => bail!(
                "unrecognized defe staging argument: {}",
                arg.to_string_lossy()
            ),
        }
    }
    Ok(Args {
        root: root.context("internal --root argument is missing")?,
        logs_dir: logs_dir.context("internal --logs-dir argument is missing")?,
        fi_cli: fi_cli.context("internal --fi-cli argument is missing")?,
        fman_cli: fman_cli.context("internal --fman-cli argument is missing")?,
        gateway_cli: gateway_cli.context("internal --gateway-cli argument is missing")?,
        complete_liquidity,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;

    use super::{
        FederationManifest, FlipManifest, GatewayManifest, Manifest, fman_api_url, ready_output,
        shell_escape, stopped_manifest,
    };
    use defe_client::{GatewaydInfo, ResourceDescriptor, ResourceHandleId, ResourceLease};

    #[test]
    fn shell_escape_handles_spaces_and_single_quotes() {
        assert_eq!(
            shell_escape(std::ffi::OsStr::new("/tmp/a b'c")),
            "'/tmp/a b'\\''c'"
        );
    }

    #[test]
    fn stopped_manifest_invalidates_every_live_state() {
        let manifest = Manifest {
            ready: true,
            state: "ready",
            federation: FederationManifest {
                invite_file: Path::new("/invite"),
                fi_state_dir: Path::new("/fi"),
            },
            fmans: vec![],
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
            Path::new("/tmp/staging/env.json"),
            Path::new("/tmp/staging/secrets.json"),
            Path::new("/tmp/staging/logs"),
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
}
