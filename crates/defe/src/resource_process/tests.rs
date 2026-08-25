use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use defe_api::ResourceDescriptor;

use super::*;
use crate::resource_manager::{
    ManagedResource, ResourceAllocation, ResourceDriver, ResourceKind, ResourceManager,
    ResourceSlotId, ResourceSpec, fake_nostr_descriptor,
};

#[test]
fn redirects_stdout_and_stderr_to_log_files() {
    let test_dir = TestDir::new("logs");
    let stdout_log = test_dir.path().join("nested/stdout.log");
    let stderr_log = test_dir.path().join("nested/stderr.log");
    let process = ResourceProcess::spawn(
        ResourceProcessConfig::new("sh", &stdout_log, &stderr_log)
            .arg("-c")
            .arg("printf 'stdout-line\\n'; printf 'stderr-line\\n' >&2"),
    )
    .expect("spawn lightweight process");

    let status = process.wait().expect("wait for process");

    assert!(status.success());
    assert!(!process.is_running());
    assert_eq!(process.exit_status(), Some(status));
    assert_eq!(
        fs::read_to_string(process.stdout_log()).unwrap(),
        "stdout-line\n"
    );
    assert_eq!(
        fs::read_to_string(process.stderr_log()).unwrap(),
        "stderr-line\n"
    );
}

#[test]
fn exited_processes_are_reaped_without_explicit_polling() {
    let test_dir = TestDir::new("active-reap");
    let process = ResourceProcess::spawn(
        ResourceProcessConfig::new(
            "sh",
            test_dir.path().join("stdout.log"),
            test_dir.path().join("stderr.log"),
        )
        .arg("-c")
        .arg("exit 0"),
    )
    .expect("spawn short-lived process");
    let pid = process.pid();

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut reaped = false;
    while Instant::now() < deadline {
        if !process_is_alive(pid) && process.exit_status().is_some() {
            reaped = true;
            break;
        }
        thread::sleep(REAPER_POLL_INTERVAL);
    }

    assert!(reaped, "short-lived process was not reaped before timeout");
    assert!(
        process
            .exit_status()
            .expect("exit status recorded")
            .success()
    );
}

#[test]
fn stop_kills_and_waits_for_running_process() {
    let test_dir = TestDir::new("stop");
    let process = sleep_process(&test_dir, "explicit-stop", 60);
    assert!(process.is_running());

    let status = process.stop().expect("stop running process");

    assert!(!status.success());
    assert!(!process.is_running());
    assert_eq!(process.exit_status(), Some(status));
}

#[test]
fn drop_kills_and_waits_for_running_process() {
    let test_dir = TestDir::new("drop");
    let process = sleep_process(&test_dir, "drop-stop", 60);
    let pid = process.pid();
    assert!(process_is_alive(pid));

    drop(process);

    assert!(!process_is_alive(pid));
}

#[test]
fn process_resource_composes_with_restart_if_exited() {
    let test_dir = TestDir::new("restart-if-exited");
    let driver = ProcessDriver::new(test_dir.path().to_owned(), ProcessKind::ExitImmediately);
    let manager = Arc::new(ResourceManager::new(driver.clone_driver()));
    let mut connection = manager.connection();
    let lease = connection
        .allocate(ResourceSpec::exclusive(ResourceKind::Fake))
        .expect("allocate process-backed resource");

    for _ in 0..100 {
        if driver.running_slot_count() == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(driver.running_slot_count(), 0);

    let restarted = connection
        .restart(lease.handle_id, defe_api::RestartMode::IfExited)
        .expect("restart after process has exited");

    assert_eq!(driver.start_count(), 2);
    assert_eq!(restarted.handle_id, lease.handle_id);
    assert_ne!(restarted.descriptor, lease.descriptor);
}

#[test]
fn process_resource_composes_with_force_restart() {
    let test_dir = TestDir::new("force-restart");
    let driver = ProcessDriver::new(test_dir.path().to_owned(), ProcessKind::Sleep);
    let manager = Arc::new(ResourceManager::new(driver.clone_driver()));
    let mut connection = manager.connection();
    let lease = connection
        .allocate(ResourceSpec::exclusive(ResourceKind::Fake))
        .expect("allocate process-backed resource");
    let slot_id = slot_id_from_descriptor(&lease.descriptor);
    assert_eq!(driver.running_slot_count(), 1);

    let restarted = connection
        .restart(lease.handle_id, defe_api::RestartMode::Force)
        .expect("force restart running process");

    assert_eq!(driver.start_count(), 2);
    assert_eq!(driver.running_slot_count(), 1);
    assert_eq!(slot_id_from_descriptor(&restarted.descriptor), slot_id);
    assert_ne!(restarted.descriptor, lease.descriptor);
}

fn sleep_process(test_dir: &TestDir, name: &str, seconds: u64) -> ResourceProcess {
    ResourceProcess::spawn(
        ResourceProcessConfig::new(
            "sleep",
            test_dir.path().join(format!("{name}.out.log")),
            test_dir.path().join(format!("{name}.err.log")),
        )
        .arg(seconds.to_string()),
    )
    .expect("spawn sleep process")
}

fn process_is_alive(pid: u32) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("kill -0 {pid} 2>/dev/null"))
        .status()
        .expect("run kill -0")
        .success()
}

fn slot_id_from_descriptor(descriptor: &ResourceDescriptor) -> ResourceSlotId {
    let ResourceDescriptor::NostrRelay(info) = descriptor else {
        panic!("expected Nostr relay descriptor, got {descriptor:?}");
    };
    let slot = info
        .url
        .strip_prefix("fake://slot-")
        .and_then(|rest| rest.split_once('/'))
        .map(|(slot, _generation)| slot)
        .expect("fake descriptor url includes slot id");
    ResourceSlotId(slot.parse().expect("slot id is numeric"))
}

#[test]
fn log_tail_reports_an_empty_log_as_empty() {
    let test_dir = TestDir::new("log-tail-empty");
    let log = test_dir.path().join("empty.log");
    fs::write(&log, b"").expect("write empty log");

    let quoted = log_tail(&log);

    assert!(quoted.contains("is empty"), "unexpected quote: {quoted}");
    assert!(quoted.contains(&log.display().to_string()));
}

#[test]
fn log_tail_quotes_the_end_of_a_log() {
    let test_dir = TestDir::new("log-tail-content");
    let log = test_dir.path().join("relay.log");
    fs::write(&log, b"early line\nAddress already in use (os error 98)\n").expect("write log");

    let quoted = log_tail(&log);

    assert!(
        quoted.contains("Address already in use"),
        "unexpected quote: {quoted}"
    );
}

#[test]
fn log_tail_keeps_only_the_last_bytes_of_a_long_log() {
    let test_dir = TestDir::new("log-tail-long");
    let log = test_dir.path().join("long.log");
    let filler = "x".repeat(usize::try_from(LOG_TAIL_BYTES).expect("tail fits in usize") * 2);
    fs::write(&log, format!("{filler}\nthe last line\n")).expect("write long log");

    let quoted = log_tail(&log);

    assert!(
        quoted.contains("the last line"),
        "unexpected quote: {quoted}"
    );
    assert!(
        quoted.len() < usize::try_from(LOG_TAIL_BYTES).expect("tail fits in usize") + 512,
        "quote kept the whole log: {} bytes",
        quoted.len()
    );
}

#[test]
fn log_tail_reports_a_missing_log_instead_of_failing() {
    let test_dir = TestDir::new("log-tail-missing");
    let log = test_dir.path().join("absent.log");

    let quoted = log_tail(&log);

    assert!(
        quoted.contains("could not be read"),
        "unexpected quote: {quoted}"
    );
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
        let path = std::env::temp_dir().join(format!(
            "defe-resource-process-test-{name}-{}-{now}-{id}",
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

#[derive(Clone)]
struct ProcessDriver {
    log_root: PathBuf,
    kind: ProcessKind,
    inner: Arc<Mutex<ProcessDriverState>>,
}

impl ProcessDriver {
    fn new(log_root: PathBuf, kind: ProcessKind) -> Self {
        Self {
            log_root,
            kind,
            inner: Arc::new(Mutex::new(ProcessDriverState::default())),
        }
    }

    fn clone_driver(&self) -> Arc<dyn ResourceDriver> {
        Arc::new(self.clone())
    }

    fn start_count(&self) -> usize {
        self.inner.lock().expect("process driver mutex").starts
    }

    fn running_slot_count(&self) -> usize {
        self.inner
            .lock()
            .expect("process driver mutex")
            .processes
            .values()
            .filter(|process| process.is_running())
            .count()
    }
}

impl ResourceDriver for ProcessDriver {
    fn start(
        &self,
        allocation: &ResourceAllocation,
    ) -> Result<Box<dyn ManagedResource>, defe_api::ApiError> {
        let command = match self.kind {
            ProcessKind::ExitImmediately => {
                let mut command = Command::new("sh");
                command.arg("-c").arg("exit 0");
                command
            }
            ProcessKind::Sleep => {
                let mut command = Command::new("sleep");
                command.arg("60");
                command
            }
        };
        let stdout_log = self.log_root.join(format!(
            "slot-{}-generation-{}.out.log",
            allocation.slot_id.0, allocation.generation
        ));
        let stderr_log = self.log_root.join(format!(
            "slot-{}-generation-{}.err.log",
            allocation.slot_id.0, allocation.generation
        ));
        let process = Arc::new(
            ResourceProcess::spawn_command(command, stdout_log, stderr_log).map_err(|err| {
                defe_api::ApiError::new(
                    defe_api::ApiErrorKind::ResourceStartFailed,
                    err.to_string(),
                )
            })?,
        );
        let descriptor = fake_nostr_descriptor(allocation.slot_id, allocation.generation);
        let mut inner = self.inner.lock().expect("process driver mutex");
        inner.starts += 1;
        inner
            .processes
            .insert(allocation.slot_id, Arc::clone(&process));
        Ok(Box::new(ProcessManagedResource {
            process,
            descriptor,
        }))
    }
}

#[derive(Clone, Copy)]
enum ProcessKind {
    ExitImmediately,
    Sleep,
}

#[derive(Default)]
struct ProcessDriverState {
    starts: usize,
    processes: HashMap<ResourceSlotId, Arc<ResourceProcess>>,
}

struct ProcessManagedResource {
    process: Arc<ResourceProcess>,
    descriptor: ResourceDescriptor,
}

impl ManagedResource for ProcessManagedResource {
    fn descriptor(&self) -> ResourceDescriptor {
        self.descriptor.clone()
    }

    fn is_running(&self) -> bool {
        self.process.is_running()
    }

    fn stop(&mut self) {
        let _ = self.process.stop();
    }
}
