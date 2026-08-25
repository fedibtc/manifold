//! One process-boundary fake for a seat child: driven control plus its real WebSocket API.

use super::*;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use jsonrpsee_server::types::{ErrorObjectOwned, Params};
use jsonrpsee_server::{RpcModule, Server, ServerHandle};
use serde_json::{Value, json};
use tokio::sync::Notify;

/// The meta module's instance id in [`FakeApiState::client_config`].
///
/// Deliberately not 0: a caller that assumed a fixed instance id instead
/// of reading the kind out of the config would miss every endpoint here.
pub(crate) const FAKE_META_INSTANCE_ID: fedimint_core::core::ModuleInstanceId = 2;
pub(crate) const FAKE_LNV2_INSTANCE_ID: fedimint_core::core::ModuleInstanceId = 3;

/// The endpoint names below are string literals because `register_method`
/// needs `&'static str`; this keeps them honest about the id above.
const _: () = assert!(FAKE_META_INSTANCE_ID == 2);

#[derive(Clone, Debug, Default)]
pub(crate) struct FakeApiState {
    /// Complete the driven ceremony after acknowledging RunDkg. False keeps
    /// the child in DKG for lifecycle tests that exercise interruption.
    pub complete_dkg: bool,
    pub consensus_running: bool,
    /// Served by `client_config` once consensus is running.
    pub client_config: Option<Value>,
    /// Served by `invite_code` once consensus is running, in place of the
    /// unparsable placeholder the setup-only fixtures use.
    pub invite_code: Option<String>,
    /// The meta module's current consensus value, if any.
    pub meta_consensus: Option<Vec<u8>>,
    /// The meta module's monotone consensus revision, served with
    /// `meta_consensus`. Starts at 0 for a fixture's initial consensus
    /// (upstream's first adoption) and increments on every later
    /// adoption, exactly as `change_consensus` does — including when the
    /// adopted bytes recur.
    pub meta_revision: u64,
    /// Number of status probes received by the fake child.
    pub probe_calls: usize,
    /// Number of final-config reads received by the fake child.
    pub client_config_calls: usize,
    /// Number of meta-consensus reads received by the fake child.
    pub meta_consensus_calls: usize,
    /// Every value submitted to the meta module, in order.
    pub meta_submissions: Vec<Vec<u8>>,
    /// The `auth` each meta submission carried.
    pub meta_submission_auth: Vec<Option<String>>,
    /// LNv2 gateway URLs accepted by the fake guardian.
    pub lnv2_gateways: std::collections::BTreeSet<String>,
    /// Admin auth carried by each LNv2 gateway insertion.
    pub lnv2_gateway_auth: Vec<Option<String>>,
    /// Record the next meta submission, then answer with an error. This
    /// models the ambiguous accept-before-response boundary of a local
    /// JSON-RPC call.
    pub fail_meta_submit_after_record_once: bool,
    /// Signal when a health probe starts, for deterministic watchdog tests.
    pub probe_entered: Option<Arc<Notify>>,
    /// Hold a health probe until notified, for pinning status independence.
    pub probe_gate: Option<Arc<Notify>>,
    /// Signal when invite lookup starts, for deterministic serialization
    /// tests.
    pub invite_entered: Option<Arc<Notify>>,
    /// Hold invite lookup after a consensus probe, for pinning its
    /// serialization against restart.
    pub invite_gate: Option<Arc<Notify>>,
}

#[derive(Clone)]
pub(crate) struct FakeSeatChildHandle {
    state: Arc<Mutex<FakeApiState>>,
}

fn method_not_found() -> ErrorObjectOwned {
    ErrorObjectOwned::owned(-32601, "Method not found", None::<()>)
}

/// The single positional `{ "auth", "params" }` object.
fn request_params(params: Params<'_>) -> Result<Value, ErrorObjectOwned> {
    let request: Value = params.one()?;
    Ok(request["params"].clone())
}

/// The request's `auth` field, which the real server requires on the meta
/// module's `submit` and ignores on its consensus reads.
fn request_auth(params: Params<'_>) -> Result<Option<String>, ErrorObjectOwned> {
    let request: Value = params.one()?;
    Ok(request["auth"].as_str().map(str::to_owned))
}

struct FakeApiServer {
    handle: ServerHandle,
}

async fn start_api_server(api_port: u16, state: Arc<Mutex<FakeApiState>>) -> FakeApiServer {
    // Rebinding a just-stopped fake's port can race the previous
    // server's TIME_WAIT connections; retry briefly.
    let mut attempts = 0;
    let server = loop {
        match Server::builder().build(("127.0.0.1", api_port)).await {
            Ok(server) => break server,
            Err(err) if attempts < 40 => {
                attempts += 1;
                tracing::debug!(%err, api_port, "fake fedimintd bind retry");
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(err) => panic!("fake fedimintd binds its seat api port: {err}"),
        }
    };

    let mut module = RpcModule::new(state.clone());
    module
        .register_async_method("status", |_params, state, _ext| async move {
            let (probe_entered, probe_gate) = {
                let state = state.lock().unwrap();
                (state.probe_entered.clone(), state.probe_gate.clone())
            };
            if let Some(probe_entered) = probe_entered {
                probe_entered.notify_one();
            }
            if let Some(probe_gate) = probe_gate {
                probe_gate.notified().await;
            }
            let mut state = state.lock().unwrap();
            state.probe_calls += 1;
            if state.consensus_running {
                Ok(json!({ "server": "consensus_running", "federation": null }))
            } else {
                Err(method_not_found())
            }
        })
        .unwrap();
    module
        .register_async_method("invite_code", |_params, state, _ext| async move {
            let (invite_entered, invite_gate) = {
                let state = state.lock().unwrap();
                (state.invite_entered.clone(), state.invite_gate.clone())
            };
            if let Some(invite_entered) = invite_entered {
                invite_entered.notify_one();
            }
            if let Some(invite_gate) = invite_gate {
                invite_gate.notified().await;
            }
            let state = state.lock().unwrap();
            if state.consensus_running {
                Ok(Value::String(
                    state
                        .invite_code
                        .clone()
                        .unwrap_or_else(|| "fed11-fake-invite".to_owned()),
                ))
            } else {
                Err(method_not_found())
            }
        })
        .unwrap();
    module
        .register_method("client_config", |_params, state, _ext| {
            let mut state = state.lock().unwrap();
            state.client_config_calls += 1;
            match (state.consensus_running, state.client_config.clone()) {
                (true, Some(config)) => Ok(config),
                _ => Err(method_not_found()),
            }
        })
        .unwrap();
    module
        .register_method("module_2_get_consensus", |_params, state, _ext| {
            let mut state = state.lock().unwrap();
            state.meta_consensus_calls += 1;
            if !state.consensus_running {
                return Err(method_not_found());
            }
            let revision = state.meta_revision;
            Ok(state
                .meta_consensus
                .as_ref()
                .map(|value| json!({ "revision": revision, "value": hex::encode(value) })))
        })
        .unwrap();
    module
        .register_method("module_2_submit", |params, state, _ext| {
            let auth = request_auth(params.clone())?;
            let request = request_params(params)?;
            let value = request["value"]
                .as_str()
                .and_then(|hex| hex::decode(hex).ok())
                .ok_or_else(|| {
                    ErrorObjectOwned::owned(-32000, "malformed meta value", None::<()>)
                })?;
            let mut state = state.lock().unwrap();
            if !state.consensus_running {
                return Err(method_not_found());
            }
            state.meta_submissions.push(value);
            state.meta_submission_auth.push(auth);
            if state.fail_meta_submit_after_record_once {
                state.fail_meta_submit_after_record_once = false;
                return Err(ErrorObjectOwned::owned(
                    -32000,
                    "meta submit response lost after acceptance",
                    None::<()>,
                ));
            }
            Ok::<_, ErrorObjectOwned>(Value::Null)
        })
        .unwrap();
    module
        .register_method("module_3_add_gateway", |params, state, _ext| {
            let auth = request_auth(params.clone())?;
            let gateway = request_params(params)?
                .as_str()
                .ok_or_else(|| {
                    ErrorObjectOwned::owned(-32000, "malformed gateway URL", None::<()>)
                })?
                .to_owned();
            let mut state = state.lock().unwrap();
            if !state.consensus_running {
                return Err(method_not_found());
            }
            state.lnv2_gateway_auth.push(auth);
            Ok::<_, ErrorObjectOwned>(state.lnv2_gateways.insert(gateway))
        })
        .unwrap();
    FakeApiServer {
        handle: server.start(module),
    }
}

impl FakeApiServer {
    async fn stop(&self) {
        let _ = self.handle.stop();
        self.handle.clone().stopped().await;
        // Let the native client's disconnect watcher evict this server's
        // pooled WebSocket before a replacement immediately rebinds the port.
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

impl FakeSeatChildHandle {
    pub(crate) fn state(&self) -> FakeApiState {
        self.state.lock().unwrap().clone()
    }

    pub(crate) fn set_consensus_running(&self, running: bool) {
        self.state.lock().unwrap().consensus_running = running;
    }

    pub(crate) fn modify_state(&self, update: impl FnOnce(&mut FakeApiState)) {
        update(&mut self.state.lock().unwrap());
    }

    /// Advance live meta consensus independently of the proposal log, as
    /// threshold peers would after accepting a submitted vote.
    pub(crate) fn set_meta_consensus(&self, value: Option<Vec<u8>>) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.meta_revision += 1;
        state.meta_consensus = value;
        state.meta_revision
    }
}

pub(super) struct FakeSeatChild {
    seat_id: SeatId,
    exit: watch::Receiver<Option<ObservedSeatExit>>,
    task: JoinHandle<()>,
    fail_stop: bool,
    install_final_on_stop: bool,
    data_dir: PathBuf,
    api_server: FakeApiServer,
    generation: u64,
    children: Arc<Mutex<HashMap<SeatId, (u64, Weak<Mutex<FakeApiState>>)>>>,
}

impl FakeSeatChild {
    pub(super) fn try_exit(&self) -> Option<ObservedSeatExit> {
        self.exit.borrow().clone()
    }

    pub(super) async fn wait(&mut self) -> ObservedSeatExit {
        while self.exit.borrow().is_none() {
            self.exit.changed().await.expect("fake exit sender is live");
        }
        self.api_server.stop().await;
        self.unregister();
        self.exit.borrow().clone().expect("fake exit was published")
    }

    pub(super) async fn stop(&mut self) -> Result<(), SeatProcessError> {
        if self.fail_stop {
            return Err(SeatProcessError::ScriptedStop);
        }
        self.task.abort();
        match (&mut self.task).await {
            Ok(()) => {}
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(SeatProcessError::ScriptedTaskJoin(error)),
        }
        self.api_server.stop().await;
        self.unregister();
        if self.install_final_on_stop {
            tokio::fs::create_dir_all(&self.data_dir)
                .await
                .map_err(|source| SeatProcessError::CreateSeatDir {
                    path: self.data_dir.clone(),
                    source,
                })?;
        }
        Ok(())
    }

    fn unregister(&self) {
        let mut children = self.children.lock().unwrap();
        if children
            .get(&self.seat_id)
            .is_some_and(|(generation, _)| *generation == self.generation)
        {
            children.remove(&self.seat_id);
        }
    }
}

/// One wire-level action performed by a scripted in-process child.
#[cfg(test)]
#[derive(Clone)]
pub(crate) enum FakeDkgStep {
    Message(ChildMessage),
    Crash,
    Hang,
    FailStop,
    InstallFinalOnStop,
}

/// Process-boundary fake whose tasks speak the real driven-DKG framing.
#[cfg(test)]
pub struct FakeSeatProcessSpawner {
    scripts: Mutex<VecDeque<Vec<FakeDkgStep>>>,
    spawn_count: AtomicUsize,
    request_count: Arc<AtomicUsize>,
    next_generation: AtomicU64,
    children: Arc<Mutex<HashMap<SeatId, (u64, Weak<Mutex<FakeApiState>>)>>>,
}

#[cfg(test)]
impl Default for FakeSeatProcessSpawner {
    fn default() -> Self {
        Self {
            scripts: Default::default(),
            spawn_count: Default::default(),
            request_count: Default::default(),
            next_generation: Default::default(),
            children: Default::default(),
        }
    }
}

#[cfg(test)]
impl FakeSeatProcessSpawner {
    pub(crate) fn scripted(sessions: impl IntoIterator<Item = Vec<Vec<FakeDkgStep>>>) -> Self {
        Self {
            scripts: std::sync::Mutex::new(sessions.into_iter().flatten().collect()),
            ..Default::default()
        }
    }

    pub(crate) fn spawn_count(&self) -> usize {
        self.spawn_count.load(Ordering::Relaxed)
    }

    pub(crate) fn request_count(&self) -> usize {
        self.request_count.load(Ordering::Relaxed)
    }

    pub(crate) async fn configure(
        &self,
        seat_id: &SeatId,
        initial: FakeApiState,
    ) -> FakeSeatChildHandle {
        loop {
            let state = self
                .children
                .lock()
                .unwrap()
                .get(seat_id)
                .and_then(|(_, state)| state.upgrade());
            if let Some(state) = state {
                *state.lock().unwrap() = initial;
                return FakeSeatChildHandle { state };
            }
            tokio::task::yield_now().await;
        }
    }
}

#[cfg(test)]
impl FakeSeatProcessSpawner {
    pub(super) async fn start(
        &self,
        config: &SeatProcessConfig,
        seat_id: SeatId,
        seat_no: SeatNo,
        ports: SeatPorts,
    ) -> Result<SeatProcess, SeatProcessError> {
        self.spawn_count.fetch_add(1, Ordering::Relaxed);
        let mut steps = self.scripts.lock().unwrap().pop_front();
        let fail_stop = steps.as_ref().is_some_and(|steps| {
            steps
                .iter()
                .any(|step| matches!(step, FakeDkgStep::FailStop))
        });
        let install_final_on_stop = steps.as_ref().is_some_and(|steps| {
            steps
                .iter()
                .any(|step| matches!(step, FakeDkgStep::InstallFinalOnStop))
        });
        if let Some(steps) = &mut steps {
            steps.retain(|step| {
                !matches!(
                    step,
                    FakeDkgStep::FailStop | FakeDkgStep::InstallFinalOnStop
                )
            });
        }
        let (parent, child) = tokio::net::UnixStream::pair().expect("create fake control socket");
        let (exit, exit_rx) = watch::channel(None);
        let task_seat_id = seat_id.clone();
        let child_seat_id = seat_id.clone();
        let request_count = self.request_count.clone();
        let api_state = Arc::new(Mutex::new(FakeApiState::default()));
        let api_server = start_api_server(ports.api(), api_state.clone()).await;
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        self.children
            .lock()
            .unwrap()
            .insert(seat_id.clone(), (generation, Arc::downgrade(&api_state)));
        let data_dir = seat_data_dir(config, seat_no);
        let stop_data_dir = data_dir.clone();
        let task_api_state = api_state.clone();
        let task = tokio::spawn(async move {
            run_fake_child(
                child,
                steps,
                task_seat_id,
                data_dir,
                exit,
                request_count,
                task_api_state,
            )
            .await;
        });
        Ok(SeatProcess {
            seat_id,
            child: SeatChild::Fake(FakeSeatChild {
                seat_id: child_seat_id,
                exit: exit_rx,
                task,
                fail_stop,
                install_final_on_stop,
                data_dir: stop_data_dir,
                api_server,
                generation,
                children: self.children.clone(),
            }),
            ports,
            stdout_pump: None,
            stderr_pump: None,
            control: Some(parent),
        })
    }
}

#[cfg(test)]
async fn run_fake_child(
    mut control: tokio::net::UnixStream,
    steps: Option<Vec<FakeDkgStep>>,
    seat_id: SeatId,
    data_dir: PathBuf,
    exit: watch::Sender<Option<ObservedSeatExit>>,
    request_count: Arc<std::sync::atomic::AtomicUsize>,
    api_state: Arc<Mutex<FakeApiState>>,
) {
    use tokio::io::AsyncWriteExt as _;

    let mut steps = steps.unwrap_or_default().into_iter().peekable();
    let state = match steps.peek() {
        Some(FakeDkgStep::Message(ChildMessage::Hello { state, .. })) => {
            let state = state.clone();
            steps.next();
            state
        }
        _ => ChildState::NeedsParams,
    };
    if matches!(state, ChildState::AlreadyConfigured { .. }) {
        let _ = tokio::fs::create_dir_all(&data_dir).await;
    }
    if write_frame(
        &mut control,
        &ChildMessage::Hello {
            proto: PROTOCOL_VERSION,
            code_version: "fake-fedimintd".to_owned(),
            state: state.clone(),
        },
    )
    .await
    .is_err()
    {
        return;
    }
    if state == ChildState::NeedsParams
        && read_frame::<_, ParentMessage>(&mut control).await.is_err()
    {
        return;
    }
    if state == ChildState::NeedsParams {
        request_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    if steps.len() == 0 {
        if state == ChildState::NeedsParams {
            if write_frame(&mut control, &ChildMessage::DkgStarted {})
                .await
                .is_err()
            {
                return;
            }
            if !api_state.lock().unwrap().complete_dkg {
                std::future::pending::<()>().await;
            }
            let invite_code = {
                let mut state = api_state.lock().unwrap();
                state.consensus_running = true;
                state
                    .invite_code
                    .clone()
                    .unwrap_or_else(|| "fed11-fake-invite".to_owned())
            };
            let _ = tokio::fs::create_dir_all(&data_dir).await;
            let _ = write_frame(
                &mut control,
                &ChildMessage::ConfigPersisted {
                    invite_code,
                    api_url: String::new(),
                },
            )
            .await;
        }
        let _ = write_frame(&mut control, &ChildMessage::ConsensusStarted {}).await;
        let _ = control.shutdown().await;
        std::future::pending::<()>().await;
    }

    for step in steps {
        match step {
            FakeDkgStep::Hang => std::future::pending::<()>().await,
            FakeDkgStep::Message(message) => {
                if matches!(message, ChildMessage::ConfigPersisted { .. }) {
                    let _ = tokio::fs::create_dir_all(&data_dir).await;
                }
                let terminal_failure = matches!(
                    message,
                    ChildMessage::ParamsRejected { .. } | ChildMessage::DkgFailed { .. }
                );
                let consensus = matches!(message, ChildMessage::ConsensusStarted {});
                if write_frame(&mut control, &message).await.is_err() {
                    return;
                }
                if terminal_failure {
                    let _ = control.shutdown().await;
                    exit.send_replace(Some(ObservedSeatExit {
                        seat_id,
                        status_code: Some(1),
                        signal: None,
                    }));
                    return;
                }
                if consensus {
                    let _ = control.shutdown().await;
                    std::future::pending::<()>().await;
                }
            }
            FakeDkgStep::Crash => {
                drop(control);
                exit.send_replace(Some(ObservedSeatExit {
                    seat_id,
                    status_code: Some(1),
                    signal: None,
                }));
                return;
            }
            FakeDkgStep::FailStop => unreachable!("stop marker is removed before child starts"),
            FakeDkgStep::InstallFinalOnStop => {
                unreachable!("stop marker is removed before child starts")
            }
        }
    }
    std::future::pending::<()>().await;
}

/// A script line that keeps a spawned shell test child alive. The production
/// spawn clears `PATH`, so resolve the executable before writing the script.
pub(crate) fn block_forever() -> String {
    let sleep = std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("sleep"))
                .find(|path| path.is_file())
        })
        .expect("sleep exists on the test PATH");
    format!("exec {} 600", sleep.display())
}

pub(crate) async fn write_fake_fedimintd(dir: &std::path::Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    let path = dir.join("fake-fedimintd");
    tokio::fs::write(&path, format!("#!/bin/sh\n{body}"))
        .await
        .unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}
