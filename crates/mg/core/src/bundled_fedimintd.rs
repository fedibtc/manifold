//! The `fedimintd` lines bundled with the MG release: seats remain separate OS
//! processes, but the daemon only ever spawns a binary from its own release
//! rather than an externally supplied one
//! (ARCH-manifold-guardian-seat-processes *Subprocess model*).
//!
//! The 0.12 line is linked into the MG binary, which spawns *itself*. The 0.13
//! line cannot share that link (one Fedimint source per Cargo workspace), so
//! it is the sibling `fedimintd-0-13` binary installed beside the MG binary.
//!
//! Core owns only the spawn contract — which file and argv[0] run each line.
//! Recognising the argv[0] and running fedimintd belong to the composition
//! root, which is the crate that actually bundles it; keeping them here would
//! put the whole `fedimintd` dependency in core for one string.

use std::path::PathBuf;

use crate::facts::FedimintdLine;

/// argv[0] under which this binary is the bundled `fedimintd`.
///
/// The seat process spawns the current executable under this name
/// ([`seat_process`](crate::seat_process)); the binary must consult it before
/// parsing its own CLI, because fedimintd owns the argv.
pub const ARGV0: &str = "fedimintd";

/// fedimintd's exit code after a scheduled shutdown took effect at a session
/// boundary (`fedimintd::SESSION_BOUNDARY_EXIT_CODE`, which the composition
/// root asserts equal).
pub const SESSION_BOUNDARY_EXIT_CODE: i32 = 3;

/// File name of the 0.13 binary, installed in the MG binary's directory.
pub const FEDIMINTD_0_13_FILE_NAME: &str = "fedimintd-0-13";

/// The program and argv[0] that run `line`, given the MG's own executable.
pub fn program(own_exe: PathBuf, line: FedimintdLine) -> (PathBuf, &'static str) {
    match line {
        FedimintdLine::V0_12 => (own_exe, ARGV0),
        FedimintdLine::V0_13 => (
            own_exe.with_file_name(FEDIMINTD_0_13_FILE_NAME),
            FEDIMINTD_0_13_FILE_NAME,
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{ARGV0, program};
    use crate::facts::FedimintdLine;

    #[test]
    fn each_line_runs_its_own_release_binary() {
        let own = PathBuf::from("/nix/store/abc-mg/bin/fleet-manager");
        assert_eq!(
            program(own.clone(), FedimintdLine::V0_12),
            (own.clone(), ARGV0),
            "0.12 seats keep running the MG binary itself"
        );
        assert_eq!(
            program(own, FedimintdLine::V0_13),
            (
                PathBuf::from("/nix/store/abc-mg/bin/fedimintd-0-13"),
                "fedimintd-0-13"
            ),
            "0.13 seats run the sibling binary, never the MG under a 0.13 name"
        );
    }
}
