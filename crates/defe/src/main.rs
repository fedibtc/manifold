use defe::bitcoind::BitcoindDriver;
use defe::flip::FlipDriver;
use defe::fman::FmanDriver;
use defe::gatewayd::GatewaydDriver;
use defe::nostr_relay::NostrRelayDriver;
use defe::push_gateway::PushGatewayDriver;
use defe::resource_manager::{
    ManagedResource, ResourceAllocation, ResourceConnection, ResourceDriver, ResourceKind,
    ResourceManager, ResourceSharing, ResourceSpec, SharedResourceKey,
};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Read as _;
use std::os::fd::FromRawFd as _;
use std::os::unix::fs::{DirBuilderExt as _, FileTypeExt as _, PermissionsExt as _};
use std::os::unix::net::{UnixListener as StdUnixListener, UnixStream as StdUnixStream};
use std::os::unix::process::{CommandExt as _, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, Command};
use tokio::sync::watch;

use defe_api::{
    ApiError, ApiErrorKind, DEV_DEFE_SOCKET_PATH, MAX_FRAME_SIZE, Request, ResourceRequest,
    Response, SharingMode,
};

const USAGE: &str = "Usage: defe [opts...] <command>\n\nCommands:\n  exec <cmd...>\n  env [--fedimint-load-test-tool-bin <path>] [--complete-liquidity] [-- COMMAND...]\n  serve --listenfd [--log-requests]\n\nOptions:\n  --tmp-dir <path>\n  --keep-temp\n  --no-keep-temp-on-failure\n  --log-dir <path>\n  --log-requests\n  --binary-path <dir>\n  --nostr-rs-relay-bin <path>\n  --push-gateway-bin <path>\n  --bitcoind-bin <path>\n  --fleet-manager-bin <path>\n  --fman-cli-bin <path>\n  --fi-cli-bin <path>\n  --liquidity-manager-daemon-bin <path>\n  --gatewayd-bin <path>\n  --gateway-cli-bin <path>\n  --defe-env-bin <path>";
const SOCKET_FILE_NAME: &str = "s";
const DEV_SERVER_TEMP_DIR_NAME: &str = "defe-dev-server";
const DEFAULT_TEMP_DIR_ATTEMPTS: u16 = 256;
const PGID_FD_ENV: &str = "DEV_DEFE_ENV_PGID_FD";

fn main() {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build defe async runtime");

    match runtime.block_on(run(args)) {
        Ok(code) => std::process::exit(code),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}

async fn run(args: Vec<OsString>) -> Result<i32, String> {
    if args.is_empty()
        || args
            .first()
            .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!("{USAGE}");
        return Ok(0);
    }

    let command = parse_command(args)?;
    match command {
        DefeCommand::Exec(exec) => run_exec(exec).await.map(exit_code_from_status),
        DefeCommand::Serve(serve) => run_serve(serve).await.map(|()| 0),
    }
}

async fn run_exec(exec: ExecArgs) -> Result<ExitStatus, String> {
    if exec.command.is_empty() && exec.environment.is_none() {
        return Err(format!("defe exec requires a command\n{USAGE}"));
    }

    let environment = exec.environment.is_some();
    let load_test_tool_bin = resolve_binary(
        exec.environment
            .as_ref()
            .and_then(|environment| environment.load_test_tool_bin.clone()),
        &exec.options.binary_paths,
        "fedimint-load-test-tool",
    );
    let log_requests = exec.options.log_requests;
    let mut generated_temp_root = None;
    let config = match prepare_server_config(exec.options, || {
        let temp_root = default_temp_root()?;
        generated_temp_root = Some(temp_root.clone());
        Ok(temp_root)
    }) {
        Ok(config) => config,
        Err(err) => {
            if let Some(temp_root) = generated_temp_root {
                let _ = fs::remove_dir_all(temp_root);
            }
            return Err(err);
        }
    };
    let mut command_args = exec.command;
    if let Some(environment) = exec.environment {
        command_args = vec![
            config.defe_env_bin.clone(),
            "--root".into(),
            config.temp_root.join("env").into_os_string(),
            "--logs-dir".into(),
            config.log_dir.clone().into_os_string(),
            "--fi-cli".into(),
            config.fi_cli_bin.clone(),
            "--fman-cli".into(),
            config.fman_cli_bin.clone(),
            "--gateway-cli".into(),
            config.gateway_cli_bin.clone(),
            "--bitcoin-cli".into(),
            config.bitcoin_cli_bin.clone(),
            "--load-test-tool".into(),
            load_test_tool_bin.clone(),
        ];
        command_args.extend(environment.args);
        for (label, binary) in [
            ("defe-env", &config.defe_env_bin),
            ("fman-cli", &config.fman_cli_bin),
            ("fi-cli", &config.fi_cli_bin),
            ("gateway-cli", &config.gateway_cli_bin),
            ("bitcoin-cli", &config.bitcoin_cli_bin),
            ("fedimint-load-test-tool", &load_test_tool_bin),
        ] {
            validate_environment_binary(label, binary)?;
        }
    }
    let resource_manager = create_resource_manager(&config);
    let request_logger = RequestLogger::new(log_requests);

    let socket_path = config.temp_root.join(SOCKET_FILE_NAME);
    remove_stale_socket(&socket_path)?;
    let listener = StdUnixListener::bind(&socket_path)
        .map_err(|err| format!("failed to bind socket {}: {err}", socket_path.display()))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| format!("failed to set socket nonblocking: {err}"))?;
    let listener = UnixListener::from_std(listener)
        .map_err(|err| format!("failed to create async socket listener: {err}"))?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let accept_task = tokio::spawn(accept_loop(
        listener,
        shutdown_rx.clone(),
        Arc::clone(&resource_manager),
        Arc::clone(&request_logger),
    ));

    install_shutdown_handler(shutdown_tx.clone())?;

    let mut command = Command::new(&command_args[0]);
    command.args(&command_args[1..]);
    command.env(DEV_DEFE_SOCKET_PATH, &socket_path);
    let environment_manifest = environment.then(|| config.temp_root.join("env/env.json"));
    let mut pgid_pipe = None;
    if environment {
        let mut pipe = [0_i32; 2];
        if unsafe { libc::pipe(pipe.as_mut_ptr()) } != 0 {
            return Err(format!(
                "failed to create environment PGID channel: {}",
                std::io::Error::last_os_error()
            ));
        }
        command.env(PGID_FD_ENV, pipe[1].to_string());
        unsafe {
            command.as_std_mut().pre_exec(move || {
                libc::close(pipe[0]);
                Ok(())
            });
        }
        pgid_pipe = Some(pipe);
    }
    let status = match command.spawn() {
        Ok(mut child) => {
            let mut pgid_reader = pgid_pipe.map(|pipe| {
                unsafe { libc::close(pipe[1]) };
                unsafe { libc::fcntl(pipe[0], libc::F_SETFL, libc::O_NONBLOCK) };
                unsafe { fs::File::from_raw_fd(pipe[0]) }
            });
            wait_for_exec_child(
                &mut child,
                shutdown_rx.clone(),
                environment_manifest.as_deref(),
                pgid_reader.as_mut(),
            )
            .await
        }
        Err(err) => {
            if let Some(pipe) = pgid_pipe {
                unsafe {
                    libc::close(pipe[0]);
                    libc::close(pipe[1]);
                }
            }
            let _ = shutdown_tx.send(true);
            let _ = accept_task.await;
            resource_manager.shutdown();
            if let Err(cleanup_err) = cleanup_temp(&config.temp_root, &exec.policy, false) {
                eprintln!("defe: {cleanup_err}");
            }
            return Err(format!(
                "failed to run {}: {err}",
                command_args[0].to_string_lossy()
            ));
        }
    }
    .map_err(|err| {
        format!(
            "failed to wait for {}: {err}",
            command_args[0].to_string_lossy()
        )
    })?;

    let _ = shutdown_tx.send(true);
    let _ = accept_task.await;
    resource_manager.shutdown();

    if let Err(err) = cleanup_temp(&config.temp_root, &exec.policy, status.success()) {
        eprintln!("defe: {err}");
    }

    Ok(status)
}

async fn run_serve(serve: ServeArgs) -> Result<(), String> {
    if !serve.listenfd {
        return Err(format!("defe serve requires --listenfd\n{USAGE}"));
    }

    let listener = take_listenfd_unix_listener()?;
    listener
        .set_nonblocking(true)
        .map_err(|err| format!("failed to set listenfd socket nonblocking: {err}"))?;
    let listener = UnixListener::from_std(listener)
        .map_err(|err| format!("failed to create async listenfd socket listener: {err}"))?;

    let log_requests = serve.options.log_requests;
    let config = prepare_server_config(serve.options, || Ok(default_serve_temp_root()))?;
    let resource_manager = create_resource_manager(&config);
    let request_logger = RequestLogger::new(log_requests);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    install_shutdown_handler(shutdown_tx)?;
    eprintln!(
        "defe: serving listenfd socket with temp root {} and log dir {}",
        config.temp_root.display(),
        config.log_dir.display()
    );
    accept_loop(
        listener,
        shutdown_rx,
        Arc::clone(&resource_manager),
        Arc::clone(&request_logger),
    )
    .await;
    resource_manager.shutdown();
    Ok(())
}

fn create_resource_manager(config: &ServerConfig) -> Arc<ResourceManager> {
    Arc::new(ResourceManager::new(Arc::new(DefeResourceDriver {
        nostr_relay: NostrRelayDriver::new(
            config.relay_bin.clone(),
            config.temp_root.join("resources"),
            config.log_dir.clone(),
        ),
        push_gateway: PushGatewayDriver::new(
            config.push_gateway_bin.clone(),
            config.temp_root.join("resources"),
            config.log_dir.clone(),
        ),
        bitcoind: BitcoindDriver::new(
            config.bitcoind_bin.clone(),
            config.temp_root.join("resources"),
            config.log_dir.clone(),
        ),
        fman: FmanDriver::new(
            config.fleet_manager_bin.clone(),
            config.fman_cli_bin.clone(),
            config.temp_root.join("resources"),
            config.log_dir.clone(),
        ),
        flip: FlipDriver::new(
            config.liquidity_manager_daemon_bin.clone(),
            config.temp_root.join("resources"),
            config.log_dir.clone(),
        ),
        gatewayd: GatewaydDriver::new(
            config.gatewayd_bin.clone(),
            config.gateway_cli_bin.clone(),
            config.temp_root.join("resources"),
            config.log_dir.clone(),
        ),
    })))
}

struct DefeResourceDriver {
    nostr_relay: NostrRelayDriver,
    push_gateway: PushGatewayDriver,
    bitcoind: BitcoindDriver,
    fman: FmanDriver,
    flip: FlipDriver,
    gatewayd: GatewaydDriver,
}

impl ResourceDriver for DefeResourceDriver {
    fn start(&self, allocation: &ResourceAllocation) -> Result<Box<dyn ManagedResource>, ApiError> {
        match allocation.kind {
            ResourceKind::NostrRelay => self.nostr_relay.start(allocation),
            ResourceKind::PushGateway => self.push_gateway.start(allocation),
            ResourceKind::Bitcoind => self.bitcoind.start(allocation),
            ResourceKind::Fman(_) => self.fman.start(allocation),
            ResourceKind::Flip(_) => self.flip.start(allocation),
            ResourceKind::Gatewayd(_) => self.gatewayd.start(allocation),
            ResourceKind::Fake => Err(ApiError::new(
                ApiErrorKind::ResourceKindUnavailable,
                "fake resources are only available in resource-manager tests",
            )),
        }
    }
}

fn prepare_server_config<F>(
    options: ServerOptions,
    default_temp_root: F,
) -> Result<ServerConfig, String>
where
    F: FnOnce() -> Result<PathBuf, String>,
{
    let temp_root = stable_absolute_path(
        match options.tmp_dir {
            Some(temp_root) => temp_root,
            None => default_temp_root()?,
        },
        "temp directory",
    )?;
    validate_utf8_path(&temp_root, "temp directory")?;
    prepare_private_dir(&temp_root)?;

    let log_dir = stable_absolute_path(
        options.log_dir.unwrap_or_else(|| temp_root.join("logs")),
        "log directory",
    )?;
    validate_utf8_path(&log_dir, "log directory")?;
    fs::create_dir_all(&log_dir).map_err(|err| {
        format!(
            "failed to create log directory {}: {err}",
            log_dir.display()
        )
    })?;

    let relay_bin = resolve_binary(
        options.nostr_rs_relay_bin,
        &options.binary_paths,
        "nostr-rs-relay",
    );
    let push_gateway_bin = resolve_binary(
        options.push_gateway_bin,
        &options.binary_paths,
        "fedi-decentralized-push-gateway",
    );
    let bitcoind_bin = resolve_binary(options.bitcoind_bin, &options.binary_paths, "bitcoind");
    let bitcoin_cli_bin = resolve_binary(None, &options.binary_paths, "bitcoin-cli");
    let fleet_manager_bin = resolve_binary(
        options.fleet_manager_bin,
        &options.binary_paths,
        "fleet-manager",
    );
    let fman_cli_bin = resolve_binary(options.fman_cli_bin, &options.binary_paths, "fman-cli");
    let fi_cli_bin = resolve_binary(options.fi_cli_bin, &options.binary_paths, "fi-cli");
    let liquidity_manager_daemon_bin = resolve_binary(
        options.liquidity_manager_daemon_bin,
        &options.binary_paths,
        "liquidity-manager-daemon",
    );
    let gatewayd_bin = resolve_binary(options.gatewayd_bin, &options.binary_paths, "gatewayd");
    let gateway_cli_bin = resolve_binary(
        options.gateway_cli_bin,
        &options.binary_paths,
        "gateway-cli",
    );
    let defe_env_bin = resolve_binary(options.defe_env_bin, &options.binary_paths, "defe-env");
    Ok(ServerConfig {
        temp_root,
        log_dir,
        relay_bin,
        push_gateway_bin,
        bitcoind_bin,
        bitcoin_cli_bin,
        fleet_manager_bin,
        fman_cli_bin,
        fi_cli_bin,
        liquidity_manager_daemon_bin,
        gatewayd_bin,
        gateway_cli_bin,
        defe_env_bin,
    })
}

fn resolve_binary(explicit: Option<PathBuf>, binary_paths: &[PathBuf], name: &str) -> OsString {
    if let Some(explicit) = explicit {
        return absolute_path(explicit).into_os_string();
    }

    binary_paths
        .iter()
        .map(|dir| absolute_path(dir.join(name)))
        .find(|candidate| candidate.exists())
        .or_else(|| {
            env::var_os("PATH")
                .into_iter()
                .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
                .map(|dir| absolute_path(dir.join(name)))
                .find(|candidate| candidate.exists())
        })
        .map(PathBuf::into_os_string)
        .unwrap_or_else(|| OsString::from(name))
}

fn absolute_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .map(|current| current.join(&path))
            .unwrap_or(path)
    }
}

fn stable_absolute_path(path: PathBuf, label: &str) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path);
    }
    env::current_dir()
        .map(|current| current.join(path))
        .map_err(|error| format!("failed to resolve relative {label}: {error}"))
}

fn validate_environment_binary(label: &str, binary: &OsString) -> Result<(), String> {
    let path = Path::new(binary);
    if path.to_str().is_none() {
        return Err(format!("defe env requires a UTF-8 {label} path"));
    }
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "defe env requires selected {label} at {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 || !path.is_absolute() {
        return Err(format!(
            "defe env requires an executable absolute {label} path: {}",
            path.display()
        ));
    }
    Ok(())
}

async fn accept_loop(
    listener: UnixListener,
    mut shutdown: watch::Receiver<bool>,
    resource_manager: Arc<ResourceManager>,
    request_logger: Arc<RequestLogger>,
) {
    loop {
        if *shutdown.borrow() {
            break;
        }

        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            result = listener.accept() => match result {
                Ok((stream, _addr)) => {
                    let connection_id = request_logger.accept_connection();
                    let connection = resource_manager.connection();
                    let request_logger = Arc::clone(&request_logger);
                    tokio::spawn(async move {
                        handle_client(stream, connection, request_logger, connection_id).await;
                    });
                }
                Err(err) => {
                    eprintln!("defe: failed to accept client connection: {err}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
        }
    }
}

async fn handle_client(
    mut stream: UnixStream,
    mut resources: ResourceConnection,
    request_logger: Arc<RequestLogger>,
    connection_id: u64,
) {
    loop {
        let request = match read_frame::<Request>(&mut stream).await {
            Ok(Some(request)) => request,
            Ok(None) => {
                request_logger.disconnect(connection_id);
                return;
            }
            Err(err) => {
                request_logger.protocol_decode_failed(connection_id, &err);
                let response =
                    Response::Error(ApiError::new(ApiErrorKind::ProtocolDecodeError, err));
                request_logger.response(connection_id, &response);
                if let Err(err) = write_response(&mut stream, &response).await {
                    request_logger.write_failed(connection_id, &err);
                }
                return;
            }
        };

        request_logger.request(connection_id, &request);

        let response = tokio::task::block_in_place(|| match request {
            Request::Ping => Response::Pong,
            Request::Allocate(request) => {
                match resources.allocate(resource_spec_from_request(request)) {
                    Ok(lease) => Response::Resource(lease),
                    Err(err) => Response::Error(err),
                }
            }
            Request::Release(handle_id) => match resources.release(handle_id) {
                Ok(()) => Response::Released,
                Err(err) => Response::Error(err),
            },
            Request::Restart { handle_id, mode } => match resources.restart(handle_id, mode) {
                Ok(lease) => Response::Resource(lease),
                Err(err) => Response::Error(err),
            },
        });

        request_logger.response(connection_id, &response);

        if let Err(err) = write_response(&mut stream, &response).await {
            request_logger.write_failed(connection_id, &err);
            return;
        }
    }
}

struct RequestLogger {
    enabled: bool,
    next_connection_id: AtomicU64,
}

impl RequestLogger {
    fn new(enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            enabled,
            next_connection_id: AtomicU64::new(1),
        })
    }

    fn accept_connection(&self) -> u64 {
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::AcqRel);
        if self.enabled {
            eprintln!("defe: client {connection_id}: connected");
        }
        connection_id
    }

    fn request(&self, connection_id: u64, request: &Request) {
        if self.enabled {
            eprintln!(
                "defe: client {connection_id}: request {}",
                request_log_label(request)
            );
        }
    }

    fn response(&self, connection_id: u64, response: &Response) {
        if !self.enabled {
            return;
        }

        if let Response::Error(err) = response {
            eprintln!(
                "defe: client {connection_id}: error {:?}: {}",
                err.kind, err.message
            );
        }
    }

    fn protocol_decode_failed(&self, connection_id: u64, err: &str) {
        if self.enabled {
            eprintln!("defe: client {connection_id}: protocol decode failed: {err}");
        }
    }

    fn write_failed(&self, connection_id: u64, err: &str) {
        if self.enabled {
            eprintln!("defe: client {connection_id}: response write failed: {err}");
        }
    }

    fn disconnect(&self, connection_id: u64) {
        if self.enabled {
            eprintln!("defe: client {connection_id}: disconnected");
        }
    }
}

fn request_log_label(request: &Request) -> String {
    match request {
        Request::Ping => "ping".to_owned(),
        Request::Allocate(ResourceRequest::NostrRelay(request)) => {
            format!("allocate nostr-relay sharing={:?}", request.sharing)
        }
        Request::Allocate(ResourceRequest::PushGateway(request)) => {
            format!("allocate push-gateway sharing={:?}", request.sharing)
        }
        Request::Allocate(ResourceRequest::Bitcoind(request)) => {
            format!("allocate bitcoind sharing={:?}", request.sharing)
        }
        Request::Allocate(ResourceRequest::Fman(request)) => {
            format!("allocate fman sharing={:?}", request.sharing)
        }
        Request::Allocate(ResourceRequest::Flip(request)) => {
            format!("allocate flip sharing={:?}", request.sharing)
        }
        Request::Allocate(ResourceRequest::Gatewayd(request)) => {
            format!("allocate gatewayd sharing={:?}", request.sharing)
        }
        Request::Release(handle_id) => format!("release handle={}", handle_id.0),
        Request::Restart { handle_id, mode } => {
            format!("restart handle={} mode={mode:?}", handle_id.0)
        }
    }
}

fn resource_spec_from_request(request: ResourceRequest) -> ResourceSpec {
    match request {
        ResourceRequest::NostrRelay(request) => {
            let sharing = match request.sharing {
                SharingMode::Shared => ResourceSharing::Shared(SharedResourceKey::NostrRelay),
                SharingMode::Exclusive => ResourceSharing::Exclusive,
            };
            ResourceSpec {
                kind: ResourceKind::NostrRelay,
                sharing,
            }
        }
        ResourceRequest::PushGateway(request) => {
            let sharing = match request.sharing {
                SharingMode::Shared => ResourceSharing::Shared(SharedResourceKey::PushGateway),
                SharingMode::Exclusive => ResourceSharing::Exclusive,
            };
            ResourceSpec {
                kind: ResourceKind::PushGateway,
                sharing,
            }
        }
        ResourceRequest::Bitcoind(request) => {
            let sharing = match request.sharing {
                SharingMode::Shared => ResourceSharing::Shared(SharedResourceKey::Bitcoind),
                SharingMode::Exclusive => ResourceSharing::Exclusive,
            };
            ResourceSpec {
                kind: ResourceKind::Bitcoind,
                sharing,
            }
        }
        ResourceRequest::Fman(request) => {
            let sharing = match request.sharing {
                SharingMode::Shared => {
                    ResourceSharing::Shared(SharedResourceKey::Fman(request.clone()))
                }
                SharingMode::Exclusive => ResourceSharing::Exclusive,
            };
            ResourceSpec {
                kind: ResourceKind::Fman(request),
                sharing,
            }
        }
        ResourceRequest::Flip(request) => {
            let sharing = match request.sharing {
                SharingMode::Shared => {
                    ResourceSharing::Shared(SharedResourceKey::Flip(request.clone()))
                }
                SharingMode::Exclusive => ResourceSharing::Exclusive,
            };
            ResourceSpec {
                kind: ResourceKind::Flip(request),
                sharing,
            }
        }
        ResourceRequest::Gatewayd(request) => {
            let sharing = match request.sharing {
                SharingMode::Shared => {
                    ResourceSharing::Shared(SharedResourceKey::Gatewayd(request.clone()))
                }
                SharingMode::Exclusive => ResourceSharing::Exclusive,
            };
            ResourceSpec {
                kind: ResourceKind::Gatewayd(request),
                sharing,
            }
        }
    }
}

async fn read_frame<T>(stream: &mut UnixStream) -> Result<Option<T>, String>
where
    T: serde::de::DeserializeOwned,
{
    let mut len_buf = [0_u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(format!("failed to read frame length: {err}")),
    }

    let payload_len = u32::from_be_bytes(len_buf) as usize;
    if MAX_FRAME_SIZE < payload_len {
        return Err(format!(
            "frame payload is too large: {payload_len} bytes exceeds {MAX_FRAME_SIZE} byte limit"
        ));
    }

    let mut frame = Vec::with_capacity(4 + payload_len);
    frame.extend_from_slice(&len_buf);
    frame.resize(4 + payload_len, 0);
    stream
        .read_exact(&mut frame[4..])
        .await
        .map_err(|err| format!("failed to read frame payload: {err}"))?;

    defe_api::decode_frame(&frame)
        .map_err(|err| err.to_string())
        .map(Some)
}

async fn write_response(stream: &mut UnixStream, response: &Response) -> Result<(), String> {
    let frame = defe_api::encode_frame(response).map_err(|err| err.to_string())?;
    stream
        .write_all(&frame)
        .await
        .map_err(|err| format!("failed to write response: {err}"))
}

async fn wait_for_exec_child(
    child: &mut Child,
    mut shutdown: watch::Receiver<bool>,
    environment_manifest: Option<&Path>,
    pgid_reader: Option<&mut fs::File>,
) -> std::io::Result<ExitStatus> {
    if *shutdown.borrow() {
        return finish_exec_child(child, environment_manifest, pgid_reader).await;
    }

    tokio::select! {
        status = child.wait() => status,
        changed = shutdown.changed() => {
            if changed.is_ok() && !*shutdown.borrow() {
                return child.wait().await;
            }
            finish_exec_child(child, environment_manifest, pgid_reader).await
        }
    }
}

async fn finish_exec_child(
    child: &mut Child,
    environment_manifest: Option<&Path>,
    pgid_reader: Option<&mut fs::File>,
) -> std::io::Result<ExitStatus> {
    if let Some(environment_manifest) = environment_manifest {
        if let Some(pid) = child.id() {
            // The environment composer catches termination, reaps its foreground
            // process group, and atomically marks the manifest stopped.
            unsafe {
                libc::kill(i32::try_from(pid).unwrap_or(i32::MAX), libc::SIGTERM);
            }
        }
        match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
            Ok(status) => status,
            Err(_) => {
                invalidate_environment_manifest(environment_manifest);
                if let Some(pgid_reader) = pgid_reader {
                    terminate_recorded_environment_group(pgid_reader).await;
                }
                let _ = child.kill().await;
                child.wait().await
            }
        }
    } else {
        let _ = child.kill().await;
        child.wait().await
    }
}

async fn terminate_recorded_environment_group(reader: &mut fs::File) {
    let mut contents = String::new();
    if reader.read_to_string(&mut contents).is_err() {
        return;
    }
    let Ok(process_group) = contents.parse::<i32>() else {
        return;
    };
    if process_group <= 1 {
        return;
    }
    unsafe { libc::kill(-process_group, libc::SIGTERM) };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while tokio::time::Instant::now() < deadline {
        if unsafe { libc::kill(-process_group, 0) } != 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    unsafe { libc::kill(-process_group, libc::SIGKILL) };
}

fn invalidate_environment_manifest(path: &Path) {
    let result = (|| -> Result<(), String> {
        let bytes = fs::read(path).map_err(|error| error.to_string())?;
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        manifest["ready"] = false.into();
        manifest["state"] = "stopped".into();
        manifest["gateway"]["state"] = "stopped".into();
        manifest["flip"]["state"] = "stopped".into();
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(&manifest).unwrap())
            .map_err(|error| error.to_string())?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
        fs::rename(temporary, path).map_err(|error| error.to_string())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(path);
        eprintln!("defe: removed environment manifest after invalidation failed: {error}");
    }
}

fn cleanup_temp(temp_root: &Path, policy: &TempPolicy, success: bool) -> Result<(), String> {
    let keep = policy.keep_temp || (!success && policy.keep_temp_on_failure);
    if keep {
        eprintln!("defe: preserving temp directory {}", temp_root.display());
        return Ok(());
    }

    fs::remove_dir_all(temp_root).map_err(|err| {
        format!(
            "failed to remove temp directory {}: {err}",
            temp_root.display()
        )
    })
}

fn remove_stale_socket(socket_path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(socket_path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            if StdUnixStream::connect(socket_path).is_ok() {
                return Err(format!(
                    "refusing to replace active socket {}",
                    socket_path.display()
                ));
            }
            fs::remove_file(socket_path).map_err(|err| {
                format!(
                    "failed to remove stale socket {}: {err}",
                    socket_path.display()
                )
            })
        }
        Ok(_metadata) => Err(format!(
            "refusing to replace non-socket path {}",
            socket_path.display()
        )),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!(
            "failed to inspect socket path {}: {err}",
            socket_path.display()
        )),
    }
}

fn validate_utf8_path(path: &Path, label: &str) -> Result<(), String> {
    if path.to_str().is_some() {
        return Ok(());
    }

    Err(format!(
        "{label} path must be valid UTF-8 for defe's wire protocol: {}",
        path.display()
    ))
}

fn prepare_private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|err| format!("failed to create temp directory {}: {err}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|err| {
        format!(
            "failed to set private permissions on {}: {err}",
            path.display()
        )
    })
}

fn default_temp_root() -> Result<PathBuf, String> {
    create_default_temp_root_in(&env::temp_dir(), std::process::id())
}

fn create_default_temp_root_in(parent: &Path, pid: u32) -> Result<PathBuf, String> {
    for attempt in 0..DEFAULT_TEMP_DIR_ATTEMPTS {
        let candidate = default_temp_root_candidate(parent, pid, attempt as u8);
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(format!(
                    "failed to create private temp directory {}: {err}",
                    candidate.display()
                ));
            }
        }
    }

    Err(format!(
        "failed to allocate a private temp directory under {} after {DEFAULT_TEMP_DIR_ATTEMPTS} attempts",
        parent.display()
    ))
}

fn default_temp_root_candidate(parent: &Path, pid: u32, attempt: u8) -> PathBuf {
    parent.join(format!("d{:04x}{attempt:02x}", pid & 0xffff))
}

fn default_serve_temp_root() -> PathBuf {
    env::temp_dir().join(DEV_SERVER_TEMP_DIR_NAME)
}

fn take_listenfd_unix_listener() -> Result<StdUnixListener, String> {
    let mut listenfd = listenfd::ListenFd::from_env();
    listenfd
        .take_unix_listener(0)
        .map_err(|err| format!("failed to take inherited listenfd Unix listener: {err}"))?
        .ok_or_else(|| "defe serve --listenfd requires an inherited Unix listener fd".to_owned())
}

fn install_shutdown_handler(shutdown: watch::Sender<bool>) -> Result<(), String> {
    ctrlc::set_handler(move || {
        let _ = shutdown.send(true);
    })
    .map_err(|err| format!("failed to install shutdown signal handler: {err}"))
}

fn parse_command(args: Vec<OsString>) -> Result<DefeCommand, String> {
    let mut options = ServerOptions::default();
    let mut policy = TempPolicy::default();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "exec" {
            return Ok(DefeCommand::Exec(ExecArgs {
                options,
                policy,
                command: args[index + 1..].to_vec(),
                environment: None,
            }));
        }
        if arg == "env" {
            let environment = parse_env_args(&args[index + 1..])?;
            return Ok(DefeCommand::Exec(ExecArgs {
                options,
                policy,
                command: Vec::new(),
                environment: Some(environment),
            }));
        }
        if arg == "serve" {
            return parse_serve(options, &args[index + 1..]);
        }

        if arg == "--keep-temp" {
            policy.keep_temp = true;
            index += 1;
        } else if arg == "--no-keep-temp-on-failure" {
            policy.keep_temp_on_failure = false;
            index += 1;
        } else if arg == "--tmp-dir" {
            index += 1;
            options.tmp_dir = Some(take_path_arg(&args, index, "--tmp-dir")?);
            index += 1;
        } else if arg == "--log-dir" {
            index += 1;
            options.log_dir = Some(take_path_arg(&args, index, "--log-dir")?);
            index += 1;
        } else if arg == "--log-requests" {
            options.log_requests = true;
            index += 1;
        } else if arg == "--binary-path" {
            index += 1;
            options
                .binary_paths
                .push(take_path_arg(&args, index, "--binary-path")?);
            index += 1;
        } else if arg == "--nostr-rs-relay-bin" {
            index += 1;
            options.nostr_rs_relay_bin = Some(take_path_arg(&args, index, "--nostr-rs-relay-bin")?);
            index += 1;
        } else if arg == "--push-gateway-bin" {
            index += 1;
            options.push_gateway_bin = Some(take_path_arg(&args, index, "--push-gateway-bin")?);
            index += 1;
        } else if arg == "--bitcoind-bin" {
            index += 1;
            options.bitcoind_bin = Some(take_path_arg(&args, index, "--bitcoind-bin")?);
            index += 1;
        } else if arg == "--fleet-manager-bin" {
            index += 1;
            options.fleet_manager_bin = Some(take_path_arg(&args, index, "--fleet-manager-bin")?);
            index += 1;
        } else if arg == "--fman-cli-bin" {
            index += 1;
            options.fman_cli_bin = Some(take_path_arg(&args, index, "--fman-cli-bin")?);
            index += 1;
        } else if arg == "--fi-cli-bin" {
            index += 1;
            options.fi_cli_bin = Some(take_path_arg(&args, index, "--fi-cli-bin")?);
            index += 1;
        } else if arg == "--liquidity-manager-daemon-bin" {
            index += 1;
            options.liquidity_manager_daemon_bin = Some(take_path_arg(
                &args,
                index,
                "--liquidity-manager-daemon-bin",
            )?);
            index += 1;
        } else if arg == "--gatewayd-bin" {
            index += 1;
            options.gatewayd_bin = Some(take_path_arg(&args, index, "--gatewayd-bin")?);
            index += 1;
        } else if arg == "--gateway-cli-bin" {
            index += 1;
            options.gateway_cli_bin = Some(take_path_arg(&args, index, "--gateway-cli-bin")?);
            index += 1;
        } else if arg == "--defe-env-bin" {
            index += 1;
            options.defe_env_bin = Some(take_path_arg(&args, index, "--defe-env-bin")?);
            index += 1;
        } else {
            return Err(format!(
                "unrecognized defe argument: {}\n{USAGE}",
                arg.to_string_lossy()
            ));
        }
    }

    Err(format!("missing defe command\n{USAGE}"))
}

fn parse_env_args(args: &[OsString]) -> Result<EnvironmentArgs, String> {
    let mut load_test_tool = None;
    let mut index = 0;
    while args
        .get(index)
        .is_some_and(|argument| argument == "--fedimint-load-test-tool-bin")
    {
        index += 1;
        load_test_tool = Some(take_path_arg(args, index, "--fedimint-load-test-tool-bin")?);
        index += 1;
    }
    Ok(EnvironmentArgs {
        args: args[index..].to_vec(),
        load_test_tool_bin: load_test_tool,
    })
}

fn parse_serve(mut options: ServerOptions, args: &[OsString]) -> Result<DefeCommand, String> {
    let mut listenfd = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--listenfd" {
            listenfd = true;
            index += 1;
        } else if parse_server_option(&mut options, args, &mut index)? {
        } else {
            return Err(format!(
                "unrecognized defe serve argument: {}\n{USAGE}",
                arg.to_string_lossy()
            ));
        }
    }

    Ok(DefeCommand::Serve(ServeArgs { options, listenfd }))
}

fn parse_server_option(
    options: &mut ServerOptions,
    args: &[OsString],
    index: &mut usize,
) -> Result<bool, String> {
    let arg = &args[*index];
    if arg == "--tmp-dir" {
        *index += 1;
        options.tmp_dir = Some(take_path_arg(args, *index, "--tmp-dir")?);
        *index += 1;
        Ok(true)
    } else if arg == "--log-dir" {
        *index += 1;
        options.log_dir = Some(take_path_arg(args, *index, "--log-dir")?);
        *index += 1;
        Ok(true)
    } else if arg == "--log-requests" {
        options.log_requests = true;
        *index += 1;
        Ok(true)
    } else if arg == "--binary-path" {
        *index += 1;
        options
            .binary_paths
            .push(take_path_arg(args, *index, "--binary-path")?);
        *index += 1;
        Ok(true)
    } else if arg == "--nostr-rs-relay-bin" {
        *index += 1;
        options.nostr_rs_relay_bin = Some(take_path_arg(args, *index, "--nostr-rs-relay-bin")?);
        *index += 1;
        Ok(true)
    } else if arg == "--push-gateway-bin" {
        *index += 1;
        options.push_gateway_bin = Some(take_path_arg(args, *index, "--push-gateway-bin")?);
        *index += 1;
        Ok(true)
    } else if arg == "--bitcoind-bin" {
        *index += 1;
        options.bitcoind_bin = Some(take_path_arg(args, *index, "--bitcoind-bin")?);
        *index += 1;
        Ok(true)
    } else if arg == "--fleet-manager-bin" {
        *index += 1;
        options.fleet_manager_bin = Some(take_path_arg(args, *index, "--fleet-manager-bin")?);
        *index += 1;
        Ok(true)
    } else if arg == "--fman-cli-bin" {
        *index += 1;
        options.fman_cli_bin = Some(take_path_arg(args, *index, "--fman-cli-bin")?);
        *index += 1;
        Ok(true)
    } else if arg == "--fi-cli-bin" {
        *index += 1;
        options.fi_cli_bin = Some(take_path_arg(args, *index, "--fi-cli-bin")?);
        *index += 1;
        Ok(true)
    } else if arg == "--liquidity-manager-daemon-bin" {
        *index += 1;
        options.liquidity_manager_daemon_bin = Some(take_path_arg(
            args,
            *index,
            "--liquidity-manager-daemon-bin",
        )?);
        *index += 1;
        Ok(true)
    } else if arg == "--gatewayd-bin" {
        *index += 1;
        options.gatewayd_bin = Some(take_path_arg(args, *index, "--gatewayd-bin")?);
        *index += 1;
        Ok(true)
    } else if arg == "--gateway-cli-bin" {
        *index += 1;
        options.gateway_cli_bin = Some(take_path_arg(args, *index, "--gateway-cli-bin")?);
        *index += 1;
        Ok(true)
    } else if arg == "--defe-env-bin" {
        *index += 1;
        options.defe_env_bin = Some(take_path_arg(args, *index, "--defe-env-bin")?);
        *index += 1;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn take_path_arg(args: &[OsString], index: usize, option: &str) -> Result<PathBuf, String> {
    args.get(index)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{option} requires a path"))
}

#[cfg(unix)]
fn exit_code_from_status(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }

    128 + status.signal().unwrap_or(1)
}

#[derive(Debug)]
enum DefeCommand {
    Exec(ExecArgs),
    Serve(ServeArgs),
}

#[derive(Debug)]
struct ExecArgs {
    options: ServerOptions,
    policy: TempPolicy,
    command: Vec<OsString>,
    environment: Option<EnvironmentArgs>,
}

#[derive(Debug)]
struct EnvironmentArgs {
    args: Vec<OsString>,
    load_test_tool_bin: Option<PathBuf>,
}

#[derive(Debug)]
struct ServeArgs {
    options: ServerOptions,
    listenfd: bool,
}

#[derive(Debug, Default)]
struct ServerOptions {
    tmp_dir: Option<PathBuf>,
    log_dir: Option<PathBuf>,
    log_requests: bool,
    binary_paths: Vec<PathBuf>,
    nostr_rs_relay_bin: Option<PathBuf>,
    push_gateway_bin: Option<PathBuf>,
    bitcoind_bin: Option<PathBuf>,
    fleet_manager_bin: Option<PathBuf>,
    fman_cli_bin: Option<PathBuf>,
    fi_cli_bin: Option<PathBuf>,
    liquidity_manager_daemon_bin: Option<PathBuf>,
    gatewayd_bin: Option<PathBuf>,
    gateway_cli_bin: Option<PathBuf>,
    defe_env_bin: Option<PathBuf>,
}

#[derive(Debug)]
struct ServerConfig {
    temp_root: PathBuf,
    log_dir: PathBuf,
    relay_bin: OsString,
    push_gateway_bin: OsString,
    bitcoind_bin: OsString,
    bitcoin_cli_bin: OsString,
    fleet_manager_bin: OsString,
    fman_cli_bin: OsString,
    fi_cli_bin: OsString,
    liquidity_manager_daemon_bin: OsString,
    gatewayd_bin: OsString,
    gateway_cli_bin: OsString,
    defe_env_bin: OsString,
}

#[derive(Debug)]
struct TempPolicy {
    keep_temp: bool,
    keep_temp_on_failure: bool,
}

impl Default for TempPolicy {
    fn default() -> Self {
        Self {
            keep_temp: false,
            keep_temp_on_failure: true,
        }
    }
}

#[cfg(test)]
mod main_tests;
