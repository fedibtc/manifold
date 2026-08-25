# CLAIM-fleet-manager-seat-authority-processes-do-not-overlap: Old and replacement seat processes do not overlap authority

On Linux, under A-host, A-kernel, and A-tokio-process, arbitrary Fleet Manager exits and
restarts on one data root cannot cause an old and a replacement `fedimintd`
for the same seat to perform guardian-authority network actions concurrently.
The quantified exits include orderly shutdown, panic/unwind, SIGKILL, and a
daemon death at any instruction during child creation. They also include the
dedicated child-spawner thread's exit while its daemon remains alive and
replacement inside one daemon's supervisor loop.

An **old child** is a `fedimintd` directly spawned for the seat before the
relevant parent/thread exit or respawn decision; a **replacement** is the next
direct child for that same data root and seat number. A guardian-authority
network action is accepting a setup or consensus API request, sending or
accepting guardian P2P traffic, or contributing to consensus. Merely being an
unreaped process, opening its database, or briefly overlapping a replacement
which fails before opening an authority endpoint is not the bad thing.

The claim does not require the old PID to be reaped before the successor daemon
acquires the flock. That ordering does not exist. The implementation attempts
to close the admitted process-liveness overlap with an early metrics bind and
a child-held database lock. The argument below
exhibits a reachable trace which bypasses both.

## Status

Falsified: during codeless DKG, startup can unlink a live child’s locked inode and recreate its database on a changed port grid while the old setup API still answers.

## Assumptions

- **A-host:** the host is single-tenant and its operator is honest. No local
  process launches a copied seat, moves or shares its data root, modifies its
  daemon-produced database/archive files, reaps the daemon's children,
  interferes with signals, binds the configured seat ports, or supplies a
  different binary. Availability, resource exhaustion, and cost are excluded.
- **A-kernel:** Linux implements `PR_SET_PDEATHSIG`, `getppid`, `fork`/`exec`,
  `SIGKILL`, `wait`, close-on-exec file descriptors, process teardown, and
  exclusive TCP bind and advisory file locking according to their documented
  semantics. In particular,
  the parent-death signal is tied to the thread which created the child; a
  successful `SIGKILL` cannot be caught or ignored; `wait` completes only after
  process exit; process exit closes all of its file descriptors; and two live
  sockets cannot exclusively bind the same address and port. An exclusive file
  lock blocks another process until the last holder releases it. Ordinary kernel
  scheduling delay is allowed.
- **A-tokio-process:** the pinned Tokio 1.52.3 process wrapper has its
  documented/current source behavior: `Command::spawn` calls synchronous
  `std::process::Command::spawn` on the polling thread and then fallibly wraps
  the returned `StdChild`; `Child::kill().await` signals and waits; and
  `kill_on_drop` applies only after wrapping succeeds. A plain `StdChild` drop
  does not kill. `Cargo.lock` pins the version whose behavior is trusted here.
