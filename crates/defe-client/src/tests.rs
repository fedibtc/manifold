use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use defe_api::BitcoindInfo;
use defe_api::{NostrRelayInfo, ResourceHandleId, ResourceLease};

use super::*;
use crate::test_support::unique_temp_dir;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(flavor = "current_thread")]
async fn connect_from_env_errors_clearly_when_env_is_missing() {
    let _guard = ENV_LOCK.lock().await;
    let previous = env::var_os(DEV_DEFE_SOCKET_PATH);
    unsafe {
        env::remove_var(DEV_DEFE_SOCKET_PATH);
    }

    let err = AsyncDefeClient::connect_from_env()
        .await
        .expect_err("missing env should fail");
    let message = err.to_string();
    assert!(message.contains(DEV_DEFE_SOCKET_PATH));
    assert!(message.contains("defe exec <cmd...>"));

    restore_env(previous);
}

#[tokio::test(flavor = "current_thread")]
async fn connect_from_env_errors_clearly_when_env_is_empty() {
    let _guard = ENV_LOCK.lock().await;
    let previous = env::var_os(DEV_DEFE_SOCKET_PATH);
    unsafe {
        env::set_var(DEV_DEFE_SOCKET_PATH, "");
    }

    let err = AsyncDefeClient::connect_from_env()
        .await
        .expect_err("empty env should fail");
    let message = err.to_string();
    assert!(message.contains(DEV_DEFE_SOCKET_PATH));
    assert!(message.contains("empty"));
    assert!(!message.contains("failed to connect"));

    restore_env(previous);
}

#[tokio::test(flavor = "current_thread")]
async fn connect_from_env_uses_socket_path_from_env() {
    let test_dir = unique_temp_dir("connect-from-env");
    fs::create_dir_all(&test_dir).expect("create test dir");
    let socket_path = test_dir.join("s");
    let server = FakeServer::start(&socket_path, vec![Response::Pong]);

    let _guard = ENV_LOCK.lock().await;
    let previous = env::var_os(DEV_DEFE_SOCKET_PATH);
    unsafe {
        env::set_var(DEV_DEFE_SOCKET_PATH, &socket_path);
    }

    let mut client = AsyncDefeClient::connect_from_env()
        .await
        .expect("connect from env");
    client.ping().await.expect("ping fake server");

    restore_env(previous);
    let observed = server.join();
    assert_eq!(observed, vec![Request::Ping]);
    fs::remove_dir_all(&test_dir).expect("remove test dir");
}

#[tokio::test(flavor = "current_thread")]
async fn convenience_methods_send_expected_requests() {
    let test_dir = unique_temp_dir("async-convenience-methods");
    fs::create_dir_all(&test_dir).expect("create test dir");
    let socket_path = test_dir.join("s");
    let lease = fake_relay_lease(7);
    let restarted = fake_relay_lease(7);
    let bitcoind = fake_bitcoind_lease(9);
    let server = FakeServer::start(
        &socket_path,
        vec![
            Response::Resource(lease.clone()),
            Response::Resource(bitcoind.clone()),
            Response::Resource(restarted.clone()),
            Response::Released,
        ],
    );

    let mut client = AsyncDefeClient::connect(&socket_path)
        .await
        .expect("connect explicit socket");
    assert_eq!(
        client
            .request_nostr_relay(SharingMode::Shared)
            .await
            .expect("request relay"),
        lease
    );
    assert_eq!(
        client
            .request_bitcoind(SharingMode::Exclusive)
            .await
            .expect("request bitcoind"),
        bitcoind
    );
    assert_eq!(
        client
            .restart(ResourceHandleId(7), RestartMode::Force)
            .await
            .expect("restart relay"),
        restarted
    );
    client
        .release(ResourceHandleId(7))
        .await
        .expect("release relay");

    let observed = server.join();
    assert_eq!(
        observed,
        vec![
            Request::Allocate(ResourceRequest::NostrRelay(NostrRelayRequest::shared())),
            Request::Allocate(ResourceRequest::Bitcoind(BitcoindRequest::exclusive())),
            Request::Restart {
                handle_id: ResourceHandleId(7),
                mode: RestartMode::Force,
            },
            Request::Release(ResourceHandleId(7)),
        ]
    );
    fs::remove_dir_all(&test_dir).expect("remove test dir");
}

#[tokio::test(flavor = "current_thread")]
async fn restart_rejects_lease_with_unexpected_handle_id() {
    let test_dir = unique_temp_dir("async-restart-wrong-handle");
    fs::create_dir_all(&test_dir).expect("create test dir");
    let socket_path = test_dir.join("s");
    let server = FakeServer::start(&socket_path, vec![Response::Resource(fake_relay_lease(8))]);

    let mut client = AsyncDefeClient::connect(&socket_path)
        .await
        .expect("connect explicit socket");
    let err = client
        .restart(ResourceHandleId(7), RestartMode::Force)
        .await
        .expect_err("restart should reject mismatched handle id");
    match err {
        DefeClientError::RestartHandleMismatch {
            requested,
            returned,
        } => {
            assert_eq!(requested, ResourceHandleId(7));
            assert_eq!(returned, ResourceHandleId(8));
        }
        err => panic!("unexpected error: {err}"),
    }

    let observed = server.join();
    assert_eq!(
        observed,
        vec![Request::Restart {
            handle_id: ResourceHandleId(7),
            mode: RestartMode::Force,
        }]
    );
    fs::remove_dir_all(&test_dir).expect("remove test dir");
}

fn restore_env(previous: Option<OsString>) {
    unsafe {
        if let Some(previous) = previous {
            env::set_var(DEV_DEFE_SOCKET_PATH, previous);
        } else {
            env::remove_var(DEV_DEFE_SOCKET_PATH);
        }
    }
}

fn fake_relay_lease(handle_id: u64) -> ResourceLease {
    ResourceLease {
        handle_id: ResourceHandleId(handle_id),
        descriptor: ResourceDescriptor::NostrRelay(NostrRelayInfo {
            url: "ws://127.0.0.1:12345".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 12345,
            data_dir: PathBuf::from("/tmp/defe-client-lib-fake-relay-db"),
        }),
    }
}

fn fake_bitcoind_lease(handle_id: u64) -> ResourceLease {
    ResourceLease {
        handle_id: ResourceHandleId(handle_id),
        descriptor: ResourceDescriptor::Bitcoind(BitcoindInfo {
            rpc_url: "http://127.0.0.1:34567".to_owned(),
            rpc_host: "127.0.0.1".to_owned(),
            rpc_port: 34567,
            p2p_port: 34568,
            rpc_username: "test-user".to_owned(),
            rpc_password: "test-password".to_owned(),
            data_dir: PathBuf::from("/tmp/defe-client-lib-fake-bitcoind"),
        }),
    }
}

/// Minimal scripted `defe` protocol server used as a client test double.
///
/// These tests intentionally do not start the real `defe` server: they verify the
/// async client API in isolation, including the exact request sequence and
/// response handling, without involving resource-manager state, subprocesses, or
/// real relay setup. End-to-end coverage belongs in separate real-server tests;
/// this stub keeps narrow client contract tests fast and deterministic.
struct FakeServer {
    thread: thread::JoinHandle<Vec<Request>>,
}

impl FakeServer {
    fn start(socket_path: &Path, responses: Vec<Response>) -> Self {
        let listener =
            std::os::unix::net::UnixListener::bind(socket_path).expect("bind fake server socket");
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
                        return Vec::new();
                    };
                    let mut requests = Vec::new();
                    for response in responses {
                        let request = read_frame_async::<Request>(&mut stream)
                            .await
                            .expect("read request");
                        requests.push(request);
                        let frame = defe_api::encode_frame(&response).expect("encode response");
                        tokio::io::AsyncWriteExt::write_all(&mut stream, &frame)
                            .await
                            .expect("write response");
                    }
                    requests
                })
        });

        Self { thread }
    }

    fn join(self) -> Vec<Request> {
        self.thread.join().expect("fake server thread exits")
    }
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
