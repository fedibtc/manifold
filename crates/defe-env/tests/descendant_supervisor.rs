//! Adversarial integration coverage for the Linux PID-namespace boundary.

#[cfg(target_os = "linux")]
use std::io::{Read as _, Write as _};
#[cfg(target_os = "linux")]
use std::os::fd::FromRawFd as _;
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt as _;
#[cfg(target_os = "linux")]
use std::os::unix::process::ExitStatusExt as _;
#[cfg(target_os = "linux")]
use std::process::Stdio;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
#[test]
fn teardown_contains_continuous_double_forks_and_retries_kernel_failures() {
    let root = tempfile::tempdir().expect("create test root");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_defe-env"))
        .arg("--internal-supervisor-test")
        .arg(root.path().join("ready"))
        .output()
        .expect("run supervisor adversary");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.matches("retaining resources and retrying").count() >= 2,
        "inspection and signaling failures did not both fail closed:\n{stderr}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn foreground_interrupt_returns_to_shell_and_preserves_status() {
    let mut master = 0;
    let mut slave = 0;
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    assert_ne!(
        unsafe { libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC) },
        -1
    );
    let mut master = unsafe { std::fs::File::from_raw_fd(master) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave) };
    let root = tempfile::tempdir().expect("create test root");
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_defe-env"));
    command
        .arg("--internal-pty-test")
        .arg(root.path())
        .stdin(Stdio::from(slave.try_clone().expect("clone PTY")))
        .stdout(Stdio::from(slave.try_clone().expect("clone PTY")))
        .stderr(Stdio::from(slave));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut helper = command.spawn().expect("start PTY helper");
    drop(command);
    let (output_tx, output_rx) = std::sync::mpsc::channel();
    let mut reader = master.try_clone().expect("clone PTY master");
    let reader = std::thread::spawn(move || {
        let mut chunk = [0_u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => return,
                Ok(read) => {
                    if output_tx.send(chunk[..read].to_vec()).is_err() {
                        return;
                    }
                }
                Err(error) if error.raw_os_error() == Some(libc::EIO) => return,
                Err(error) => panic!("read PTY output: {error}"),
            }
        }
    });
    let mut output = Vec::new();
    // The terminal echoes each command line, so every marker is computed by
    // the shell and appears only once the shell has run the command. An
    // interrupt sent before the shell owns the terminal would reach the
    // helper itself instead.
    master
        .write_all(b"echo DEFE_$((1+1))_SHELL_READY\n")
        .expect("probe the shell");
    wait_for_pty_output(&output_rx, &mut output, "DEFE_2_SHELL_READY");
    master
        .write_all(b"sh -c 'echo DEFE_$((2+2))_JOB_STARTED; exec sleep 30'\n")
        .expect("start foreground job");
    wait_for_pty_output(&output_rx, &mut output, "DEFE_4_JOB_STARTED");
    master.write_all(b"\x03").expect("interrupt foreground job");
    master
        .write_all(b"echo DEFE_SHELL_SURVIVED\nexit 7\n")
        .expect("exit shell");
    let status = helper.wait().expect("wait for PTY helper");
    reader.join().expect("PTY reader");
    output.extend(output_rx.try_iter().flatten());
    let output = String::from_utf8_lossy(&output);
    assert_eq!(status.code(), Some(7), "helper output:\n{output}");
    assert!(
        output.contains("DEFE_SHELL_SURVIVED"),
        "helper output:\n{output}"
    );
}

#[cfg(target_os = "linux")]
fn wait_for_pty_output(
    output_rx: &std::sync::mpsc::Receiver<Vec<u8>>,
    output: &mut Vec<u8>,
    marker: &str,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(output).contains(marker) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match output_rx.recv_timeout(remaining) {
            Ok(chunk) => output.extend(chunk),
            Err(error) => panic!(
                "waiting for {marker}: {error}; helper output:\n{}",
                String::from_utf8_lossy(output)
            ),
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn abrupt_composer_death_retains_connection_until_namespace_is_empty() {
    let root = tempfile::tempdir().expect("create test root");
    let socket = root.path().join("custody.sock");
    let marker = root.path().join("child.pid");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind custody socket");
    let mut composer = std::process::Command::new(env!("CARGO_BIN_EXE_defe-env"))
        .arg("--internal-abrupt-owner-test")
        .arg(&socket)
        .arg(&marker)
        .spawn()
        .expect("start abrupt-owner helper");
    let (mut custody, _) = listener.accept().expect("accept guarded connection");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "child PID was not reported"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let child_pid = std::fs::read_to_string(&marker)
        .expect("read child PID")
        .trim()
        .parse::<i32>()
        .expect("parse child PID");
    unsafe { libc::kill(i32::try_from(composer.id()).unwrap(), libc::SIGKILL) };
    composer.wait().expect("reap killed composer");
    custody
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set custody timeout");
    let mut byte = [0_u8; 1];
    assert_eq!(
        custody
            .read(&mut byte)
            .expect("wait for lifetime guard EOF"),
        0
    );
    assert_eq!(
        unsafe { libc::kill(child_pid, 0) },
        -1,
        "lease custody ended while a namespace command remained"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn broker_preserves_occupied_and_inherited_file_descriptors() {
    for closed_stdio in [0_u8, 0b001, 0b011, 0b111] {
        run_occupied_fd_case(closed_stdio);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn composer_preserves_native_signal_status() {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_defe-env"));
    command.arg("--internal-signal-status-test");
    unsafe {
        command.pre_exec(|| {
            let mut signals = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&raw mut signals);
            libc::sigaddset(&raw mut signals, libc::SIGTERM);
            let result =
                libc::pthread_sigmask(libc::SIG_BLOCK, &raw const signals, std::ptr::null_mut());
            if result != 0 {
                return Err(std::io::Error::from_raw_os_error(result));
            }
            Ok(())
        });
    }
    let status = command.status().expect("run composer signal test");
    assert_eq!(status.signal(), Some(libc::SIGTERM));
}

#[cfg(target_os = "linux")]
#[test]
fn timeout_reaps_gateway_command_before_retry() {
    let root = tempfile::tempdir().expect("create timeout test root");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_defe-env"))
        .arg("--internal-timeout-test")
        .arg(root.path().join("timed-out.pid"))
        .status()
        .expect("run timeout overlap test");
    assert!(status.success());
}

#[cfg(target_os = "linux")]
fn run_occupied_fd_case(closed_stdio: u8) {
    let mut pipe = [0_i32; 2];
    assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
    let sentinel = unsafe { libc::fcntl(pipe[1], libc::F_DUPFD, 200) };
    assert!(sentinel >= 200);
    unsafe { libc::close(pipe[1]) };
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_defe-env"));
    command
        .arg("--internal-fd-occupation-test")
        .arg(sentinel.to_string())
        .arg(closed_stdio.to_string());
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(sentinel, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            for fd in 0..=2 {
                if closed_stdio & (1 << fd) != 0 {
                    libc::close(fd);
                }
            }
            Ok(())
        });
    }
    let status = command.status().expect("run occupied-FD broker test");
    unsafe { libc::close(sentinel) };
    let mut output = [0_u8; 9];
    let read = unsafe { libc::read(pipe[0], output.as_mut_ptr().cast(), output.len()) };
    unsafe { libc::close(pipe[0]) };
    assert!(
        status.success(),
        "occupied-FD case failed with closed mask {closed_stdio:#05b}: {status:?}"
    );
    assert_eq!(read, 9);
    assert_eq!(&output, b"sentinel\n");
}
