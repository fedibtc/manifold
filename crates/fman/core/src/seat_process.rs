//! One seat's `fedimintd` process boundary: spawn, contain, stop, and wait.
//! (Hosting model and its alternatives: ARCH-fleet-manager-seat-processes.)
//!
//! The per-seat lifecycle loop owns [`SeatProcess`] and the driven client
//! directly. This module owns only the OS boundary and test seam; the seat loop
//! decides when a child has purpose and when a formed child must respawn.
//!
//! The spawn owns the child's entire startup contract. The environment is
//! cleared and rebuilt from a small pass-through allowlist, so nothing in
//! the daemon's environment can alter fedimintd behavior — FMan sets every
//! `FM_*` contract variable itself, and tests assert the dangerous ones
//! are absent. The p2p and api ports bind all interfaces: fedimintd places
//! its iroh UDP sockets at those same addresses, and a loopback-bound UDP
//! socket cannot exchange packets with the internet, which would silently
//! force every peer path through public relays instead of hole-punched
//! direct connections. In iroh mode fedimintd binds no TCP listener at the
//! p2p address; the api address also carries its plaintext WebSocket client
//! API — the same designed-public API it already serves every iroh dialer,
//! with admin verbs gated by `api_auth` — so the wide bind adds a transport
//! to an already-public surface and needs no inbound reachability. The ui
//! and metrics ports stay loopback-only, and the local e2e harness — whose
//! seat keys are port-derived and publicly derivable — keeps all four ports
//! on loopback. The public transport is iroh with
//! deterministic per-seat keys (ARCH-fleet-manager-identity). The
//! seat's `api_auth` is never in env or argv — it travels only through the
//! private driven-DKG socket; when Bitcoin Core is selected its RPC credentials are
//! the only secret handed to the child. Esplora configuration is public.
//!
//! Guarantees the rest of the daemon relies on:
//! - **Daemon exit kills the child, even on SIGKILL** (kill-on-drop plus a
//!   Linux parent-death signal): a leaked fedimintd would squat the seat
//!   port grid and answer the next daemon's clients with a stale
//!   `api_auth`.
//! - **[`SeatProcess::stop`] returns only after the child is reaped**, so a
//!   ceremony restart can safely inspect the final data-directory gate.
//! - **Child output has structural line integrity**: stdout and stderr use
//!   private pipes pumped into tracing events tagged with the seat id. A
//!   bundled child also writes explicitly shareable structured events to its
//!   bounded journal outside the fedimintd data directory.

#[cfg(not(test))]
use std::os::unix::process::CommandExt as _;
use std::os::unix::process::ExitStatusExt as _;
use std::path::PathBuf;
use std::process::Stdio;
#[cfg(test)]
use std::sync::Arc;
use std::time::Duration;

use fedimint_core::{envs::FM_IROH_DNS_ENV, util::SafeUrl};
use fedimint_server::config::driven::DrivenDkgClient;
#[cfg(test)]
use fedimint_server::config::driven::{
    ChildMessage, ChildState, PROTOCOL_VERSION, ParentMessage, read_frame, write_frame,
};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
#[cfg(test)]
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::instrument::WithSubscriber;

#[cfg(not(test))]
use crate::bundled_fedimintd;
use crate::facts::{SeatNo, SeatPorts};
use crate::identity::SeatKeys;
use fedi_decentralized_service_fleet_manager::SeatId;

#[cfg(target_os = "linux")]
mod die_with_parent;

/// Private parent-to-bundled-child path for the safe-event journal.
pub const SAFE_EVENT_DIR_ENV: &str = "FMAN_SAFE_EVENT_DIR";

const HELLO_TIMEOUT: Duration = Duration::from_secs(30);
/// `RunDkg` is a local control handshake, not a ceremony patience limit.
pub(crate) const DKG_START_TIMEOUT: Duration = Duration::from_secs(10);

/// Respawn timing. The defaults are the production policy; tests shrink them.
#[derive(Clone, Copy, Debug)]
pub struct RespawnPolicy {
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    /// A run that stays up at least this long resets the backoff, so a seat
    /// that crashes rarely is not punished for last month's crash loop.
    pub backoff_reset_after: Duration,
}

impl Default for RespawnPolicy {
    fn default() -> Self {
        Self {
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            backoff_reset_after: Duration::from_secs(60),
        }
    }
}

// No `Debug`: holds [`BitcoinBackend`] and potentially a bitcoind password.
#[derive(Clone)]
pub struct SeatProcessConfig {
    pub data_root: PathBuf,
    /// Test double for the seat program; production has no such choice.
    #[cfg(test)]
    pub fedimintd: PathBuf,
    pub bitcoin_network: bitcoin::Network,
    pub bitcoin_backend: BitcoinBackend,
    /// Pkarr HTTP relay used alongside Iroh's default n0 DNS discovery.
    pub iroh_dns: SafeUrl,
}

/// The one chain-data backend supplied to a seat's bundled `fedimintd`.
#[derive(Clone)]
pub enum BitcoinBackend {
    /// Public HTTP Esplora API.
    Esplora(url::Url),
    /// Operator-owned Bitcoin Core JSON-RPC.
    Bitcoind(BitcoindConfig),
}

// No `Debug`: `password` is a credential and must never be formatted.
#[derive(Clone)]
pub struct BitcoindConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedSeatExit {
    pub seat_id: SeatId,
    pub status_code: Option<i32>,
    pub signal: Option<i32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SeatProcessStatus {
    Running { pid: u32 },
    Exited(ObservedSeatExit),
}

#[derive(Debug, Error)]
pub enum SeatProcessError {
    #[error("create seat directory {path}: {source}")]
    CreateSeatDir {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("spawn fedimintd {path}: {source}")]
    Spawn {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("locate own binary to spawn the bundled fedimintd: {source}")]
    LocateSelf { source: std::io::Error },

    #[error("inspect child for {seat_id}: {source}")]
    InspectChild {
        seat_id: SeatId,
        source: std::io::Error,
    },

    #[error("kill child for {seat_id}: {source}")]
    KillChild {
        seat_id: SeatId,
        source: std::io::Error,
    },

    #[cfg(test)]
    #[error("join scripted child task: {0}")]
    ScriptedTaskJoin(tokio::task::JoinError),

    #[cfg(test)]
    #[error("scripted child stop failed")]
    ScriptedStop,
}

/// The process boundary used to start one seat child.
#[derive(Clone)]
pub enum SeatProcessSpawner {
    Bundled,
    #[cfg(test)]
    Fake(Arc<fake::FakeSeatProcessSpawner>),
}

impl SeatProcessSpawner {
    pub(crate) async fn start(
        &self,
        config: &SeatProcessConfig,
        seat_id: SeatId,
        seat_no: SeatNo,
        ports: SeatPorts,
    ) -> Result<SeatProcess, SeatProcessError> {
        match self {
            Self::Bundled => SeatProcess::start(config, seat_id, seat_no, ports).await,
            #[cfg(test)]
            Self::Fake(fake) => fake.start(config, seat_id, seat_no, ports).await,
        }
    }

    #[cfg(test)]
    pub(crate) fn fake(&self) -> &Arc<fake::FakeSeatProcessSpawner> {
        let Self::Fake(fake) = self else {
            panic!("test fleet uses the fake seat process spawner")
        };
        fake
    }
}

/// Owns one live `fedimintd` child. Reports process liveness only; seat
/// health (API probing) is the lifecycle layer's judgment to combine with
/// this.
pub struct SeatProcess {
    seat_id: SeatId,
    child: SeatChild,
    ports: SeatPorts,
    stdout_pump: Option<JoinHandle<()>>,
    stderr_pump: Option<JoinHandle<()>>,
    control: Option<tokio::net::UnixStream>,
}

enum SeatChild {
    Real(Child),
    #[cfg(test)]
    Fake(fake::FakeSeatChild),
}

impl SeatProcess {
    pub async fn start(
        config: &SeatProcessConfig,
        seat_id: SeatId,
        seat_no: SeatNo,
        ports: SeatPorts,
    ) -> Result<Self, SeatProcessError> {
        let (child, stdout_pump, stderr_pump, control) =
            spawn_child(config, &seat_id, seat_no, ports).await?;
        Ok(Self {
            seat_id,
            child: SeatChild::Real(child),
            ports,
            stdout_pump: Some(stdout_pump),
            stderr_pump: Some(stderr_pump),
            control,
        })
    }

    pub(crate) async fn driven_client(
        &mut self,
    ) -> anyhow::Result<DrivenDkgClient<tokio::net::UnixStream>> {
        let control = self
            .control
            .take()
            .expect("driven child has a control socket");
        tokio::time::timeout(HELLO_TIMEOUT, DrivenDkgClient::connect(control))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "driven-DKG child did not send Hello within {}s",
                    HELLO_TIMEOUT.as_secs()
                )
            })?
    }

    pub fn seat_id(&self) -> &SeatId {
        &self.seat_id
    }

    pub fn ports(&self) -> SeatPorts {
        self.ports
    }

    pub fn status(&mut self) -> Result<SeatProcessStatus, SeatProcessError> {
        if let Some(exit) = self.try_exit()? {
            return Ok(SeatProcessStatus::Exited(exit));
        }
        let pid = match &self.child {
            SeatChild::Real(child) => child.id().expect("child not yet reaped has a live pid"),
            #[cfg(test)]
            SeatChild::Fake(_) => 1,
        };
        Ok(SeatProcessStatus::Running { pid })
    }

    /// Wait until the child exits. Cancel-safe: delegates to tokio
    /// [`Child::wait`], so it can be raced in a `select!` loop.
    pub async fn wait(&mut self) -> Result<ObservedSeatExit, SeatProcessError> {
        let status = match &mut self.child {
            SeatChild::Real(child) => {
                child
                    .wait()
                    .await
                    .map_err(|source| SeatProcessError::InspectChild {
                        seat_id: self.seat_id.clone(),
                        source,
                    })?
            }
            #[cfg(test)]
            SeatChild::Fake(child) => return Ok(child.wait().await),
        };
        self.finish_pumps().await;
        Ok(ObservedSeatExit {
            seat_id: self.seat_id.clone(),
            status_code: status.code(),
            signal: status.signal(),
        })
    }

    pub async fn stop(mut self) -> Result<(), SeatProcessError> {
        #[cfg(test)]
        if let SeatChild::Fake(child) = &mut self.child {
            return child.stop().await;
        }
        if self.try_exit()?.is_none()
            && let SeatChild::Real(child) = &mut self.child
            && let Err(source) = child.kill().await
        {
            self.abort_pumps();
            return Err(SeatProcessError::KillChild {
                seat_id: self.seat_id.clone(),
                source,
            });
        }
        self.finish_pumps().await;
        Ok(())
    }

    async fn finish_pumps(&mut self) {
        if let Some(pump) = &mut self.stdout_pump {
            if let Err(err) = pump.await {
                tracing::warn!(seat_id = %self.seat_id, stream = "stdout", %err, "seat log pump failed");
            }
            self.stdout_pump = None;
        }
        if let Some(pump) = &mut self.stderr_pump {
            if let Err(err) = pump.await {
                tracing::warn!(seat_id = %self.seat_id, stream = "stderr", %err, "seat log pump failed");
            }
            self.stderr_pump = None;
        }
    }

    fn abort_pumps(&mut self) {
        if let Some(pump) = self.stdout_pump.take() {
            pump.abort();
        }
        if let Some(pump) = self.stderr_pump.take() {
            pump.abort();
        }
    }

    fn try_exit(&mut self) -> Result<Option<ObservedSeatExit>, SeatProcessError> {
        #[cfg(test)]
        if let SeatChild::Fake(child) = &self.child {
            return Ok(child.try_exit());
        }
        #[cfg(not(test))]
        let SeatChild::Real(child) = &mut self.child;
        #[cfg(test)]
        let child = match &mut self.child {
            SeatChild::Real(child) => child,
            SeatChild::Fake(_) => unreachable!("fake child handled above"),
        };
        let Some(status) = child
            .try_wait()
            .map_err(|source| SeatProcessError::InspectChild {
                seat_id: self.seat_id.clone(),
                source,
            })?
        else {
            return Ok(None);
        };
        Ok(Some(ObservedSeatExit {
            seat_id: self.seat_id.clone(),
            status_code: status.code(),
            signal: status.signal(),
        }))
    }
}

impl Drop for SeatProcess {
    fn drop(&mut self) {
        self.abort_pumps();
    }
}

const MAX_LOG_LINE_BYTES: usize = 64 * 1024;
const LOG_READ_BUFFER_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

async fn pump_output(mut input: impl AsyncRead + Unpin, seat_id: SeatId, stream: OutputStream) {
    let mut read_buffer = [0_u8; LOG_READ_BUFFER_BYTES];
    let mut line = Vec::with_capacity(MAX_LOG_LINE_BYTES);
    let mut chunk_ended_at_limit = false;

    loop {
        match input.read(&mut read_buffer).await {
            Ok(0) => break,
            Ok(read) => {
                for &byte in &read_buffer[..read] {
                    if byte == b'\n' {
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        if !line.is_empty() || !chunk_ended_at_limit {
                            emit_output(&seat_id, stream, &line);
                        }
                        line.clear();
                        chunk_ended_at_limit = false;
                    } else {
                        line.push(byte);
                        if line.len() == MAX_LOG_LINE_BYTES {
                            emit_output(&seat_id, stream, &line);
                            line.clear();
                            chunk_ended_at_limit = true;
                        } else {
                            chunk_ended_at_limit = false;
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!(
                    target: "seat",
                    seat_id = %seat_id,
                    stream = stream.name(),
                    %err,
                    "failed to read fedimintd output"
                );
                tracing::warn!(
                    target: "seat",
                    safe_to_share = true,
                    seat_id = %seat_id,
                    stream = stream.name(),
                    stage = "child_output",
                    failure_kind = "read_failed",
                    "failed to read fedimintd output"
                );
                return;
            }
        }
    }

    if !line.is_empty() {
        emit_output(&seat_id, stream, &line);
    }
}

fn emit_output(seat_id: &SeatId, stream: OutputStream, bytes: &[u8]) {
    let line = String::from_utf8_lossy(bytes);
    tracing::info!(target: "seat", seat_id = %seat_id, stream = stream.name(), "{line}");
}

impl OutputStream {
    fn name(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

async fn spawn_child(
    config: &SeatProcessConfig,
    seat_id: &SeatId,
    seat_no: SeatNo,
    ports: SeatPorts,
) -> Result<
    (
        Child,
        JoinHandle<()>,
        JoinHandle<()>,
        Option<tokio::net::UnixStream>,
    ),
    SeatProcessError,
> {
    let data_dir = seat_data_dir(config, seat_no);
    let directory = seat_dir(config, seat_no);
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|source| SeatProcessError::CreateSeatDir {
            path: directory,
            source,
        })?;

    // Only the local formation harness may replace the install-derived keys
    // with port-derived keys. Those keys are publicly derivable, so harness
    // children keep the old loopback binds: their discovery records then
    // carry only loopback and relay addresses, and the harness dials
    // loopback routes via FM_IROH_CONNECT_OVERRIDES.
    let local_e2e = std::env::var_os("FMAN_E2E_LOCAL_IROH").is_some();
    let iroh_bind_ip = if local_e2e { "127.0.0.1" } else { "0.0.0.0" };
    #[cfg(not(test))]
    let program =
        std::env::current_exe().map_err(|source| SeatProcessError::LocateSelf { source })?;
    #[cfg(test)]
    let program = config.fedimintd.clone();
    let mut command = Command::new(&program);
    #[cfg(not(test))]
    command.as_std_mut().arg0(bundled_fedimintd::ARGV0);
    command
        .env_clear()
        .arg("--data-dir")
        .arg(&data_dir)
        .arg("--bitcoin-network")
        .arg(config.bitcoin_network.to_string())
        // fedimintd binds its iroh UDP sockets at the p2p/api addresses;
        // loopback there cuts off hole-punched direct paths and forces
        // relay-only peering. ui and metrics stay private to the host.
        .arg("--bind-p2p")
        .arg(format!("{iroh_bind_ip}:{}", ports.p2p()))
        .arg("--bind-api")
        .arg(format!("{iroh_bind_ip}:{}", ports.api()))
        .arg("--bind-ui")
        .arg(format!("127.0.0.1:{}", ports.ui()))
        .arg("--bind-metrics")
        .arg(format!("127.0.0.1:{}", ports.metrics()))
        .arg("--enable-iroh")
        .env(SAFE_EVENT_DIR_ENV, safe_event_dir(config, seat_no))
        .env(
            FM_IROH_DNS_ENV,
            config.iroh_dns.clone().to_unsafe().as_str(),
        )
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (parent, child) =
        std::os::unix::net::UnixStream::pair().map_err(|source| SeatProcessError::Spawn {
            path: program.clone(),
            source,
        })?;
    child
        .set_nonblocking(false)
        .map_err(|source| SeatProcessError::Spawn {
            path: program.clone(),
            source,
        })?;
    command
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(child)))
        .env("FM_DKG_CTRL", "1");
    if local_e2e {
        // The bundled child deliberately starts from an empty environment.
        // Forward only the explicit harness marker so its module registry can
        // select hermetic test dependencies too.
        command.env("FMAN_E2E_LOCAL_IROH", "1");
    }
    parent
        .set_nonblocking(true)
        .map_err(|source| SeatProcessError::Spawn {
            path: program.clone(),
            source,
        })?;
    let control = Some(parent);
    match &config.bitcoin_backend {
        BitcoinBackend::Esplora(url) => {
            command.env("FM_ESPLORA_URL", url.as_str());
        }
        BitcoinBackend::Bitcoind(bitcoind) => {
            command
                .env("FM_BITCOIND_URL", &bitcoind.url)
                .env("FM_BITCOIND_USERNAME", &bitcoind.username)
                .env("FM_BITCOIND_PASSWORD", &bitcoind.password);
        }
    }
    // Do not let the daemon's development/package environment silently change
    // fedimintd's transport auth, module config, chain-backend wiring, or metrics.
    // FMan owns every `FM_*` contract var and passes only the intended values
    // above. Keep a tiny non-FM pass-through set for diagnostics and for local
    // Nix-built fedimintd binaries that need a dynamic library path.
    for key in ["LD_LIBRARY_PATH", "RUST_BACKTRACE", "RUST_LOG"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    // A defe-managed FMan gives its fedimintd children Fedimint's own
    // test-environment switch (second-scale instead of minute-scale polling).
    // Derive it from defe rather than accepting an ambient FM_* override.
    if std::env::var_os("DEV_DEFE_SOCKET_PATH").is_some() {
        command.env("FM_IN_DEVIMINT", "1");
    }
    if local_e2e && let Some(value) = std::env::var_os("FM_IROH_CONNECT_OVERRIDES") {
        command.env("FM_IROH_CONNECT_OVERRIDES", value);
    }

    // "FMan exit kills its fedimintd children" (ARCH-fleet-manager)
    // must hold even
    // when the FMan is SIGKILLed and never runs its drops — otherwise every
    // hard kill leaks fedimintds that squat the seat port grid and answer
    // the next FMan's clients with the wrong api_auth.
    #[cfg(target_os = "linux")]
    unsafe {
        let parent_pid = libc::getpid();
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // If the parent died before the prctl took effect the child was
            // already reparented and will never get the signal; bail out.
            if libc::getppid() != parent_pid {
                return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
            }
            Ok(())
        });
    }

    #[cfg(target_os = "linux")]
    let mut child =
        die_with_parent::spawn(command)
            .await
            .map_err(|source| SeatProcessError::Spawn {
                path: program.clone(),
                source,
            })?;
    #[cfg(not(target_os = "linux"))]
    let mut child = command.spawn().map_err(|source| SeatProcessError::Spawn {
        path: program,
        source,
    })?;
    let stdout = child
        .stdout
        .take()
        .expect("fedimintd stdout was configured as piped");
    let stderr = child
        .stderr
        .take()
        .expect("fedimintd stderr was configured as piped");
    let stdout_seat_id = seat_id.clone();
    let stderr_seat_id = seat_id.clone();
    let stdout_pump = tokio::spawn(
        async move {
            pump_output(stdout, stdout_seat_id, OutputStream::Stdout).await;
        }
        .with_current_subscriber(),
    );
    let stderr_pump = tokio::spawn(
        async move {
            pump_output(stderr, stderr_seat_id, OutputStream::Stderr).await;
        }
        .with_current_subscriber(),
    );
    let control = control
        .map(tokio::net::UnixStream::from_std)
        .transpose()
        .map_err(|source| SeatProcessError::Spawn {
            path: program,
            source,
        })?;
    Ok((child, stdout_pump, stderr_pump, control))
}

fn e2e_iroh_key(port: u16, role: &[u8]) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"fman-e2e-local-iroh-v1\0")
        .chain_update(port.to_be_bytes())
        .chain_update(role)
        .finalize()
        .into()
}

pub(crate) fn effective_iroh_p2p_key(keys: &SeatKeys, ports: SeatPorts) -> [u8; 32] {
    if std::env::var_os("FMAN_E2E_LOCAL_IROH").is_some() {
        e2e_iroh_key(ports.p2p(), b"p2p")
    } else {
        keys.iroh_p2p.to_bytes()
    }
}

/// Exact Iroh API secret handed to this seat's child.
///
/// Production uses mnemonic-derived seat material. The local formation/defe
/// harness deliberately substitutes a port-derived key whose loopback route it
/// can precompute; any daemon-side proof over the child's setup code must use
/// this same effective key rather than assuming the production branch.
pub(crate) fn effective_iroh_api_key(keys: &SeatKeys, ports: SeatPorts) -> iroh::SecretKey {
    if std::env::var_os("FMAN_E2E_LOCAL_IROH").is_some() {
        iroh::SecretKey::from_bytes(&e2e_iroh_key(ports.api(), b"api"))
    } else {
        keys.iroh_api.clone()
    }
}

/// Root of one seat's on-disk state: fedimintd data and a
/// separately retained bounded safe-event journal.
pub fn seat_dir(config: &SeatProcessConfig, seat_no: SeatNo) -> PathBuf {
    config.data_root.join("seats").join(seat_no.0.to_string())
}

/// The fedimintd `--data-dir` for one seat. Its existence permanently closes
/// destructive ceremony restart.
pub fn seat_data_dir(config: &SeatProcessConfig, seat_no: SeatNo) -> PathBuf {
    seat_dir(config, seat_no).join("data")
}

/// The bounded explicitly-shareable event journal for one seat.
pub(crate) fn safe_event_dir(config: &SeatProcessConfig, seat_no: SeatNo) -> PathBuf {
    seat_dir(config, seat_no).join("safe-events")
}

#[cfg(test)]
#[path = "../tests/support/fake_child.rs"]
pub(crate) mod fake;

#[cfg(test)]
#[path = "../tests/seat_process.rs"]
mod tests;
