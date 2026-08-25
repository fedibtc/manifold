use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use defe_api::{RestartMode, SharingMode};

use super::*;
use crate::resource_manager::{
    ResourceManager, ResourceSharing, ResourceSlotId, ResourceSpec, SharedResourceKey,
};

const RUN_REAL_RELAY_TESTS_ENV: &str = "DEV_DEFE_RUN_REAL_NOSTR_RELAY_TESTS";
const REAL_RELAY_BIN_ENV: &str = "DEV_DEFE_NOSTR_RS_RELAY_BIN";

#[test]
fn toml_basic_string_escapes_paths() {
    assert_eq!(toml_basic_string("/tmp/a\\b\"c"), "/tmp/a\\\\b\\\"c");
}

#[test]
fn relay_log_filter_defaults_to_info_so_the_log_is_never_empty() {
    assert_eq!(relay_log_filter(None), OsString::from("info"));
}

#[test]
fn relay_log_filter_keeps_an_inherited_level() {
    assert_eq!(
        relay_log_filter(Some(OsString::from("debug"))),
        OsString::from("debug")
    );
}

#[test]
fn nostr_relay_logs_are_generation_specific() {
    assert_eq!(
        nostr_relay_log_path(Path::new("/tmp/logs"), ResourceSlotId(7), 3),
        PathBuf::from("/tmp/logs/nostr-relay-slot-7-generation-3.log")
    );
}

#[tokio::test]
#[ignore = "opt-in real relay test; run in nix develop and set DEV_DEFE_RUN_REAL_NOSTR_RELAY_TESTS=1"]
async fn real_nostr_relay_lifecycle_and_restart() {
    if env::var_os(RUN_REAL_RELAY_TESTS_ENV).as_deref() != Some(std::ffi::OsStr::new("1")) {
        eprintln!("skipping real nostr relay test; set {RUN_REAL_RELAY_TESTS_ENV}=1 to run");
        return;
    }

    let test_dir = TestDir::new("real-nostr-relay");
    let relay_bin = env::var_os(REAL_RELAY_BIN_ENV).unwrap_or_else(|| "nostr-rs-relay".into());
    let driver = Arc::new(NostrRelayDriver::new(
        relay_bin,
        test_dir.path().join("resources"),
        test_dir.path().join("logs"),
    ));
    let manager = Arc::new(ResourceManager::new(driver.clone()));

    let mut first = manager.connection();
    let shared = first
        .allocate(nostr_spec(SharingMode::Shared))
        .expect("start shared nostr relay");
    let ResourceDescriptor::NostrRelay(shared_info) = &shared.descriptor else {
        panic!(
            "expected Nostr relay descriptor, got {:?}",
            shared.descriptor
        );
    };
    assert_eq!(shared_info.host, NOSTR_RELAY_HOST);
    assert!(shared_info.data_dir.is_dir());
    assert_relay_accepts_tcp(shared_info).await;

    let mut second = manager.connection();
    let reused = second
        .allocate(nostr_spec(SharingMode::Shared))
        .expect("reuse shared nostr relay");
    assert_eq!(reused.descriptor, shared.descriptor);

    let exclusive = second
        .allocate(nostr_spec(SharingMode::Exclusive))
        .expect("start exclusive nostr relay");
    assert_ne!(exclusive.descriptor, shared.descriptor);

    drop(second);

    let forced = first
        .restart(shared.handle_id, RestartMode::Force)
        .expect("force restart shared nostr relay");
    assert_eq!(forced.handle_id, shared.handle_id);
    assert_eq!(forced.descriptor, shared.descriptor);

    driver.stop_only_running_process_for_test();
    let restarted = first
        .restart(shared.handle_id, RestartMode::IfExited)
        .expect("restart exited nostr relay");
    assert_eq!(restarted.handle_id, shared.handle_id);
    assert_eq!(restarted.descriptor, shared.descriptor);

    first
        .release(shared.handle_id)
        .expect("release shared nostr relay");
}

fn nostr_spec(sharing: SharingMode) -> ResourceSpec {
    let sharing = match sharing {
        SharingMode::Shared => ResourceSharing::Shared(SharedResourceKey::NostrRelay),
        SharingMode::Exclusive => ResourceSharing::Exclusive,
    };
    ResourceSpec {
        kind: ResourceKind::NostrRelay,
        sharing,
    }
}

async fn assert_relay_accepts_tcp(info: &NostrRelayInfo) {
    tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect((info.host.as_str(), info.port)),
    )
    .await
    .expect("relay accepts TCP connections before timeout")
    .expect("relay accepts TCP connections");
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "defe-nostr-relay-test-{name}-{}-{now}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test directory");
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
