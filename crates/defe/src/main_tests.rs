use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use defe::resource_manager::{
    ManagedResource, ResourceAllocation, ResourceDriver, ResourceKind, ResourceSharing,
    SharedResourceKey, fake_nostr_descriptor,
};
use defe_api::{
    ApiError, ApiErrorKind, NostrRelayRequest, PushGatewayRequest, ResourceDescriptor,
    ResourceHandleId, ResourceLease, RestartMode,
};

use tokio::io::AsyncWriteExt as _;

use super::*;

#[test]
fn parse_exec_keeps_existing_global_options() {
    let command = parse_command(vec![
        "--tmp-dir".into(),
        "/tmp/defe-one-shot".into(),
        "--log-dir".into(),
        "/tmp/defe-one-shot-logs".into(),
        "--binary-path".into(),
        "/tmp/defe-bin".into(),
        "--nostr-rs-relay-bin".into(),
        "/bin/nostr-rs-relay".into(),
        "--push-gateway-bin".into(),
        "/bin/push-gateway".into(),
        "--fman-cli-bin".into(),
        "/bin/fman-cli".into(),
        "--liquidity-manager-daemon-bin".into(),
        "/bin/liquidity-manager-daemon".into(),
        "--keep-temp".into(),
        "exec".into(),
        "true".into(),
    ])
    .expect("parse exec command");

    let DefeCommand::Exec(exec) = command else {
        panic!("expected exec command");
    };
    assert_eq!(
        exec.options.tmp_dir,
        Some(PathBuf::from("/tmp/defe-one-shot"))
    );
    assert_eq!(
        exec.options.log_dir,
        Some(PathBuf::from("/tmp/defe-one-shot-logs"))
    );
    assert_eq!(
        exec.options.binary_paths,
        vec![PathBuf::from("/tmp/defe-bin")]
    );
    assert_eq!(
        exec.options.nostr_rs_relay_bin,
        Some(PathBuf::from("/bin/nostr-rs-relay"))
    );
    assert_eq!(
        exec.options.push_gateway_bin,
        Some(PathBuf::from("/bin/push-gateway"))
    );
    assert_eq!(
        exec.options.fman_cli_bin,
        Some(PathBuf::from("/bin/fman-cli"))
    );
    assert!(exec.policy.keep_temp);
    assert_eq!(exec.command, vec![OsString::from("true")]);
}

#[test]
fn parse_serve_requires_listenfd_and_keeps_server_options() {
    let command = parse_command(vec![
        "--tmp-dir".into(),
        "/tmp/defe-server".into(),
        "--log-dir".into(),
        "/tmp/defe-server-logs".into(),
        "--binary-path".into(),
        "/tmp/defe-bin".into(),
        "--nostr-rs-relay-bin".into(),
        "/bin/nostr-rs-relay".into(),
        "--push-gateway-bin".into(),
        "/bin/push-gateway".into(),
        "--fman-cli-bin".into(),
        "/bin/fman-cli".into(),
        "--log-requests".into(),
        "serve".into(),
        "--listenfd".into(),
        "--binary-path".into(),
        "/tmp/defe-bin-late".into(),
    ])
    .expect("parse serve command");

    let DefeCommand::Serve(serve) = command else {
        panic!("expected serve command");
    };
    assert!(serve.listenfd);
    assert!(serve.options.log_requests);
    assert_eq!(
        serve.options.tmp_dir,
        Some(PathBuf::from("/tmp/defe-server"))
    );
    assert_eq!(
        serve.options.log_dir,
        Some(PathBuf::from("/tmp/defe-server-logs"))
    );
    assert_eq!(
        serve.options.binary_paths,
        vec![
            PathBuf::from("/tmp/defe-bin"),
            PathBuf::from("/tmp/defe-bin-late"),
        ]
    );
    assert_eq!(
        serve.options.nostr_rs_relay_bin,
        Some(PathBuf::from("/bin/nostr-rs-relay"))
    );
    assert_eq!(
        serve.options.push_gateway_bin,
        Some(PathBuf::from("/bin/push-gateway"))
    );
    assert_eq!(
        serve.options.fman_cli_bin,
        Some(PathBuf::from("/bin/fman-cli"))
    );
}

#[test]
fn parse_serve_accepts_request_logging_after_subcommand() {
    let command = parse_command(vec![
        "serve".into(),
        "--listenfd".into(),
        "--log-requests".into(),
    ])
    .expect("parse serve command");

    let DefeCommand::Serve(serve) = command else {
        panic!("expected serve command");
    };
    assert!(serve.listenfd);
    assert!(serve.options.log_requests);
}

#[test]
fn parse_serve_accepts_documented_server_options_after_subcommand() {
    let command = parse_command(vec![
        "serve".into(),
        "--listenfd".into(),
        "--tmp-dir".into(),
        "/tmp/defe-server".into(),
        "--log-dir".into(),
        "/tmp/defe-server-logs".into(),
        "--binary-path".into(),
        "/tmp/defe-bin".into(),
        "--nostr-rs-relay-bin".into(),
        "/bin/nostr-rs-relay".into(),
        "--push-gateway-bin".into(),
        "/bin/push-gateway".into(),
        "--liquidity-manager-daemon-bin".into(),
        "/bin/liquidity-manager-daemon".into(),
    ])
    .expect("parse serve command");

    let DefeCommand::Serve(serve) = command else {
        panic!("expected serve command");
    };
    assert!(serve.listenfd);
    assert_eq!(
        serve.options.tmp_dir,
        Some(PathBuf::from("/tmp/defe-server"))
    );
    assert_eq!(
        serve.options.log_dir,
        Some(PathBuf::from("/tmp/defe-server-logs"))
    );
    assert_eq!(
        serve.options.binary_paths,
        vec![PathBuf::from("/tmp/defe-bin")]
    );
    assert_eq!(
        serve.options.nostr_rs_relay_bin,
        Some(PathBuf::from("/bin/nostr-rs-relay"))
    );
    assert_eq!(
        serve.options.push_gateway_bin,
        Some(PathBuf::from("/bin/push-gateway"))
    );
    assert_eq!(
        serve.options.liquidity_manager_daemon_bin,
        Some(PathBuf::from("/bin/liquidity-manager-daemon"))
    );
}

#[test]
fn resolve_binary_prefers_first_existing_binary_path() {
    let test_dir = TestDir::new("binary-path");
    let first = test_dir.path().join("first");
    let second = test_dir.path().join("second");
    fs::create_dir_all(&first).expect("create first binary path");
    fs::create_dir_all(&second).expect("create second binary path");
    fs::write(second.join("tool"), "#!/bin/sh\n").expect("write tool");

    assert_eq!(
        resolve_binary(None, &[first, second.clone()], "tool"),
        second.join("tool").into_os_string()
    );
}

#[test]
fn resolve_binary_keeps_explicit_path_and_falls_back_to_name() {
    assert_eq!(
        resolve_binary(
            Some(PathBuf::from("/bin/tool")),
            &[PathBuf::from("/tmp/bin")],
            "tool"
        ),
        OsString::from("/bin/tool")
    );
    assert_eq!(resolve_binary(None, &[], "tool"), OsString::from("tool"));
}

#[test]
fn stale_socket_file_is_removed_before_binding() {
    let test_dir = TestDir::new("stale-socket");
    let socket_path = test_dir.path().join(SOCKET_FILE_NAME);
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).expect("bind stale socket");
    drop(listener);

    remove_stale_socket(&socket_path).expect("remove stale socket");

    assert!(!socket_path.exists());
    std::os::unix::net::UnixListener::bind(&socket_path).expect("stale socket path can be reused");
}

#[test]
fn active_socket_file_is_not_removed() {
    let test_dir = TestDir::new("active-socket");
    let socket_path = test_dir.path().join(SOCKET_FILE_NAME);
    let listener =
        std::os::unix::net::UnixListener::bind(&socket_path).expect("bind active socket");

    let err = remove_stale_socket(&socket_path).expect_err("active socket is rejected");

    assert!(err.contains("refusing to replace active socket"));
    drop(listener);
}

#[test]
fn stale_non_socket_path_is_not_removed() {
    let test_dir = TestDir::new("stale-file");
    let socket_path = test_dir.path().join(SOCKET_FILE_NAME);
    fs::write(&socket_path, "not a socket").expect("write non-socket file");

    let err = remove_stale_socket(&socket_path).expect_err("non-socket path is rejected");

    assert!(err.contains("refusing to replace non-socket path"));
    assert_eq!(fs::read_to_string(&socket_path).unwrap(), "not a socket");
}

#[cfg(unix)]
#[test]
fn prepare_server_config_rejects_non_utf8_temp_path() {
    use std::os::unix::ffi::OsStringExt;

    let err = prepare_server_config(
        ServerOptions {
            tmp_dir: Some(PathBuf::from(OsString::from_vec(vec![
                b'/', b't', b'm', b'p', b'/', 0xff,
            ]))),
            ..ServerOptions::default()
        },
        || panic!("explicit temp path must not evaluate the default"),
    )
    .expect_err("non-UTF-8 temp path is rejected");

    assert!(err.contains("valid UTF-8"));
}

#[test]
fn default_temp_root_is_compact_private_and_collision_safe() {
    let parent = TestDir::new("default-temp-parent");
    let occupied = default_temp_root_candidate(parent.path(), 0x12345, 0);
    fs::create_dir(&occupied).expect("create occupied candidate");

    let temp_root = create_default_temp_root_in(parent.path(), 0x12345)
        .expect("allocate compact private temp root");

    assert_eq!(
        temp_root.file_name().and_then(|name| name.to_str()),
        Some("d234501")
    );
    assert_eq!(
        fs::metadata(&temp_root)
            .expect("read temp root metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(occupied.is_dir(), "an occupied candidate is not replaced");
}

#[test]
fn compact_socket_path_fits_known_darwin_nix_build_parents() {
    use std::os::unix::ffi::OsStrExt as _;

    let github_runner_build_parent = Path::new("/nix/var/nix/builds/nix-60643-1064089074");
    let legacy_socket_path = github_runner_build_parent
        .join("defe-main-test-active-socket-14012-1785970543150882000-0")
        .join("defe.sock");
    assert!(
        104 <= legacy_socket_path.as_os_str().as_bytes().len(),
        "the regression fixture must exceed Darwin's sockaddr_un limit"
    );

    for nix_build_parent in [
        github_runner_build_parent,
        Path::new(
            "/private/tmp/nix-build-decentralized-federations-nextest-flip-deposit-ci-0.1.0.drv-0",
        ),
    ] {
        let socket_path =
            default_temp_root_candidate(nix_build_parent, 25278, 0).join(SOCKET_FILE_NAME);
        assert!(
            socket_path.as_os_str().as_bytes().len() < 104,
            "Darwin sockaddr_un path, including its NUL terminator, must fit: {}",
            socket_path.display()
        );
    }
}

#[test]
fn request_log_labels_include_major_request_details() {
    assert_eq!(request_log_label(&Request::Ping), "ping");
    assert_eq!(
        request_log_label(&Request::Allocate(ResourceRequest::NostrRelay(
            NostrRelayRequest::exclusive(),
        ))),
        "allocate nostr-relay sharing=Exclusive"
    );
    assert_eq!(
        request_log_label(&Request::Allocate(ResourceRequest::PushGateway(
            PushGatewayRequest::shared(),
        ))),
        "allocate push-gateway sharing=Shared"
    );
    assert_eq!(
        request_log_label(&Request::Release(ResourceHandleId(7))),
        "release handle=7"
    );
    assert_eq!(
        request_log_label(&Request::Restart {
            handle_id: ResourceHandleId(8),
            mode: RestartMode::Force,
        }),
        "restart handle=8 mode=Force"
    );
}

#[test]
fn push_gateway_request_maps_to_resource_spec() {
    let shared =
        resource_spec_from_request(ResourceRequest::PushGateway(PushGatewayRequest::shared()));
    assert_eq!(shared.kind, ResourceKind::PushGateway);
    assert_eq!(
        shared.sharing,
        ResourceSharing::Shared(SharedResourceKey::PushGateway)
    );

    let exclusive =
        resource_spec_from_request(ResourceRequest::PushGateway(PushGatewayRequest::exclusive()));
    assert_eq!(exclusive.kind, ResourceKind::PushGateway);
    assert_eq!(exclusive.sharing, ResourceSharing::Exclusive);
}

#[test]
fn run_serve_errors_without_inherited_listenfd() {
    let err = test_runtime()
        .block_on(run_serve(ServeArgs {
            options: ServerOptions::default(),
            listenfd: true,
        }))
        .expect_err("missing listenfd should fail");

    assert!(err.contains("inherited Unix listener"));
}

#[test]
fn server_resource_api_allocates_restarts_and_releases() {
    let driver = FakeServerDriver::default();
    let manager = Arc::new(ResourceManager::new(Arc::new(driver.clone())));
    let request_logger = RequestLogger::new(false);
    let runtime = test_runtime();
    runtime.block_on(async {
        let (mut client, server) = UnixStream::pair().expect("create unix stream pair");
        let server_task = tokio::spawn(handle_client(
            server,
            manager.connection(),
            request_logger,
            1,
        ));

        write_request(
            &mut client,
            &Request::Allocate(ResourceRequest::NostrRelay(NostrRelayRequest::shared())),
        )
        .await;
        let lease = expect_resource(read_response(&mut client).await);
        assert_eq!(lease.handle_id, ResourceHandleId(1));
        assert_eq!(slot_generation(&lease.descriptor), 1);
        assert_eq!(driver.start_count(), 1);

        write_request(
            &mut client,
            &Request::Restart {
                handle_id: lease.handle_id,
                mode: RestartMode::Force,
            },
        )
        .await;
        let restarted = expect_resource(read_response(&mut client).await);
        assert_eq!(restarted.handle_id, lease.handle_id);
        assert_eq!(slot_generation(&restarted.descriptor), 2);
        assert_eq!(driver.start_count(), 2);
        assert_eq!(driver.stop_count(), 1);

        write_request(&mut client, &Request::Release(lease.handle_id)).await;
        assert_eq!(read_response(&mut client).await, Response::Released);
        assert_eq!(driver.stop_count(), 2);

        write_request(&mut client, &Request::Release(lease.handle_id)).await;
        match read_response(&mut client).await {
            Response::Error(err) => assert_eq!(err.kind, ApiErrorKind::UnknownHandle),
            response => panic!("expected unknown-handle error, got {response:?}"),
        }

        drop(client);
        server_task.await.expect("server task exits");
    });
}

async fn write_request(stream: &mut UnixStream, request: &Request) {
    let frame = defe_api::encode_frame(request).expect("encode request");
    stream.write_all(&frame).await.expect("write request");
}

async fn read_response(stream: &mut UnixStream) -> Response {
    read_frame(stream)
        .await
        .expect("read response")
        .expect("server response")
}

fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime")
}

fn expect_resource(response: Response) -> ResourceLease {
    match response {
        Response::Resource(lease) => lease,
        response => panic!("expected resource response, got {response:?}"),
    }
}

fn slot_generation(descriptor: &ResourceDescriptor) -> u16 {
    let ResourceDescriptor::NostrRelay(info) = descriptor else {
        panic!("expected Nostr relay descriptor, got {descriptor:?}");
    };
    info.port
}

#[derive(Clone, Default)]
struct FakeServerDriver {
    inner: Arc<FakeServerDriverInner>,
}

#[derive(Default)]
struct FakeServerDriverInner {
    starts: Mutex<usize>,
    stops: Mutex<usize>,
}

impl FakeServerDriver {
    fn start_count(&self) -> usize {
        *self.inner.starts.lock().expect("starts mutex")
    }

    fn stop_count(&self) -> usize {
        *self.inner.stops.lock().expect("stops mutex")
    }
}

impl ResourceDriver for FakeServerDriver {
    fn start(&self, allocation: &ResourceAllocation) -> Result<Box<dyn ManagedResource>, ApiError> {
        *self.inner.starts.lock().expect("starts mutex") += 1;
        Ok(Box::new(FakeServerResource {
            driver: self.clone(),
            running: AtomicBool::new(true),
            descriptor: fake_nostr_descriptor(allocation.slot_id, allocation.generation),
        }))
    }
}

struct FakeServerResource {
    driver: FakeServerDriver,
    running: AtomicBool,
    descriptor: ResourceDescriptor,
}

impl ManagedResource for FakeServerResource {
    fn descriptor(&self) -> ResourceDescriptor {
        self.descriptor.clone()
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn stop(&mut self) {
        if self.running.swap(false, Ordering::AcqRel) {
            *self.driver.inner.stops.lock().expect("stops mutex") += 1;
        }
    }
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        let path = create_default_temp_root_in(&std::env::temp_dir(), std::process::id())
            .unwrap_or_else(|err| panic!("create {name} test directory: {err}"));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn fman_request_preserves_requested_sharing() {
    let request = defe_api::FmanRequest {
        sharing: SharingMode::Shared,
        bitcoind: defe_api::BitcoindInfo {
            rpc_url: "http://127.0.0.1:18443".to_owned(),
            rpc_host: "127.0.0.1".to_owned(),
            rpc_port: 18443,
            p2p_port: 18444,
            rpc_username: "bitcoin".to_owned(),
            rpc_password: "bitcoin".to_owned(),
            data_dir: PathBuf::from("/tmp/defe/bitcoind"),
        },
        nostr_relay_url: "ws://127.0.0.1:7777".to_owned(),
        first_port_base: 34000,
        iroh_connect_overrides: "routes".to_owned(),
    };

    let spec = resource_spec_from_request(ResourceRequest::Fman(request.clone()));
    assert_eq!(spec.kind, ResourceKind::Fman(request.clone()));
    assert_eq!(
        spec.sharing,
        ResourceSharing::Shared(SharedResourceKey::Fman(request))
    );
}

#[test]
fn flip_request_preserves_requested_sharing() {
    let request = defe_api::FlipRequest {
        sharing: SharingMode::Shared,
        iroh_connect_overrides: None,
    };

    let spec = resource_spec_from_request(ResourceRequest::Flip(request.clone()));
    assert_eq!(spec.kind, ResourceKind::Flip(request.clone()));
    assert_eq!(
        spec.sharing,
        ResourceSharing::Shared(SharedResourceKey::Flip(request))
    );
}

#[test]
fn gatewayd_request_preserves_requested_sharing() {
    let request = defe_api::GatewaydRequest {
        sharing: SharingMode::Shared,
        bitcoind: defe_api::BitcoindInfo {
            rpc_url: "http://127.0.0.1:18443".to_owned(),
            rpc_host: "127.0.0.1".to_owned(),
            rpc_port: 18443,
            p2p_port: 18444,
            rpc_username: "bitcoin".to_owned(),
            rpc_password: "bitcoin".to_owned(),
            data_dir: PathBuf::from("/tmp/defe/bitcoind"),
        },
        iroh_connect_overrides: None,
    };

    let spec = resource_spec_from_request(ResourceRequest::Gatewayd(request.clone()));
    assert_eq!(spec.kind, ResourceKind::Gatewayd(request.clone()));
    assert_eq!(
        spec.sharing,
        ResourceSharing::Shared(SharedResourceKey::Gatewayd(request))
    );
}
