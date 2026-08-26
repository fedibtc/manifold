//! Keeps a formed local federation and its supporting services alive for manual use.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::future::Future;
use std::os::fd::{AsFd as _, AsRawFd as _};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};
use defe_client::{
    AsyncDefeClient, FlipRequest, FmanInfo, FmanRequest, GatewaydInfo, GatewaydRequest,
    ResourceDescriptor, SharingMode,
};
use iroh_base_035::{NodeAddr, NodeId, SecretKey, ticket::NodeTicket};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;
use tokio::process::Command;

#[derive(Debug)]
struct SetupCommandTimeout;

impl std::fmt::Display for SetupCommandTimeout {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("setup command timed out after environment teardown")
    }
}

impl std::error::Error for SetupCommandTimeout {}

#[cfg(target_os = "linux")]
mod descendant_supervisor;
#[cfg(not(target_os = "linux"))]
#[path = "descendant_supervisor_unsupported.rs"]
mod descendant_supervisor;
mod flip_setup;
mod synthetic_remit;
mod traffic;

use descendant_supervisor::{DescendantSupervisor, run_lease_guard, run_namespace_spawn};

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
    bitcoin_cli: PathBuf,
    load_test_tool: PathBuf,
    complete_liquidity: bool,
    command: Vec<OsString>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest<'a> {
    schema_version: u8,
    ready: bool,
    state: &'static str,
    federation: FederationManifest<'a>,
    fmans: Vec<FmanManifest<'a>>,
    bitcoin: BitcoinManifest<'a>,
    nostr_relay_url: &'a str,
    gateway: GatewayManifest<'a>,
    flip: FlipManifest<'a>,
    logs_dir: &'a Path,
    secrets_file: &'a Path,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FederationManifest<'a> {
    invite_file: &'a Path,
    fi_state_dir: &'a Path,
    fi_account_file: &'a Path,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FmanManifest<'a> {
    number: usize,
    seat_id: &'a str,
    locator: &'a str,
    api_base_url: &'a str,
    auth_url: String,
    admin_url: String,
    data_dir: &'a Path,
    safe_journal_dir: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BitcoinManifest<'a> {
    rpc_url: &'a str,
    data_dir: &'a Path,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GatewayManifest<'a> {
    api_url: &'a str,
    state: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FlipManifest<'a> {
    admin_url: &'a str,
    data_dir: &'a Path,
    state: &'static str,
    public_endpoint_id: &'a str,
}

fn main() {
    let raw_args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if raw_args
        .first()
        .is_some_and(|arg| arg == "--internal-child-gate")
    {
        run_internal_child_gate(&raw_args);
    }
    if raw_args
        .first()
        .is_some_and(|arg| arg == "--internal-fork-adversary")
    {
        run_internal_fork_adversary(&raw_args);
    }
    if raw_args
        .first()
        .is_some_and(|arg| arg == "--internal-timeout-probe")
    {
        run_internal_timeout_probe(&raw_args);
    }
    if raw_args
        .first()
        .is_some_and(|arg| arg == "--internal-unblock-term-probe")
    {
        unsafe {
            let mut signals = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&raw mut signals);
            libc::sigaddset(&raw mut signals, libc::SIGTERM);
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const signals, std::ptr::null_mut());
            libc::raise(libc::SIGTERM);
            libc::_exit(127);
        }
    }
    if raw_args
        .first()
        .is_some_and(|arg| arg == "--internal-namespace-spawn")
    {
        run_namespace_spawn(&raw_args);
    }
    if raw_args
        .first()
        .is_some_and(|arg| arg == "--internal-lease-guard")
    {
        run_lease_guard(&raw_args);
    }
    if raw_args
        .first()
        .is_some_and(|arg| arg == "--internal-write-fd")
    {
        let fd = raw_args
            .get(1)
            .and_then(|arg| arg.to_str())
            .and_then(|arg| arg.parse::<i32>().ok())
            .unwrap_or_else(|| std::process::exit(127));
        unsafe {
            libc::_exit(i32::from(
                libc::write(fd, b"sentinel\n".as_ptr().cast(), 9) != 9,
            ))
        };
    }
    if raw_args
        .first()
        .is_some_and(|arg| arg == "--internal-fd-occupation-test")
    {
        run_internal_fd_occupation_test(&raw_args);
    }
    let supervisor = match raw_args.first().and_then(|argument| argument.to_str()) {
        Some("--internal-with-lock" | "--internal-synthetic-remit" | "--internal-traffic") => None,
        _ => match DescendantSupervisor::establish() {
            Ok(supervisor) => Some(Arc::new(supervisor)),
            Err(error) => {
                eprintln!("defe env: {error:#}");
                std::process::exit(1);
            }
        },
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("defe env: create async runtime: {error}");
            std::process::exit(1);
        }
    };
    let status = runtime.block_on(async_main(raw_args, supervisor));
    match status {
        Ok(status) => exit_with_status(status),
        Err(error) => {
            eprintln!("defe env: {error:#}");
            std::process::exit(1);
        }
    }
}

fn run_internal_fd_occupation_test(args: &[OsString]) -> ! {
    let sentinel = args
        .get(1)
        .and_then(|arg| arg.to_str())
        .and_then(|arg| arg.parse::<i32>().ok())
        .unwrap_or_else(|| std::process::exit(127));
    let closed_stdio = args
        .get(2)
        .and_then(|arg| arg.to_str())
        .and_then(|arg| arg.parse::<u8>().ok())
        .unwrap_or_else(|| std::process::exit(127));
    let null = std::fs::File::open("/dev/null").unwrap_or_else(|_| std::process::exit(127));
    for fd in 3..=102 {
        if unsafe { libc::dup2(null.as_raw_fd(), fd) } < 0 {
            std::process::exit(127);
        }
    }
    if unsafe { libc::dup2(sentinel, 100) } < 0 {
        std::process::exit(127);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|_| std::process::exit(127));
    for fd in 0..=2 {
        if closed_stdio & (1 << fd) != 0 {
            unsafe { libc::close(fd) };
        }
    }
    let supervisor = DescendantSupervisor::establish().unwrap_or_else(|_| std::process::exit(127));
    let status = runtime
        .block_on(async {
            let mut setup = Command::new("sh");
            setup.args(["-c", "printf captured-setup-output"]);
            let captured = run_command(
                &supervisor,
                &mut setup,
                "closed-stdio capture probe",
                Duration::from_secs(1),
            )
            .await?;
            ensure!(
                captured == "captured-setup-output",
                "explicit setup stdout capture was overridden"
            );
            let mut original = Command::new(std::env::current_exe().unwrap());
            original.env("DEFE_ENV_TEST_PRESERVE_CLOSED_STDIO", "1");
            let mut command = supervisor.wrap(&original, false)?;
            command.command.arg("--internal-write-fd").arg("100");
            let mut child = supervisor.spawn(command)?;
            Ok::<_, anyhow::Error>(child.child.wait().await?)
        })
        .unwrap_or_else(|_| std::process::exit(127));
    supervisor
        .terminate_and_reap()
        .unwrap_or_else(|_| std::process::exit(127));
    std::process::exit(exit_code(status));
}

async fn async_main(
    raw_args: Vec<OsString>,
    supervisor: Option<Arc<DescendantSupervisor>>,
) -> Result<ExitStatus> {
    match raw_args.first().and_then(|argument| argument.to_str()) {
        Some("--internal-with-lock") => run_with_lock(&raw_args[1..]).await,
        Some("--internal-supervisor-test") => {
            let supervisor = supervisor.context("missing test descendant supervisor")?;
            run_internal_supervisor_test(&supervisor, &raw_args[1..]).await
        }
        Some("--internal-pty-test") => {
            let supervisor = supervisor.context("missing PTY test descendant supervisor")?;
            run_internal_pty_test(&supervisor, &raw_args[1..]).await
        }
        Some("--internal-abrupt-owner-test") => {
            let supervisor = supervisor.context("missing abrupt-owner test supervisor")?;
            run_internal_abrupt_owner_test(&supervisor, &raw_args[1..]).await
        }
        Some("--internal-signal-status-test") => {
            let supervisor = supervisor.context("missing signal-status test supervisor")?;
            run_internal_signal_status_test(&supervisor).await
        }
        Some("--internal-timeout-test") => {
            let supervisor = supervisor.context("missing timeout test supervisor")?;
            run_internal_timeout_test(&supervisor, &raw_args[1..]).await
        }
        Some("--internal-synthetic-remit") => synthetic_remit::run(&raw_args[1..])
            .await
            .map(|()| ExitStatus::from_raw(0)),
        Some("--internal-traffic") => traffic::run(&raw_args[1..])
            .await
            .map(|()| ExitStatus::from_raw(0)),
        _ => {
            let result = parse_args(raw_args).and_then(|args| {
                if args.complete_liquidity {
                    bail!(
                        "--complete-liquidity is not implemented yet; basic environment remains available without it"
                    );
                }
                Ok(args)
            });
            match result {
                Ok(args) => {
                    let supervisor =
                        supervisor.context("missing environment descendant supervisor")?;
                    run(args, supervisor).await
                }
                Err(error) => Err(error),
            }
        }
    }
}

fn run_internal_fork_adversary(args: &[OsString]) -> ! {
    let Some(marker) = args.get(1) else {
        std::process::exit(127);
    };
    if fs::write(marker, b"ready\n").is_err() {
        std::process::exit(127);
    }
    loop {
        let child = unsafe { libc::fork() };
        if child == 0 {
            let grandchild = unsafe { libc::fork() };
            if grandchild == 0 {
                loop {
                    unsafe { libc::pause() };
                }
            }
            unsafe { libc::_exit(i32::from(grandchild < 0)) };
        }
        if child < 0 {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        let mut status = 0;
        while unsafe { libc::waitpid(child, &mut status, 0) } < 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        {}
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn run_internal_timeout_probe(args: &[OsString]) -> ! {
    let Some(marker) = args.get(1) else {
        std::process::exit(127);
    };
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    let host_pid = status
        .lines()
        .find_map(|line| line.strip_prefix("NSpid:"))
        .and_then(|pids| pids.split_whitespace().next())
        .unwrap_or("0");
    if fs::write(marker, format!("{host_pid}\n")).is_err() {
        std::process::exit(127);
    }
    if unsafe { libc::fork() } == 0 {
        loop {
            unsafe { libc::pause() };
        }
    }
    loop {
        unsafe { libc::pause() };
    }
}

async fn run_internal_supervisor_test(
    supervisor: &DescendantSupervisor,
    args: &[OsString],
) -> Result<ExitStatus> {
    let [marker] = args else {
        bail!("internal supervisor test requires a marker path");
    };
    let command = Command::new(std::env::current_exe()?);
    let mut command = supervisor.wrap(&command, false)?;
    command.command.arg("--internal-fork-adversary").arg(marker);
    let child = supervisor.spawn(command)?;
    let child_pid = child.command_pid;
    drop(child.child);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !Path::new(marker).exists() {
        ensure!(
            tokio::time::Instant::now() < deadline,
            "fork adversary did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut failed = Command::new("true");
    failed.env("DEFE_ENV_TEST_BROKER_FAILURE", "1");
    let failed = supervisor.wrap(&failed, false)?;
    ensure!(
        supervisor.spawn(failed).is_err(),
        "injected broker failure unexpectedly admitted a command"
    );
    supervisor.inject_helper_open_failure();
    let helper_open_failure = Command::new("sleep");
    let mut helper_open_failure = supervisor.wrap(&helper_open_failure, false)?;
    helper_open_failure.command.arg("600");
    ensure!(
        supervisor.spawn(helper_open_failure).is_err(),
        "injected helper identity failure lost ownership"
    );
    let process_group = Command::new("sleep");
    let mut process_group = process_group;
    process_group.env("DEFE_ENV_TEST_CHILD_FIRST_PGRP", "1");
    let mut process_group = supervisor.wrap(&process_group, true)?;
    process_group.command.arg("600");
    let process_group = supervisor.spawn(process_group)?;
    ensure!(
        unsafe { libc::getpgid(process_group.command_pid) } == process_group.command_pid,
        "broker published the command before its process group existed"
    );
    drop(process_group.child);
    tokio::time::sleep(Duration::from_millis(100)).await;
    supervisor.inject_test_failures(1, 1);
    supervisor.terminate_and_reap()?;
    ensure!(
        unsafe { libc::kill(child_pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH),
        "fork adversary survived namespace teardown"
    );
    let late = Command::new("true");
    let late = supervisor.wrap(&late, false)?;
    ensure!(
        supervisor.spawn(late).is_err(),
        "teardown reopened subprocess admission"
    );
    Ok(ExitStatus::from_raw(0))
}

async fn run_internal_timeout_test(
    supervisor: &DescendantSupervisor,
    args: &[OsString],
) -> Result<ExitStatus> {
    let [marker] = args else {
        bail!("internal timeout test requires a marker path");
    };
    let mut timeout_probe = Command::new(std::env::current_exe()?);
    timeout_probe.arg("--internal-timeout-probe").arg(marker);
    let error = run_command(
        supervisor,
        &mut timeout_probe,
        "gateway-cli connect-fed",
        Duration::from_millis(100),
    )
    .await
    .expect_err("timeout probe unexpectedly completed");
    ensure!(
        error.downcast_ref::<SetupCommandTimeout>().is_some(),
        "timeout returned the wrong error: {error:#}"
    );
    let timed_out_pid = fs::read_to_string(marker)?.trim().parse::<i32>()?;
    ensure!(
        unsafe { libc::kill(timed_out_pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH),
        "timed-out gateway command overlapped its retry"
    );
    let retry = Command::new("true");
    let retry = supervisor.wrap(&retry, false)?;
    ensure!(
        supervisor.spawn(retry).is_err(),
        "gateway retry admitted work after timeout teardown"
    );
    Ok(ExitStatus::from_raw(0))
}

async fn run_internal_pty_test(
    supervisor: &DescendantSupervisor,
    args: &[OsString],
) -> Result<ExitStatus> {
    let [root] = args else {
        bail!("internal PTY test requires a root path");
    };
    let root = PathBuf::from(root);
    fs::create_dir_all(&root)?;
    let args = Args {
        root: root.clone(),
        logs_dir: root.join("logs"),
        fi_cli: "/bin/false".into(),
        fman_cli: "/bin/false".into(),
        gateway_cli: "/bin/false".into(),
        bitcoin_cli: "/bin/false".into(),
        load_test_tool: "/bin/false".into(),
        complete_liquidity: false,
        command: vec![OsString::from("sh"), OsString::from("-i")],
    };
    run_child(
        &args.command,
        &args,
        &root.join("env.json"),
        &root.join("secrets.json"),
        &root.join("invite"),
        &root.join("fi"),
        &root.join("routes"),
        &root.join("bin"),
        "ws://relay",
        "http://gateway",
        "http://flip",
        "flip-id",
        supervisor,
    )
    .await
}

async fn run_internal_abrupt_owner_test(
    supervisor: &DescendantSupervisor,
    args: &[OsString],
) -> Result<ExitStatus> {
    let [socket, marker] = args else {
        bail!("internal abrupt-owner test requires socket and marker paths");
    };
    let connection = std::os::unix::net::UnixStream::connect(socket)?;
    supervisor.guard_connection(connection.as_fd().try_clone_to_owned()?)?;
    ensure!(
        supervisor
            .guard_connection(connection.as_fd().try_clone_to_owned()?)
            .is_err(),
        "duplicate lease lifetime guard was admitted"
    );
    let command = Command::new("sleep");
    let mut command = supervisor.wrap(&command, false)?;
    command.command.arg("600");
    let child = supervisor.spawn(command)?;
    fs::write(marker, format!("{}\n", child.command_pid))?;
    std::mem::forget(connection);
    std::mem::forget(child.child);
    std::future::pending().await
}

async fn run_internal_signal_status_test(supervisor: &DescendantSupervisor) -> Result<ExitStatus> {
    let original = Command::new(std::env::current_exe()?);
    let mut command = supervisor.wrap(&original, false)?;
    command.command.arg("--internal-unblock-term-probe");
    let mut child = supervisor.spawn(command)?;
    let status = child.child.wait().await?;
    supervisor.terminate_and_reap()?;
    Ok(status)
}

async fn run_with_lock(raw_args: &[OsString]) -> Result<ExitStatus> {
    let (lock_path, command) = match raw_args {
        [lock, path, separator, command @ ..]
            if lock == "--lock" && separator == "--" && !command.is_empty() =>
        {
            (PathBuf::from(path), command)
        }
        _ => bail!("internal lock invocation requires --lock PATH -- COMMAND..."),
    };
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open environment lock {}", lock_path.display()))?;
    if unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        bail!(
            "lock environment command at {}: {}",
            lock_path.display(),
            std::io::Error::last_os_error()
        );
    }
    let mut child = Command::new(&command[0]);
    child.args(&command[1..]).kill_on_drop(true);
    let status = child
        .status()
        .await
        .with_context(|| format!("run locked command {}", command[0].to_string_lossy()))?;
    drop(lock_file);
    Ok(status)
}

fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
}

fn exit_with_status(status: ExitStatus) -> ! {
    if let Some(signal) = status.signal() {
        raise_default_signal(signal);
    }
    std::process::exit(status.code().unwrap_or(1))
}

fn raise_default_signal(signal: i32) -> ! {
    unsafe {
        let mut signals = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(&raw mut signals);
        libc::sigaddset(&raw mut signals, signal);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const signals, std::ptr::null_mut());
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
        libc::_exit(128 + signal);
    }
}

fn run_internal_child_gate(args: &[OsString]) -> ! {
    let fd = |index: usize| {
        args.get(index)
            .and_then(|arg| arg.to_str())
            .and_then(|arg| arg.parse::<i32>().ok())
            .unwrap_or_else(|| std::process::exit(127))
    };
    let read_fd = fd(1);
    unsafe { libc::close(fd(2)) };
    let mut byte = 0_u8;
    if unsafe { libc::read(read_fd, (&raw mut byte).cast(), 1) } != 1 {
        std::process::exit(127);
    }
    unsafe { libc::close(read_fd) };
    let command = args.get(4).unwrap_or_else(|| std::process::exit(127));
    let error = std::process::Command::new(command).args(&args[5..]).exec();
    eprintln!("defe env: execute {}: {error}", command.to_string_lossy());
    std::process::exit(127);
}

async fn run(args: Args, incoming_supervisor: Arc<DescendantSupervisor>) -> Result<ExitStatus> {
    fs::create_dir_all(&args.root)
        .with_context(|| format!("create environment root {}", args.root.display()))?;
    set_private(&args.root)?;
    // These owners are declared before the supervisor so Rust drops the
    // supervisor first on every error and async-cancellation path.
    let mut defe;
    let mut held_leases = Vec::new();
    // This local must drop before the lease-owning client declared above.
    let supervisor = incoming_supervisor;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let signal_supervisor = Arc::downgrade(&supervisor);
    let signal_manifest = args.root.join("env.json");
    tokio::spawn(async move {
        let signal = tokio::select! {
            _ = interrupt.recv() => libc::SIGINT,
            _ = terminate.recv() => libc::SIGTERM,
        };
        let Some(supervisor) = signal_supervisor.upgrade() else {
            return;
        };
        if let Err(error) = supervisor.terminate_and_reap() {
            eprintln!("defe env: terminate descendants: {error:#}");
            return;
        }
        invalidate_ready_manifest(&signal_manifest);
        // Process exit closes the defe connection and releases leases only after
        // every environment descendant has been terminated and reaped.
        raise_default_signal(signal);
    });
    defe = AsyncDefeClient::connect_from_env()
        .await
        .context("connect to the defe server")?;
    supervisor.guard_connection(
        defe.duplicate_lifetime_guard()
            .context("duplicate Defe connection for lifetime guard")?,
    )?;

    status("allocating regtest Bitcoin and Nostr relay");
    let bitcoin_lease = defe.request_bitcoind(SharingMode::Exclusive).await?;
    let ResourceDescriptor::Bitcoind(bitcoin) = bitcoin_lease.descriptor.clone() else {
        bail!("defe returned the wrong descriptor for bitcoind");
    };
    held_leases.push(bitcoin_lease);
    let relay_lease = defe.request_nostr_relay(SharingMode::Exclusive).await?;
    let ResourceDescriptor::NostrRelay(relay) = relay_lease.descriptor.clone() else {
        bail!("defe returned the wrong descriptor for the Nostr relay");
    };
    held_leases.push(relay_lease);

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
        held_leases.push(lease);
        fmans.push(fman);
    }
    for fman in &fmans {
        run_command(
            &supervisor,
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
    let invite = form_federation(&supervisor, &args.fi_cli, &fi_state_dir, &fmans, &routes).await?;
    let invite_file = args.root.join("federation-invite");
    write_private(&invite_file, invite.as_bytes())?;
    let fi_account_file = fi_state_dir.join("fi-spv2-account.json");
    let seat_ids = read_seat_ids(&supervisor, &args.fman_cli, &fmans).await?;

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
    held_leases.push(gateway_lease);
    connect_gateway(&supervisor, &args.gateway_cli, &gateway, &invite).await?;

    status("starting FLIP");
    let flip_lease = defe
        .request_flip(FlipRequest {
            sharing: SharingMode::Exclusive,
            iroh_connect_overrides: Some(routes.clone()),
            holder_authorization_relay_url: Some(relay.url.clone()),
        })
        .await?;
    let ResourceDescriptor::Flip(flip) = flip_lease.descriptor.clone() else {
        bail!("defe returned the wrong descriptor for FLIP");
    };
    held_leases.push(flip_lease);
    status("configuring FLIP and publishing its advertisement");
    let public_endpoint_id =
        flip_setup::configure_and_publish(&flip, &gateway, &bitcoin, &relay.url).await?;

    let secrets_file = args.root.join("secrets.json");
    write_private(
        &secrets_file,
        serde_json::to_vec_pretty(&serde_json::json!({
            "fmans": fmans.iter().map(|fman| &fman.admin_password).collect::<Vec<_>>(),
            "gateway_password": gateway.password,
            "flip_admin_token": flip.admin_token,
        }))?
        .as_slice(),
    )?;
    let manifest = Manifest {
        schema_version: 1,
        ready: true,
        state: "ready",
        federation: FederationManifest {
            invite_file: &invite_file,
            fi_state_dir: &fi_state_dir,
            fi_account_file: &fi_account_file,
        },
        fmans: fmans
            .iter()
            .zip(&seat_ids)
            .enumerate()
            .map(|(index, (fman, seat_id))| FmanManifest {
                number: index + 1,
                seat_id,
                locator: &fman.locator,
                api_base_url: &fman.admin_url,
                auth_url: fman_api_url(&fman.admin_url, "auth"),
                admin_url: fman_api_url(&fman.admin_url, "admin"),
                data_dir: &fman.data_dir,
                safe_journal_dir: fman.data_dir.join("safe-events"),
            })
            .collect(),
        bitcoin: BitcoinManifest {
            rpc_url: &bitcoin.rpc_url,
            data_dir: &bitcoin.data_dir,
        },
        nostr_relay_url: &relay.url,
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
    let routes_file = args.root.join("iroh-connect-overrides");
    write_private(&routes_file, routes.as_bytes())?;
    let manifest_file = args.root.join("env.json");
    let bin_dir = args.root.join("bin");
    let defe_env = std::env::current_exe().context("locate the running defe-env binary")?;
    write_tools(
        &bin_dir,
        &args,
        &defe_env,
        &manifest_file,
        &secrets_file,
        &invite_file,
        &fi_state_dir,
        &routes,
        &fmans,
        &seat_ids,
        &gateway,
        &bitcoin,
    )?;
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

    let child_result = run_child(
        &args.command,
        &args,
        &manifest_file,
        &secrets_file,
        &invite_file,
        &fi_state_dir,
        &routes_file,
        &bin_dir,
        &relay.url,
        &gateway.api_url,
        &flip.admin_url,
        &public_endpoint_id,
        &supervisor,
    )
    .await;
    // `run_child` can fail before it spawns the command. Drain here as well as
    // inside its normal path so every error observes the lifetime boundary
    // before publishing stopped state.
    supervisor.terminate_and_reap()?;
    let stopped_manifest = stopped_manifest(&manifest)?;
    if let Err(error) = write_json_atomic(&manifest_file, &stopped_manifest) {
        // Never leave a retained ready manifest after the lease boundary.
        let _ = fs::remove_file(&manifest_file);
        return Err(error).context("invalidate environment manifest before teardown");
    }
    eprintln!("defe env: command exited; releasing all leased resources");
    let status = child_result?;
    drop((held_leases, defe));
    Ok(status)
}

async fn form_federation(
    supervisor: &DescendantSupervisor,
    fi_cli: &Path,
    state_dir: &Path,
    fmans: &[FmanInfo],
    routes: &str,
) -> Result<String> {
    run_command(
        supervisor,
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
    for fman in fmans {
        command.arg("--locator").arg(&fman.locator);
    }
    let output = run_command(
        supervisor,
        &mut command,
        "fi-cli create",
        Duration::from_secs(150),
    )
    .await?;
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

async fn connect_gateway(
    supervisor: &DescendantSupervisor,
    cli: &Path,
    gateway: &GatewaydInfo,
    invite: &str,
) -> Result<()> {
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
            supervisor,
            &mut command,
            "gateway-cli connect-fed",
            Duration::from_secs(15),
        )
        .await
        {
            Ok(_) => return Ok(()),
            Err(error) if error.downcast_ref::<SetupCommandTimeout>().is_some() => {
                return Err(error);
            }
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn read_seat_ids(
    supervisor: &DescendantSupervisor,
    fman_cli: &Path,
    fmans: &[FmanInfo],
) -> Result<Vec<String>> {
    let mut seat_ids = Vec::with_capacity(fmans.len());
    for fman in fmans {
        let output = run_command(
            supervisor,
            Command::new(fman_cli)
                .arg("--data-dir")
                .arg(&fman.data_dir)
                .arg("seats")
                .arg("list"),
            "fman-cli seats list",
            Duration::from_secs(15),
        )
        .await?;
        let value: serde_json::Value = serde_json::from_str(&output)?;
        seat_ids.push(
            value["seats"][0]["seat_id"]
                .as_str()
                .context("formed FMan has no recorded seat id")?
                .to_owned(),
        );
    }
    Ok(seat_ids)
}

#[allow(clippy::too_many_arguments)]
fn write_tools(
    bin_dir: &Path,
    args: &Args,
    defe_env: &Path,
    manifest: &Path,
    secrets: &Path,
    invite: &Path,
    fi_state_dir: &Path,
    routes: &str,
    fmans: &[FmanInfo],
    seat_ids: &[String],
    gateway: &GatewaydInfo,
    bitcoin: &defe_client::BitcoindInfo,
) -> Result<()> {
    fs::create_dir_all(bin_dir)?;
    set_private(bin_dir)?;

    write_wrapper(
        &bin_dir.join("defe-env-info"),
        &format!(
            "#!/bin/sh\nprintf '%s: %s\\n' 'Defe environment manifest' {} 'Secrets' {} 'Invite' {} 'Logs' {}\ncat {}\n",
            shell_escape(manifest.as_os_str()),
            shell_escape(secrets.as_os_str()),
            shell_escape(invite.as_os_str()),
            shell_escape(args.logs_dir.as_os_str()),
            shell_escape(manifest.as_os_str())
        ),
    )?;
    for (index, fman) in fmans.iter().enumerate() {
        write_wrapper(
            &bin_dir.join(format!("fman-{}", index + 1)),
            &format!(
                "#!/bin/sh\nexec {} --data-dir {} \"$@\"\n",
                shell_escape(args.fman_cli.as_os_str()),
                shell_escape(fman.data_dir.as_os_str())
            ),
        )?;
    }
    let locked_fi_cli = bin_dir.join("fi-cli-locked");
    write_wrapper(
        &locked_fi_cli,
        &format!(
            "#!/bin/sh\nFMAN_E2E_LOCAL_IROH=1 FM_IROH_CONNECT_OVERRIDES={} exec {} --internal-with-lock --lock {} -- {} \"$@\"\n",
            shell_escape(OsStr::new(routes)),
            shell_escape(defe_env.as_os_str()),
            shell_escape(args.root.join("fi-cli.lock").as_os_str()),
            shell_escape(args.fi_cli.as_os_str()),
        ),
    )?;
    write_wrapper(
        &bin_dir.join("fi-cli"),
        &format!(
            "#!/bin/sh\nexec {} --state-dir {} \"$@\"\n",
            shell_escape(locked_fi_cli.as_os_str()),
            shell_escape(fi_state_dir.as_os_str())
        ),
    )?;
    write_wrapper(
        &bin_dir.join("gateway"),
        &format!(
            "#!/bin/sh\nexec {} --address {} --rpcpassword={} \"$@\"\n",
            shell_escape(args.gateway_cli.as_os_str()),
            shell_escape(OsStr::new(&gateway.api_url)),
            shell_escape(OsStr::new(&gateway.password))
        ),
    )?;
    write_wrapper(
        &bin_dir.join("bitcoin-cli"),
        &format!(
            "#!/bin/sh\nexec {} -regtest -datadir={} -rpcuser={} -rpcpassword={} -rpcport={} \"$@\"\n",
            shell_escape(args.bitcoin_cli.as_os_str()),
            shell_escape(bitcoin.data_dir.as_os_str()),
            shell_escape(OsStr::new(&bitcoin.rpc_username)),
            shell_escape(OsStr::new(&bitcoin.rpc_password)),
            bitcoin.rpc_port
        ),
    )?;

    let mut ui = String::from("#!/bin/sh\ncase \"${1-}\" in\n");
    for (index, fman) in fmans.iter().enumerate() {
        writeln!(
            ui,
            "  {}) target={}; password={} ;;",
            index + 1,
            shell_escape(OsStr::new(&fman.admin_url)),
            shell_escape(OsStr::new(&fman.admin_password))
        )?;
    }
    ui.push_str(
        "  *) echo 'usage: fman-ui GUARDIAN' >&2; exit 2 ;;\nesac\n\
         echo \"FMan UI: http://127.0.0.1:5174  password: $password\" >&2\n\
         VITE_MOCKS=off FMAN_ADMIN_PROXY_TARGET=\"$target\" exec pnpm --dir ",
    );
    ui.push_str(&shell_escape(OsStr::new(FMAN_OPERATOR_UI_DIR)));
    ui.push_str(" --filter fman exec vite --host 127.0.0.1\n");
    write_wrapper(&bin_dir.join("fman-ui"), &ui)?;

    let mut fees = String::from(
        "#!/bin/sh\nusage() { echo 'usage: fees show --guardian N [ARGS...] | fees collect (--guardian N | --all) | fees synthetic-remit --guardian N --amount-msats AMOUNT' >&2; exit 2; }\n\
         run_one() { n=$1; shift; case \"$n\" in\n",
    );
    for (index, seat_id) in seat_ids.iter().enumerate() {
        writeln!(
            fees,
            "  {}) tool={}; seat={} ;;",
            index + 1,
            shell_escape(bin_dir.join(format!("fman-{}", index + 1)).as_os_str()),
            shell_escape(OsStr::new(seat_id))
        )?;
    }
    fees.push_str(
        "  *) usage ;; esac\n\
         if [ \"$verb\" = collect ]; then\n\
           \"$tool\" guardian-fees collect \"$seat\" \"$@\"; collect_status=$?\n\
           echo \"Post-collect guardian $n status:\" >&2\n\
           \"$tool\" guardian-fees show \"$seat\" || true\n\
           return \"$collect_status\"\n\
         fi\n\
         \"$tool\" guardian-fees show \"$seat\" \"$@\"\n}\n\
         verb=${1-}; shift || usage\n\
         case \"$verb\" in\n\
           show|collect)\n\
             guardian=\nall=0\n\
             while [ \"$#\" -gt 0 ]; do case \"$1\" in --guardian) [ \"$#\" -ge 2 ] || usage; guardian=$2; shift 2 ;; --all) all=1; shift ;; *) break ;; esac; done\n\
             [ \"$all\" -eq 1 ] && [ -n \"$guardian\" ] && usage\n\
             if [ \"$verb\" = show ]; then [ \"$all\" -eq 0 ] && [ -n \"$guardian\" ] || usage; run_one \"$guardian\" \"$@\"\n\
             elif [ \"$all\" -eq 1 ]; then for n in 1 2 3 4 5 6 7; do run_one \"$n\" \"$@\" || exit; done\n\
             elif [ -n \"$guardian\" ]; then run_one \"$guardian\" \"$@\"\n\
             else usage; fi ;;\n\
           synthetic-remit)\n\
             guardian=\namount=\n\
             while [ \"$#\" -gt 0 ]; do case \"$1\" in --guardian) [ \"$#\" -ge 2 ] || usage; guardian=$2; shift 2 ;; --amount-msats) [ \"$#\" -ge 2 ] || usage; amount=$2; shift 2 ;; *) usage ;; esac; done\n\
             [ -n \"$guardian\" ] && [ -n \"$amount\" ] || usage\n\
             case \"$guardian\" in\n",
    );
    for (index, seat_id) in seat_ids.iter().enumerate() {
        writeln!(
            fees,
            "               {}) tool={}; data_dir={}; seat={} ;;",
            index + 1,
            shell_escape(args.fman_cli.as_os_str()),
            shell_escape(fmans[index].data_dir.as_os_str()),
            shell_escape(OsStr::new(seat_id))
        )?;
    }
    fees.push_str(&format!(
        "               *) usage ;; esac\n\
             exec {} --internal-with-lock --lock {} -- {} --internal-synthetic-remit --root {} --fman-cli \"$tool\" --fman-data-dir \"$data_dir\" --fi-cli {} --bitcoin-cli {} --invite-file {} --guardian \"$guardian\" --seat-id \"$seat\" --amount-msats \"$amount\" ;;\n\
           *) usage ;;\n\
         esac\n",
        shell_escape(defe_env.as_os_str()),
        shell_escape(args.root.join("synthetic-remit.lock").as_os_str()),
        shell_escape(defe_env.as_os_str()),
        shell_escape(args.root.as_os_str()),
        shell_escape(locked_fi_cli.as_os_str()),
        shell_escape(bin_dir.join("bitcoin-cli").as_os_str()),
        shell_escape(invite.as_os_str()),
    ));
    write_wrapper(&bin_dir.join("fees"), &fees)?;
    write_wrapper(
        &bin_dir.join("traffic"),
        &format!(
            "#!/bin/sh\nexec {} --internal-with-lock --lock {} -- {} --internal-traffic --load-test-tool {} --invite-file {} --routes-file {} \"$@\"\n",
            shell_escape(defe_env.as_os_str()),
            shell_escape(args.root.join("traffic.lock").as_os_str()),
            shell_escape(defe_env.as_os_str()),
            shell_escape(args.load_test_tool.as_os_str()),
            shell_escape(invite.as_os_str()),
            shell_escape(args.root.join("iroh-connect-overrides").as_os_str()),
        ),
    )?;
    Ok(())
}

fn write_wrapper(path: &Path, contents: &str) -> Result<()> {
    write_private(path, contents.as_bytes())?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_child(
    requested: &[OsString],
    args: &Args,
    manifest: &Path,
    secrets: &Path,
    invite: &Path,
    fi_state_dir: &Path,
    routes_file: &Path,
    bin_dir: &Path,
    relay_url: &str,
    gateway_url: &str,
    flip_url: &str,
    flip_endpoint_id: &str,
    supervisor: &DescendantSupervisor,
) -> Result<ExitStatus> {
    let command = if requested.is_empty() {
        vec![std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"))]
    } else {
        requested.to_vec()
    };
    let mut gate = [0_i32; 2];
    if unsafe { libc::pipe(gate.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).context("create child launch gate");
    }
    #[cfg(not(test))]
    let mut child = {
        let mut child = Command::new(std::env::current_exe()?);
        child
            .arg("--internal-child-gate")
            .arg(gate[0].to_string())
            .arg(gate[1].to_string())
            .arg("--")
            .args(&command);
        child
    };
    #[cfg(test)]
    let mut child = {
        let mut child = Command::new("sh");
        let read_fd = gate[0].to_string();
        let write_fd = gate[1].to_string();
        child
            .args([
                "-c",
                "eval \"exec $2>&-\"; IFS= read -r _ <&$1; eval \"exec $1>&-\"; shift 2; exec \"$@\"",
                "sh",
                &read_fd,
                &write_fd,
            ])
            .args(&command);
        child
    };
    child
        .env("DEFE_ENV", "1")
        .env("DEFE_ENV_SCHEMA_VERSION", "1")
        .env("DEFE_ENV_ROOT", &args.root)
        .env("DEFE_ENV_MANIFEST", manifest)
        .env("DEFE_ENV_SECRETS", secrets)
        .env("DEFE_ENV_LOG_DIR", &args.logs_dir)
        .env("DEFE_ENV_BIN_DIR", bin_dir)
        .env("DEFE_ENV_INVITE_FILE", invite)
        .env("DEFE_ENV_FI_STATE_DIR", fi_state_dir)
        .env("DEFE_ENV_IROH_CONNECT_OVERRIDES_FILE", routes_file)
        .env("DEFE_ENV_NOSTR_RELAY_URL", relay_url)
        .env("DEFE_ENV_GATEWAY_API_URL", gateway_url)
        .env("DEFE_ENV_FLIP_ADMIN_URL", flip_url)
        .env("DEFE_ENV_FLIP_PUBLIC_ENDPOINT_ID", flip_endpoint_id);
    let mut paths = vec![bin_dir.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    child.env("PATH", std::env::join_paths(paths)?);

    // Inspect and transfer only this process's controlling terminal.
    let has_tty = unsafe { libc::isatty(libc::STDIN_FILENO) } == 1;
    let original_foreground = has_tty
        .then(|| unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) })
        .filter(|group| *group > 0);
    let mut child = match supervisor
        .wrap(&child, true)
        .and_then(|command| supervisor.spawn(command))
    {
        Ok(child) => child,
        Err(error) => {
            unsafe {
                libc::close(gate[0]);
                libc::close(gate[1]);
            }
            return Err(error).with_context(|| {
                format!("start environment command {}", command[0].to_string_lossy())
            });
        }
    };
    unsafe { libc::close(gate[0]) };
    let child_pid = child.command_pid;
    // The composer becomes a background process after this transfer. Ignore
    // SIGTTOU until it has reclaimed the terminal; the child was already spawned
    // with the default disposition.
    let previous_sigttou = has_tty.then(|| unsafe { libc::signal(libc::SIGTTOU, libc::SIG_IGN) });
    if has_tty && unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, child_pid) } != 0 {
        let _ = supervisor.terminate_and_reap();
        if let Some(handler) = previous_sigttou {
            unsafe { libc::signal(libc::SIGTTOU, handler) };
        }
        unsafe { libc::close(gate[1]) };
        bail!(
            "give terminal foreground to environment command: {}",
            std::io::Error::last_os_error()
        );
    }
    let gate_released = unsafe { libc::write(gate[1], b"\n".as_ptr().cast(), 1) } == 1;
    unsafe { libc::close(gate[1]) };
    let status: Result<ExitStatus> = if gate_released {
        child.child.wait().await.map_err(Into::into)
    } else {
        Err(anyhow::anyhow!("release environment child launch gate"))
    };
    let drain_result = supervisor.terminate_and_reap();
    let restore_result = restore_terminal(original_foreground);
    if let Some(handler) = previous_sigttou {
        unsafe { libc::signal(libc::SIGTTOU, handler) };
    }
    restore_result?;
    drain_result?;
    status
}

fn restore_terminal(original_foreground: Option<i32>) -> Result<()> {
    let Some(group) = original_foreground else {
        return Ok(());
    };
    loop {
        if unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, group) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).context("restore terminal foreground process group");
        }
    }
}

async fn run_command(
    supervisor: &DescendantSupervisor,
    command: &mut Command,
    name: &str,
    timeout: Duration,
) -> Result<String> {
    let mut command = supervisor
        .wrap(command, false)
        .with_context(|| format!("prepare {name}"))?;
    command
        .command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = supervisor
        .spawn(command)
        .with_context(|| format!("start {name}"))?;
    let mut stdout = child
        .child
        .stdout
        .take()
        .context("capture command stdout")?;
    let mut stderr = child
        .child
        .stderr
        .take()
        .context("capture command stderr")?;
    let collect = async move {
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let (_, _, status) = tokio::try_join!(
            stdout.read_to_end(&mut stdout_bytes),
            stderr.read_to_end(&mut stderr_bytes),
            child.child.wait(),
        )?;
        Ok::<_, std::io::Error>((status, stdout_bytes, stderr_bytes))
    };
    let (status, stdout, stderr) = match await_or_cleanup(timeout, collect, || {
        supervisor
            .terminate_and_reap()
            .with_context(|| format!("tear down environment after timed-out {name}"))
    })
    .await?
    {
        Some(output) => output,
        None => {
            return Err(SetupCommandTimeout.into());
        }
    };
    ensure!(
        status.success(),
        "{name} failed:\n{}",
        String::from_utf8_lossy(&stderr)
    );
    String::from_utf8(stdout).context("command output is not UTF-8")
}

/// Drops an owned timed-out operation before cleanup can reap resources it owns.
async fn await_or_cleanup<F, T, C>(timeout: Duration, operation: F, cleanup: C) -> Result<Option<T>>
where
    F: Future<Output = std::io::Result<T>>,
    C: FnOnce() -> Result<()>,
{
    let mut operation = Box::pin(operation);
    let outcome = tokio::time::timeout(timeout, operation.as_mut()).await;
    // In run_command this retires the kill-on-drop Tokio proxy while its
    // registered helper PID still denotes that process. Cleanup may then reap
    // the helper without leaving a handle that could later signal a reused PID.
    drop(operation);
    match outcome {
        Ok(output) => Ok(Some(output?)),
        Err(_) => {
            cleanup()?;
            Ok(None)
        }
    }
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
    eprintln!("defe env: {message}...");
}

#[allow(clippy::too_many_arguments)]
fn print_ready(
    manifest: &Path,
    secrets: &Path,
    logs: &Path,
    fman_cli: &Path,
    fmans: &[FmanInfo],
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
    fmans: &[FmanInfo],
    gateway: &GatewaydInfo,
    flip_url: &str,
    public_endpoint_id: &str,
) -> String {
    let mut output = String::new();
    writeln!(output, "defe env is ready").expect("write to string");
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
    for (index, fman) in fmans.iter().enumerate() {
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
    writeln!(
        output,
        "Exit the environment shell or command to tear the environment down."
    )
    .expect("write to string");
    output
}

fn fman_api_url(base_url: &str, endpoint: &str) -> String {
    format!("{}/api/{endpoint}", base_url.trim_end_matches('/'))
}

fn shell_escape(value: &std::ffi::OsStr) -> String {
    let value = value
        .to_str()
        .expect("Defe validates every generated-wrapper value as UTF-8");
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

fn invalidate_ready_manifest(path: &Path) {
    let Ok(contents) = fs::read(path) else {
        return;
    };
    let Ok(mut manifest) = serde_json::from_slice::<serde_json::Value>(&contents) else {
        let _ = fs::remove_file(path);
        return;
    };
    manifest["ready"] = false.into();
    manifest["state"] = "stopped".into();
    manifest["gateway"]["state"] = "stopped".into();
    manifest["flip"]["state"] = "stopped".into();
    if write_json_atomic(path, &manifest).is_err() {
        let _ = fs::remove_file(path);
    }
}

fn parse_args(args: Vec<std::ffi::OsString>) -> Result<Args> {
    let mut root = None;
    let mut logs_dir = None;
    let mut fi_cli = None;
    let mut gateway_cli = None;
    let mut bitcoin_cli = None;
    let mut load_test_tool = None;
    let mut fman_cli = None;
    let mut complete_liquidity = false;
    let mut command = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--root") => root = args.next().map(PathBuf::from),
            Some("--logs-dir") => logs_dir = args.next().map(PathBuf::from),
            Some("--fi-cli") => fi_cli = args.next().map(PathBuf::from),
            Some("--fman-cli") => fman_cli = args.next().map(PathBuf::from),
            Some("--gateway-cli") => gateway_cli = args.next().map(PathBuf::from),
            Some("--bitcoin-cli") => bitcoin_cli = args.next().map(PathBuf::from),
            Some("--load-test-tool") => load_test_tool = args.next().map(PathBuf::from),
            Some("--complete-liquidity") => complete_liquidity = true,
            Some("--") => {
                command.extend(args);
                break;
            }
            Some("--help" | "-h") => {
                println!("Usage: defe env [--complete-liquidity] [-- COMMAND...]");
                std::process::exit(0);
            }
            _ => bail!("unrecognized defe env argument: {}", arg.to_string_lossy()),
        }
    }
    Ok(Args {
        root: root.context("internal --root argument is missing")?,
        logs_dir: logs_dir.context("internal --logs-dir argument is missing")?,
        fi_cli: fi_cli.context("internal --fi-cli argument is missing")?,
        fman_cli: fman_cli.context("internal --fman-cli argument is missing")?,
        gateway_cli: gateway_cli.context("internal --gateway-cli argument is missing")?,
        bitcoin_cli: bitcoin_cli.context("internal --bitcoin-cli argument is missing")?,
        load_test_tool: load_test_tool.context("internal --load-test-tool argument is missing")?,
        complete_liquidity,
        command,
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::process::ExitStatusExt as _;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::{
        Args, BitcoinManifest, FederationManifest, FlipManifest, GatewayManifest, Manifest,
        await_or_cleanup, exit_code, fman_api_url, ready_output, shell_escape, stopped_manifest,
        write_tools,
    };
    use defe_client::{BitcoindInfo, GatewaydInfo};

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn timed_out_operation_drops_its_owner_before_cleanup() {
        let dropped = Arc::new(AtomicBool::new(false));
        let operation = {
            let drop_flag = DropFlag(Arc::clone(&dropped));
            async move {
                let _drop_flag = drop_flag;
                std::future::pending::<std::io::Result<()>>().await
            }
        };
        let observed = Arc::clone(&dropped);
        let output = await_or_cleanup(Duration::ZERO, operation, move || {
            assert!(
                observed.load(Ordering::SeqCst),
                "cleanup ran before the timed-out operation released its process handle"
            );
            Ok(())
        })
        .await
        .expect("time out operation and run cleanup");
        assert!(output.is_none());
    }

    #[test]
    fn shell_escape_handles_spaces_and_single_quotes() {
        assert_eq!(
            shell_escape(std::ffi::OsStr::new("/tmp/a b'c")),
            "'/tmp/a b'\\''c'"
        );
    }

    #[test]
    fn signal_status_uses_the_conventional_shell_exit_code() {
        assert_eq!(
            exit_code(std::process::ExitStatus::from_raw(libc::SIGTERM)),
            143
        );
    }

    #[test]
    fn stopped_manifest_invalidates_every_live_state() {
        let manifest = Manifest {
            schema_version: 1,
            ready: true,
            state: "ready",
            federation: FederationManifest {
                invite_file: Path::new("/invite"),
                fi_state_dir: Path::new("/fi"),
                fi_account_file: Path::new("/fi/account"),
            },
            fmans: vec![],
            bitcoin: BitcoinManifest {
                rpc_url: "http://bitcoin",
                data_dir: Path::new("/bitcoin"),
            },
            nostr_relay_url: "ws://relay",
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
        let output = ready_output(
            Path::new("/tmp/env/env.json"),
            Path::new("/tmp/env/secrets.json"),
            Path::new("/tmp/env/logs"),
            Path::new("/tmp/fman-cli"),
            &[fman],
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

    #[test]
    fn generated_wrappers_forward_exact_arguments_and_preserve_fee_failures() {
        let root = std::env::temp_dir().join(format!("defe-wrapper-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let recorder = root.join("record command");
        std::fs::write(
            &recorder,
            "#!/bin/sh\nprintf 'args' >>\"$RECORD\"\nprevious=\nfailed=0\nfor arg in \"$@\"; do printf '[%s]' \"$arg\" >>\"$RECORD\"; [ \"$previous $arg\" = 'guardian-fees collect' ] && failed=1; previous=$arg; done\nprintf ' env=[%s][%s]\\n' \"${FMAN_E2E_LOCAL_IROH-}\" \"${FM_IROH_CONNECT_OVERRIDES-}\" >>\"$RECORD\"\nif [ \"$failed\" -eq 1 ]; then exit 23; fi\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&recorder, std::fs::Permissions::from_mode(0o700)).unwrap();
        let locker = root.join("locker");
        std::fs::write(
            &locker,
            "#!/bin/sh\nwhile [ \"$1\" != -- ]; do shift; done\nshift\nexec \"$@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&locker, std::fs::Permissions::from_mode(0o700)).unwrap();
        let record = root.join("record");
        let args = Args {
            root: root.clone(),
            logs_dir: root.join("logs"),
            fi_cli: recorder.clone(),
            fman_cli: recorder.clone(),
            gateway_cli: recorder.clone(),
            bitcoin_cli: recorder.clone(),
            load_test_tool: recorder.clone(),
            complete_liquidity: false,
            command: vec![],
        };
        let fman = defe_client::FmanInfo {
            locator: "locator".into(),
            data_dir: root.join("fman data"),
            iroh_connect_overrides: "routes".into(),
            admin_url: "http://fman".into(),
            admin_password: "fman-secret".into(),
        };
        let fmans = vec![fman];
        let gateway = GatewaydInfo {
            api_url: "http://gateway".into(),
            password: "gateway-secret".into(),
        };
        let bitcoin = BitcoindInfo {
            rpc_url: "http://bitcoin".into(),
            rpc_host: "127.0.0.1".into(),
            rpc_port: 18443,
            p2p_port: 18444,
            rpc_username: "bitcoin-user".into(),
            rpc_password: "bitcoin-password".into(),
            data_dir: root.join("bitcoin data"),
        };
        let bin = root.join("bin");
        write_tools(
            &bin,
            &args,
            &locker,
            &root.join("env.json"),
            &root.join("secrets.json"),
            &root.join("invite"),
            &root.join("fi state"),
            "route one,route two",
            &fmans,
            &["seat-one".into()],
            &gateway,
            &bitcoin,
        )
        .unwrap();

        let run = |tool: &str, arguments: &[&str]| {
            std::process::Command::new(bin.join(tool))
                .args(arguments)
                .env("RECORD", &record)
                .status()
                .unwrap()
        };
        assert!(run("fman-1", &["alpha", "two words"]).success());
        assert!(run("fi-cli", &["status", "two words"]).success());
        assert!(run("gateway", &["info", "two words"]).success());
        assert!(run("bitcoin-cli", &["getblockchaininfo", "two words"]).success());
        let collect = run("fees", &["collect", "--guardian", "1"]);
        assert_eq!(collect.code(), Some(23));
        let before_mixed = std::fs::read(&record).unwrap();
        assert_eq!(
            run("fees", &["collect", "--guardian", "1", "--all"]).code(),
            Some(2)
        );
        assert_eq!(std::fs::read(&record).unwrap(), before_mixed);

        let recorded = std::fs::read_to_string(&record).unwrap();
        assert!(recorded.contains("args[--data-dir]["));
        assert!(recorded.contains("][alpha][two words]"));
        assert!(recorded.contains("args[--state-dir]["));
        assert!(recorded.contains("][status][two words] env=[1][route one,route two]"));
        assert!(recorded.contains(
            "args[--address][http://gateway][--rpcpassword=gateway-secret][info][two words]"
        ));
        assert!(recorded.contains("args[-regtest][-datadir=",));
        assert!(recorded.contains(
            "][-rpcuser=bitcoin-user][-rpcpassword=bitcoin-password][-rpcport=18443][getblockchaininfo][two words]"
        ));
        assert!(recorded.contains("args[--data-dir]["));
        assert!(recorded.contains("][guardian-fees][collect][seat-one]"));
        assert!(recorded.contains("][guardian-fees][show][seat-one]"));
        assert!(
            std::fs::read_to_string(bin.join("fi-cli-locked"))
                .unwrap()
                .contains("--internal-with-lock --lock")
        );
        let traffic = std::fs::read_to_string(bin.join("traffic")).unwrap();
        assert!(traffic.contains("--internal-with-lock --lock"));
        assert!(traffic.contains("traffic.lock"));
        assert!(traffic.contains("--internal-traffic"));
        assert!(traffic.contains("--load-test-tool"));
        assert!(traffic.contains("--invite-file"));
        assert!(traffic.contains("--routes-file"));
        let fees = std::fs::read_to_string(bin.join("fees")).unwrap();
        assert!(fees.contains("synthetic-remit --guardian N --amount-msats AMOUNT"));
        assert!(fees.contains("--internal-synthetic-remit"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
