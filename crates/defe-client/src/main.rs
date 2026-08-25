use std::env;
use std::ffi::{OsStr, OsString};
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use defe_api::{BitcoindInfo, BitcoindRequest, FmanInfo};
use defe_api::{
    DEV_DEFE_SOCKET_PATH, NostrRelayInfo, NostrRelayRequest, PushGatewayInfo, PushGatewayRequest,
    ResourceDescriptor, ResourceLease, ResourceRequest, SharingMode,
};
use defe_client::AsyncDefeClient;

const DEV_DEFE_NOSTR_RELAY_URL: &str = "DEV_DEFE_NOSTR_RELAY_URL";
const DEV_DEFE_NOSTR_RELAY_PORT: &str = "DEV_DEFE_NOSTR_RELAY_PORT";
const DEV_DEFE_NOSTR_RELAY_DATA_DIR: &str = "DEV_DEFE_NOSTR_RELAY_DATA_DIR";
const DEV_DEFE_PUSH_GATEWAY_URL: &str = "DEV_DEFE_PUSH_GATEWAY_URL";
const DEV_DEFE_PUSH_GATEWAY_PORT: &str = "DEV_DEFE_PUSH_GATEWAY_PORT";
const DEV_DEFE_PUSH_GATEWAY_APP_ID: &str = "DEV_DEFE_PUSH_GATEWAY_APP_ID";
const DEV_DEFE_PUSH_GATEWAY_DATABASE_PATH: &str = "DEV_DEFE_PUSH_GATEWAY_DATABASE_PATH";
const DEV_DEFE_BITCOIND_URL: &str = "DEV_DEFE_BITCOIND_URL";
const DEV_DEFE_BITCOIND_RPC_HOST: &str = "DEV_DEFE_BITCOIND_RPC_HOST";
const DEV_DEFE_BITCOIND_RPC_PORT: &str = "DEV_DEFE_BITCOIND_RPC_PORT";
const DEV_DEFE_BITCOIND_P2P_PORT: &str = "DEV_DEFE_BITCOIND_P2P_PORT";
const DEV_DEFE_BITCOIND_RPC_USERNAME: &str = "DEV_DEFE_BITCOIND_RPC_USERNAME";
const DEV_DEFE_BITCOIND_RPC_PASSWORD: &str = "DEV_DEFE_BITCOIND_RPC_PASSWORD";
const DEV_DEFE_BITCOIND_DATA_DIR: &str = "DEV_DEFE_BITCOIND_DATA_DIR";
const DEV_DEFE_FMAN_LOCATOR: &str = "DEV_DEFE_FMAN_LOCATOR";
const DEV_DEFE_FMAN_DATA_DIR: &str = "DEV_DEFE_FMAN_DATA_DIR";

const USAGE: &str = "Usage: defe-cli [opts...] <cmd...>\n       defe-cli --request-relay[=shared|exclusive] [--] <cmd...>\n       defe-cli --request-push-gateway[=shared|exclusive] [--] <cmd...>\n       defe-cli --request-bitcoind[=shared|exclusive] [--] <cmd...>\n       defe-cli ping";

#[tokio::main]
async fn main() {
    let args = env::args_os().skip(1).collect::<Vec<_>>();

    match run(args).await {
        Ok(code) => std::process::exit(code),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}

async fn run(args: Vec<OsString>) -> Result<i32, String> {
    match parse_args(args)? {
        CliCommand::Help => {
            println!("{USAGE}");
            Ok(0)
        }
        CliCommand::Ping => ping().await.map(|()| 0),
        CliCommand::Wrap(wrapper) => run_wrapper(wrapper).await,
    }
}

fn parse_args(args: Vec<OsString>) -> Result<CliCommand, String> {
    if args.is_empty() || args.first().is_some_and(|arg| is_help_arg(arg)) {
        return Ok(CliCommand::Help);
    }

    if args.first().is_some_and(|arg| arg == "ping") {
        if args.len() != 1 {
            return Err(format!(
                "defe-cli ping does not accept extra arguments\n{USAGE}"
            ));
        }
        return Ok(CliCommand::Ping);
    }

    parse_wrapper_args(args).map(CliCommand::Wrap)
}

fn parse_wrapper_args(args: Vec<OsString>) -> Result<WrapperArgs, String> {
    let mut requests = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }

        match parse_resource_request_arg(arg)? {
            Some(request) => {
                requests.push(request);
                index += 1;
            }
            None => break,
        }
    }

    let command = args[index..].to_vec();
    if command.is_empty() {
        return Err(format!("defe-cli requires a command\n{USAGE}"));
    }

    Ok(WrapperArgs { requests, command })
}

fn parse_resource_request_arg(arg: &OsStr) -> Result<Option<ResourceRequestArg>, String> {
    let Some(arg) = arg.to_str() else {
        return Ok(None);
    };

    match arg {
        "--request-relay" | "--request-relay=shared" => Ok(Some(ResourceRequestArg::NostrRelay {
            sharing: SharingMode::Shared,
        })),
        "--request-relay=exclusive" => Ok(Some(ResourceRequestArg::NostrRelay {
            sharing: SharingMode::Exclusive,
        })),
        _ if arg.starts_with("--request-relay=") => Err(format!(
            "unsupported --request-relay mode: {}\n{USAGE}",
            arg.trim_start_matches("--request-relay=")
        )),
        "--request-push-gateway" | "--request-push-gateway=shared" => {
            Ok(Some(ResourceRequestArg::PushGateway {
                sharing: SharingMode::Shared,
            }))
        }
        "--request-push-gateway=exclusive" => Ok(Some(ResourceRequestArg::PushGateway {
            sharing: SharingMode::Exclusive,
        })),
        _ if arg.starts_with("--request-push-gateway=") => Err(format!(
            "unsupported --request-push-gateway mode: {}\n{USAGE}",
            arg.trim_start_matches("--request-push-gateway=")
        )),
        "--request-bitcoind" | "--request-bitcoind=shared" => {
            Ok(Some(ResourceRequestArg::Bitcoind {
                sharing: SharingMode::Shared,
            }))
        }
        "--request-bitcoind=exclusive" => Ok(Some(ResourceRequestArg::Bitcoind {
            sharing: SharingMode::Exclusive,
        })),
        _ if arg.starts_with("--request-bitcoind=") => Err(format!(
            "unsupported --request-bitcoind mode: {}\n{USAGE}",
            arg.trim_start_matches("--request-bitcoind=")
        )),
        _ if arg.starts_with("--request-") => Err(format!(
            "unsupported resource request option: {arg}\n{USAGE}"
        )),
        _ => Ok(None),
    }
}

async fn run_wrapper(wrapper: WrapperArgs) -> Result<i32, String> {
    let socket_path = if wrapper.requests.is_empty() {
        None
    } else {
        Some(
            env::var_os(DEV_DEFE_SOCKET_PATH).ok_or_else(|| {
                format!(
                    "{DEV_DEFE_SOCKET_PATH} is not set; run this command inside `defe exec <cmd...>` or set {DEV_DEFE_SOCKET_PATH} to a defe server Unix socket path"
                )
            })?,
        )
    };
    run_wrapper_with_socket(wrapper, socket_path).await
}

async fn run_wrapper_with_socket(
    wrapper: WrapperArgs,
    socket_path: Option<OsString>,
) -> Result<i32, String> {
    let mut client =
        if wrapper.requests.is_empty() {
            None
        } else {
            Some(
                AsyncDefeClient::connect(socket_path.ok_or_else(|| {
                    "defe socket path is required for resource requests".to_owned()
                })?)
                .await
                .map_err(|err| err.to_string())?,
            )
        };

    let mut child_env = Vec::new();
    let mut leases = Vec::new();
    if let Some(client) = &mut client {
        for request in &wrapper.requests {
            let lease = client
                .allocate(request.to_api_request())
                .await
                .map_err(|err| err.to_string())?;
            add_resource_env(&lease, &mut child_env);
            leases.push(lease);
        }
    }

    let mut command = tokio::process::Command::new(&wrapper.command[0]);
    command.args(&wrapper.command[1..]);
    command.envs(child_env);
    let status = command.status().await.map_err(|err| {
        format!(
            "failed to run {}: {err}",
            wrapper.command[0].to_string_lossy()
        )
    })?;

    drop(leases);
    drop(client);

    Ok(exit_code_from_status(status))
}

fn add_resource_env(lease: &ResourceLease, child_env: &mut Vec<(&'static str, OsString)>) {
    match &lease.descriptor {
        ResourceDescriptor::NostrRelay(info) => add_nostr_relay_env(info, child_env),
        ResourceDescriptor::PushGateway(info) => add_push_gateway_env(info, child_env),
        ResourceDescriptor::Bitcoind(info) => add_bitcoind_env(info, child_env),
        ResourceDescriptor::Fman(info) => add_fman_env(info, child_env),
        // FLIP needs no generic wrapper form: its fixture and setup inputs are
        // intentionally supplied by the consuming integration test.
        ResourceDescriptor::Flip(_) => {}
        ResourceDescriptor::Gatewayd(_) => {}
    }
}

fn add_nostr_relay_env(info: &NostrRelayInfo, child_env: &mut Vec<(&'static str, OsString)>) {
    child_env.push((DEV_DEFE_NOSTR_RELAY_URL, OsString::from(&info.url)));
    child_env.push((
        DEV_DEFE_NOSTR_RELAY_PORT,
        OsString::from(info.port.to_string()),
    ));
    child_env.push((
        DEV_DEFE_NOSTR_RELAY_DATA_DIR,
        info.data_dir.as_os_str().to_owned(),
    ));
}

fn add_push_gateway_env(info: &PushGatewayInfo, child_env: &mut Vec<(&'static str, OsString)>) {
    child_env.push((DEV_DEFE_PUSH_GATEWAY_URL, OsString::from(&info.url)));
    child_env.push((
        DEV_DEFE_PUSH_GATEWAY_PORT,
        OsString::from(info.port.to_string()),
    ));
    child_env.push((DEV_DEFE_PUSH_GATEWAY_APP_ID, OsString::from(&info.app_id)));
    child_env.push((
        DEV_DEFE_PUSH_GATEWAY_DATABASE_PATH,
        info.database_path.as_os_str().to_owned(),
    ));
}

fn add_bitcoind_env(info: &BitcoindInfo, child_env: &mut Vec<(&'static str, OsString)>) {
    child_env.push((DEV_DEFE_BITCOIND_URL, OsString::from(&info.rpc_url)));
    child_env.push((DEV_DEFE_BITCOIND_RPC_HOST, OsString::from(&info.rpc_host)));
    child_env.push((
        DEV_DEFE_BITCOIND_RPC_PORT,
        OsString::from(info.rpc_port.to_string()),
    ));
    child_env.push((
        DEV_DEFE_BITCOIND_P2P_PORT,
        OsString::from(info.p2p_port.to_string()),
    ));
    child_env.push((
        DEV_DEFE_BITCOIND_RPC_USERNAME,
        OsString::from(&info.rpc_username),
    ));
    child_env.push((
        DEV_DEFE_BITCOIND_RPC_PASSWORD,
        OsString::from(&info.rpc_password),
    ));
    child_env.push((
        DEV_DEFE_BITCOIND_DATA_DIR,
        info.data_dir.as_os_str().to_owned(),
    ));
}

fn add_fman_env(info: &FmanInfo, child_env: &mut Vec<(&'static str, OsString)>) {
    child_env.push((DEV_DEFE_FMAN_LOCATOR, OsString::from(&info.locator)));
    child_env.push((DEV_DEFE_FMAN_DATA_DIR, info.data_dir.as_os_str().to_owned()));
}

async fn ping() -> Result<(), String> {
    let mut client = AsyncDefeClient::connect_from_env()
        .await
        .map_err(|err| err.to_string())?;
    client.ping().await.map_err(|err| err.to_string())?;
    println!("pong");
    Ok(())
}

fn is_help_arg(arg: &OsStr) -> bool {
    arg == "--help" || arg == "-h"
}

#[cfg(unix)]
fn exit_code_from_status(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }

    128 + status.signal().unwrap_or(1)
}

#[derive(Debug)]
enum CliCommand {
    Help,
    Ping,
    Wrap(WrapperArgs),
}

#[derive(Debug)]
struct WrapperArgs {
    requests: Vec<ResourceRequestArg>,
    command: Vec<OsString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceRequestArg {
    NostrRelay { sharing: SharingMode },
    PushGateway { sharing: SharingMode },
    Bitcoind { sharing: SharingMode },
}

impl ResourceRequestArg {
    fn to_api_request(self) -> ResourceRequest {
        match self {
            ResourceRequestArg::NostrRelay { sharing } => {
                ResourceRequest::NostrRelay(NostrRelayRequest { sharing })
            }
            ResourceRequestArg::PushGateway { sharing } => {
                ResourceRequest::PushGateway(PushGatewayRequest { sharing })
            }
            ResourceRequestArg::Bitcoind { sharing } => {
                ResourceRequest::Bitcoind(BitcoindRequest { sharing })
            }
        }
    }
}

#[cfg(test)]
mod main_tests;
#[cfg(test)]
mod test_support;
