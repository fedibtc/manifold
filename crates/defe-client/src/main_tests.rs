use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use defe_api::{Request, ResourceHandleId, ResourceLease, Response};

use super::*;
use crate::test_support::unique_temp_dir;

#[test]
fn parses_relay_request_modes_and_separator() {
    let parsed = parse_wrapper_args(os_args([
        "--request-relay",
        "--request-relay=shared",
        "--request-relay=exclusive",
        "--",
        "echo",
        "ok",
    ]))
    .expect("parse wrapper args");

    assert_eq!(
        parsed.requests,
        vec![
            ResourceRequestArg::NostrRelay {
                sharing: SharingMode::Shared,
            },
            ResourceRequestArg::NostrRelay {
                sharing: SharingMode::Shared,
            },
            ResourceRequestArg::NostrRelay {
                sharing: SharingMode::Exclusive,
            },
        ]
    );
    assert_eq!(parsed.command, os_args(["echo", "ok"]));
}

#[test]
fn parses_push_gateway_request_modes_and_rejects_invalid_mode() {
    let parsed = parse_wrapper_args(os_args([
        "--request-push-gateway",
        "--request-push-gateway=shared",
        "--request-push-gateway=exclusive",
        "--",
        "echo",
        "ok",
    ]))
    .expect("parse wrapper args");

    assert_eq!(
        parsed.requests,
        vec![
            ResourceRequestArg::PushGateway {
                sharing: SharingMode::Shared,
            },
            ResourceRequestArg::PushGateway {
                sharing: SharingMode::Shared,
            },
            ResourceRequestArg::PushGateway {
                sharing: SharingMode::Exclusive,
            },
        ]
    );
    assert_eq!(parsed.command, os_args(["echo", "ok"]));

    let err = parse_wrapper_args(os_args(["--request-push-gateway=bad", "echo"]))
        .expect_err("reject invalid push gateway mode");
    assert!(err.contains("unsupported --request-push-gateway mode: bad"));
}

#[test]
fn parses_bitcoind_request_modes_and_rejects_invalid_mode() {
    let parsed = parse_wrapper_args(os_args([
        "--request-bitcoind",
        "--request-bitcoind=shared",
        "--request-bitcoind=exclusive",
        "--",
        "echo",
        "ok",
    ]))
    .expect("parse wrapper args");

    assert_eq!(
        parsed.requests,
        vec![
            ResourceRequestArg::Bitcoind {
                sharing: SharingMode::Shared,
            },
            ResourceRequestArg::Bitcoind {
                sharing: SharingMode::Shared,
            },
            ResourceRequestArg::Bitcoind {
                sharing: SharingMode::Exclusive,
            },
        ]
    );
    assert_eq!(parsed.command, os_args(["echo", "ok"]));

    let err = parse_wrapper_args(os_args(["--request-bitcoind=bad", "echo"]))
        .expect_err("reject invalid bitcoind mode");
    assert!(err.contains("unsupported --request-bitcoind mode: bad"));
}

#[test]
fn rejects_unknown_request_flags_before_command() {
    let err = parse_wrapper_args(os_args(["--request-foo", "echo"])).expect_err("reject flag");

    assert!(err.contains("unsupported resource request option"));
}

#[tokio::test(flavor = "current_thread")]
async fn request_relay_exports_env_and_keeps_connection_until_child_exits() {
    let test_dir = unique_temp_dir("relay-wrapper");
    fs::create_dir_all(&test_dir).expect("create test dir");
    let marker_path = test_dir.join("child-ready");
    let release_path = test_dir.join("release-child");
    let server = FakeServer::start(
        &test_dir,
        ResourceLease {
            handle_id: ResourceHandleId(7),
            descriptor: fake_relay_descriptor(),
        },
        marker_path.clone(),
        release_path.clone(),
    );

    let script = format!(
        "test \"$DEV_DEFE_NOSTR_RELAY_URL\" = 'ws://127.0.0.1:12345' && \
         test \"$DEV_DEFE_NOSTR_RELAY_PORT\" = '12345' && \
         test \"$DEV_DEFE_NOSTR_RELAY_DATA_DIR\" = '/tmp/defe-client-fake-relay-db' && \
         touch '{}' && \
         while [ ! -e '{}' ]; do sleep 0.01; done",
        marker_path.display(),
        release_path.display()
    );
    let wrapper = WrapperArgs {
        requests: vec![ResourceRequestArg::NostrRelay {
            sharing: SharingMode::Shared,
        }],
        command: os_args(["sh", "-c", &script]),
    };

    let code = run_wrapper_with_socket(wrapper, Some(server.socket_path().as_os_str().to_owned()))
        .await
        .expect("run wrapper");
    let observed = server.join();

    assert_eq!(code, 0);
    assert_eq!(
        observed.request,
        Some(Request::Allocate(ResourceRequest::NostrRelay(
            NostrRelayRequest::shared()
        )))
    );
    assert!(observed.child_ready_seen);
    assert!(observed.connection_alive_while_child_running);
    assert!(observed.eof_after_child_exit);

    fs::remove_dir_all(&test_dir).expect("remove test dir");
}

#[tokio::test(flavor = "current_thread")]
async fn request_push_gateway_exports_env_and_keeps_connection_until_child_exits() {
    let test_dir = unique_temp_dir("push-gateway-wrapper");
    fs::create_dir_all(&test_dir).expect("create test dir");
    let marker_path = test_dir.join("child-ready");
    let release_path = test_dir.join("release-child");
    let server = FakeServer::start(
        &test_dir,
        ResourceLease {
            handle_id: ResourceHandleId(8),
            descriptor: fake_push_gateway_descriptor(),
        },
        marker_path.clone(),
        release_path.clone(),
    );

    let script = format!(
        "test \"$DEV_DEFE_PUSH_GATEWAY_URL\" = 'http://127.0.0.1:23456' && \
         test \"$DEV_DEFE_PUSH_GATEWAY_PORT\" = '23456' && \
         test \"$DEV_DEFE_PUSH_GATEWAY_APP_ID\" = 'test-app' && \
         test \"$DEV_DEFE_PUSH_GATEWAY_DATABASE_PATH\" = '/tmp/defe-client-fake-push-gateway.sqlite' && \
         touch '{}' && \
         while [ ! -e '{}' ]; do sleep 0.01; done",
        marker_path.display(),
        release_path.display()
    );
    let wrapper = WrapperArgs {
        requests: vec![ResourceRequestArg::PushGateway {
            sharing: SharingMode::Exclusive,
        }],
        command: os_args(["sh", "-c", &script]),
    };

    let code = run_wrapper_with_socket(wrapper, Some(server.socket_path().as_os_str().to_owned()))
        .await
        .expect("run wrapper");
    let observed = server.join();

    assert_eq!(code, 0);
    assert_eq!(
        observed.request,
        Some(Request::Allocate(ResourceRequest::PushGateway(
            PushGatewayRequest::exclusive()
        )))
    );
    assert!(observed.child_ready_seen);
    assert!(observed.connection_alive_while_child_running);
    assert!(observed.eof_after_child_exit);

    fs::remove_dir_all(&test_dir).expect("remove test dir");
}

#[tokio::test(flavor = "current_thread")]
async fn wrapper_preserves_child_exit_code_without_resource_requests() {
    let wrapper = WrapperArgs {
        requests: Vec::new(),
        command: os_args(["sh", "-c", "exit 17"]),
    };

    let code = run_wrapper_with_socket(wrapper, None)
        .await
        .expect("run wrapper");

    assert_eq!(code, 17);
}

fn os_args<const N: usize>(args: [&str; N]) -> Vec<OsString> {
    args.into_iter().map(OsString::from).collect()
}

fn fake_relay_descriptor() -> ResourceDescriptor {
    ResourceDescriptor::NostrRelay(NostrRelayInfo {
        url: "ws://127.0.0.1:12345".to_owned(),
        host: "127.0.0.1".to_owned(),
        port: 12345,
        data_dir: PathBuf::from("/tmp/defe-client-fake-relay-db"),
    })
}

fn fake_push_gateway_descriptor() -> ResourceDescriptor {
    ResourceDescriptor::PushGateway(PushGatewayInfo {
        url: "http://127.0.0.1:23456".to_owned(),
        host: "127.0.0.1".to_owned(),
        port: 23456,
        app_id: "test-app".to_owned(),
        database_path: PathBuf::from("/tmp/defe-client-fake-push-gateway.sqlite"),
    })
}

#[allow(dead_code)]
fn fake_bitcoind_descriptor() -> ResourceDescriptor {
    ResourceDescriptor::Bitcoind(BitcoindInfo {
        rpc_url: "http://127.0.0.1:34567".to_owned(),
        rpc_host: "127.0.0.1".to_owned(),
        rpc_port: 34567,
        p2p_port: 34568,
        rpc_username: "test-user".to_owned(),
        rpc_password: "test-password".to_owned(),
        data_dir: PathBuf::from("/tmp/defe-client-fake-bitcoind"),
    })
}

/// Minimal scripted `defe` protocol server used as a wrapper test double.
///
/// The wrapper tests need to observe client-side lifecycle details that are hard
/// to assert through the real server, especially that `defe-cli` keeps its
/// connection open while the child process runs and closes it after the child
/// exits. This stub speaks only the required protocol exchange and records those
/// timing-sensitive observations. Real-server tests provide end-to-end coverage;
/// this keeps the wrapper contract test focused and deterministic.
struct FakeServer {
    socket_path: PathBuf,
    thread: thread::JoinHandle<FakeServerObserved>,
}

impl FakeServer {
    fn start(
        test_dir: &std::path::Path,
        lease: ResourceLease,
        marker_path: PathBuf,
        release_path: PathBuf,
    ) -> Self {
        let socket_path = test_dir.join("s");
        let listener =
            std::os::unix::net::UnixListener::bind(&socket_path).expect("bind fake server socket");
        listener
            .set_nonblocking(true)
            .expect("set fake server nonblocking");
        let thread = thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build fake server runtime")
                .block_on(async move {
                    let listener = tokio::net::UnixListener::from_std(listener)
                        .expect("create tokio fake server listener");
                    let Some(mut stream) = accept_with_timeout(&listener).await else {
                        return FakeServerObserved::default();
                    };

                    let request = read_frame_async::<Request>(&mut stream).await.ok();
                    let response = Response::Resource(lease);
                    let frame = defe_api::encode_frame(&response).expect("encode response");
                    tokio::io::AsyncWriteExt::write_all(&mut stream, &frame)
                        .await
                        .expect("write response");

                    let child_ready_seen =
                        wait_for_file(&marker_path, Duration::from_secs(5)).await;
                    let connection_alive_while_child_running =
                        connection_is_alive(&mut stream).await;

                    fs::write(&release_path, b"release").expect("release child");
                    let eof_after_child_exit = read_eof_with_timeout(&mut stream).await;

                    FakeServerObserved {
                        request,
                        child_ready_seen,
                        connection_alive_while_child_running,
                        eof_after_child_exit,
                    }
                })
        });

        Self {
            socket_path,
            thread,
        }
    }

    fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    fn join(self) -> FakeServerObserved {
        self.thread.join().expect("fake server thread exits")
    }
}

#[derive(Default)]
struct FakeServerObserved {
    request: Option<Request>,
    child_ready_seen: bool,
    connection_alive_while_child_running: bool,
    eof_after_child_exit: bool,
}

async fn accept_with_timeout(
    listener: &tokio::net::UnixListener,
) -> Option<tokio::net::UnixStream> {
    match tokio::time::timeout(Duration::from_secs(5), listener.accept()).await {
        Ok(Ok((stream, _addr))) => Some(stream),
        Ok(Err(err)) => panic!("fake server accept failed: {err}"),
        Err(_elapsed) => None,
    }
}

async fn read_frame_async<T>(stream: &mut tokio::net::UnixStream) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    use tokio::io::AsyncReadExt as _;

    let mut len_buf = [0_u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .map_err(|err| format!("failed to read frame length: {err}"))?;
    let payload_len = u32::from_be_bytes(len_buf) as usize;
    let mut frame = Vec::with_capacity(4 + payload_len);
    frame.extend_from_slice(&len_buf);
    frame.resize(4 + payload_len, 0);
    stream
        .read_exact(&mut frame[4..])
        .await
        .map_err(|err| format!("failed to read frame payload: {err}"))?;
    defe_api::decode_frame(&frame).map_err(|err| err.to_string())
}

async fn wait_for_file(path: &std::path::Path, duration: Duration) -> bool {
    tokio::time::timeout(duration, async {
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        loop {
            if path.exists() {
                return;
            }
            interval.tick().await;
        }
    })
    .await
    .is_ok()
}

async fn connection_is_alive(stream: &mut tokio::net::UnixStream) -> bool {
    use tokio::io::AsyncReadExt as _;

    let mut buf = [0_u8; 1];
    tokio::time::timeout(Duration::from_millis(50), stream.read(&mut buf))
        .await
        .is_err()
}

async fn read_eof_with_timeout(stream: &mut tokio::net::UnixStream) -> bool {
    use tokio::io::AsyncReadExt as _;

    let mut buf = [0_u8; 1];
    tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .is_ok_and(|result| result.is_ok_and(|bytes| bytes == 0))
}
