//! Native signal propagation across the outer `defe` process boundary.

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt as _;

#[cfg(unix)]
#[test]
fn exec_preserves_native_signal_status() {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_defe"))
        .args(["exec", "sh", "-c", "kill -TERM $$"])
        .status()
        .expect("run defe signal propagation test");
    assert_eq!(status.signal(), Some(libc::SIGTERM));
}
