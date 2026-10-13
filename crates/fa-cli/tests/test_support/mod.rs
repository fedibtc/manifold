use std::ffi::OsString;

/// Resolve at execution time so cached Nextest artifacts remain relocatable.
/// Ordinary `cargo test` runs retain Cargo's compile-time binary path fallback.
pub(crate) fn fa_cli_bin() -> OsString {
    std::env::var_os("FA_CLI_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_fa-cli").into())
}
