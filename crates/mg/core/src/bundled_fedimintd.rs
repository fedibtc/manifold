//! The `fedimintd` bundled into the MG binary: seats remain separate OS
//! processes, but the daemon spawns *itself* rather than an externally
//! supplied binary (ARCH-manifold-guardian-seat-processes *Subprocess model*).
//!
//! Core owns only the spawn contract — the argv[0] the seat process spawns
//! under. Recognising that argv[0] and running fedimintd belong to the
//! composition root, which is the crate that actually bundles it; keeping
//! them here would put the whole `fedimintd` dependency in core for one
//! string.

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

/// How long a consensus session may run before fedimintd exits on its own,
/// so that a respawn can unstick it. The seat process sets it on every spawn
/// (`FM_SESSION_TIMEOUT_SECS`, upstream's default), and the seat's stall limit
/// is derived from it.
pub const SESSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60 * 60);
