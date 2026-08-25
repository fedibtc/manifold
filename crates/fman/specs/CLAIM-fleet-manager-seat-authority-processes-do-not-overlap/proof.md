# Proof: Old and replacement seat processes do not overlap authority



Scope: `Cargo.lock`,
`crates/fman/core/src/{fedimint_api,fleet,identity,seat,supervisor}.rs`,
`crates/fman/bin/src/main.rs`, `crates/fman/specs/{ARCH-fleet-manager,ARCH-fleet-manager-seat-processes}.md`,
`.nix-deps/fedimint/**`

## Claim

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

## Axioms (trusted, not checked here)



## Argument

**L1 (enum) — all production child starts use one spawn contract.** The
production call chain is `Seat::start` -> `SeatSupervisor::spawn` ->
`supervise` -> `SeatProcess::start` -> `spawn_child`. Startup of stored seats
and insertion of a new seat call `Seat::start`; `RestartDKG` replaces the
supervisor; and `supervise` performs crash respawns. The bundled argv dispatch
enters `fedimintd::run` but is invoked as a child only by `spawn_child` under
A-host. No source reachable from the bundled fedimintd and its default modules
creates an OS process, so it has no descendant outside this direct lifetime coupling.

**L2 (code + axiom) — every successful Linux child closes the
fork-before-prctl escape.** Immediately before `exec`, in the child, `pre_exec`
installs `PR_SET_PDEATHSIG=SIGKILL` and then requires `getppid()` to equal the
daemon TGID captured before `fork`. On full daemon death before `prctl`, no
thread-group member remains to adopt the child, so the check observes
reparenting outside that TGID and exits rather than executing fedimintd. Full
daemon death after `prctl` generates the installed signal; death between the
check and `exec` is included. Failure to install the signal fails spawn.

Linux's creator-thread rule has a subtler thread-only race: if the creating
thread exits before `prctl` while another daemon thread lives, Linux may adopt
the child into that live member, leaving `getppid()` equal to the daemon TGID.
The subsequent `prctl` then couples it to that current parent; it is not an
uncoupled escape. If the creator exits after `prctl`, PDEATHSIG kills the child.
Thus no placement around `fork`, `prctl`, the race check, or `exec` produces an
executing child which can survive the entire daemon.

**L3 (code + axiom) — one process-lifetime thread creates every child, and its
exit cannot cause overlap.** `supervise` sends each prepared command to the
static `ChildSpawner`. Its dedicated OS thread synchronously calls
`tokio::process::Command::spawn` while entered into the requesting runtime, and
returns the wrapped child to the seat task. Retaining the static sender keeps
that thread blocked on its request channel for the daemon's process lifetime;
ordinary Tokio worker migration and retirement cannot trigger a child's
parent-death signal. If the dedicated thread nevertheless exits, the
post-`prctl` case SIGKILLs its children, while the pre-`prctl` case is safely
adopted as in L2. In the killed case, `Child::wait` cannot complete until exit
and only then enters backoff. The static sender remains connected to the dead
thread, so all later spawn requests fail closed until daemon restart. The
failure is therefore fleet-wide unavailability, not overlap.

**L4 (code + axiom) — wrapped-child stop reaps, but a spawn-return gap can
leave an untracked live child.** Once the supervisor owns a `SeatProcess`, a
naturally exiting child is awaited before backoff. Shutdown and `RestartDKG`
use `Child::kill().await`, and `SeatSupervisor::stop` awaits the supervision
task before installing a new supervisor. Under A-host/A-kernel, kill/wait of a
still-live owned child succeeds, while ESRCH means there is no live old child.
Task abort after successful wrapping invokes `kill_on_drop(true)` without
waiting, so successor-daemon cases still require L5.

There is also a narrower same-daemon gap before `SeatProcess` exists. By
A-tokio-process, `self.std.spawn()` may successfully exec the PDEATHSIG child
and then the fallible Tokio stdio/pidfd/signal wrapping may return an error.
Dropping that plain `StdChild` does not kill it; `spawn_child` reports failure
and the supervisor retries after backoff while the untracked child remains.
The retry uses the same daemon configuration, so the old child's early metrics
listener prevents the retry from reaching authority; this is availability, not
the claimed overlap. It nevertheless means same-daemon replacement is not
universally reap-ordered and the early gate is load-bearing even here.

**L5 (code) — neither candidate cross-daemon gate is stable across every
restart.** The data-root flock belongs to fleet-owning daemon state, not to an
already-execed child. A SIGKILLed daemon can release it while the old child
still exists before PDEATHSIG delivery. Ordinary error unwind cannot release it
ahead of a started seat loop because that loop's `Db` clone retains the flock,
but there is no finite
scheduling-time bound on this PID-liveness overlap.

The metrics listener is bound before the database and authority endpoints, but
its port is not stable state: `first_port_base` is restart-time CLI input, and
`Fleet::open_with_wallet` recomputes every stored seat's four ports from that
current value. A successor using a different honest configuration bypasses the
old metrics listener. Ordinarily the same seat-directory database still gates
the successor: `RocksDb::open` takes a blocking exclusive
`data/database.db.lock` before `fedimint_server::run` builds authority
endpoints. But that lock file lives below the directory startup can wipe.

**L6 (code + counterexample) — a durable codeless DKG attempt bypasses both
gates.** This trace is inside the claim and satisfies A-host/A-kernel:

1. `RestartDKG` orderly stops and reaps the previous child, appends a fresh
   durable attempt with no `guardian_code`, removes the old data directory, and
   spawns a new child. Begin the ordinary FMan `GetDkgCode` flow while that child
   serves `AwaitingLocalParams`, leaving its `setup_status`/`set_local_params`
   request queued or its handler runnable before the response is durably
   recorded.
2. The daemon exits or unwinds after releasing its flock, while scheduling
   delays that honest request handler and the old child's kill/teardown. TCP
   bytes already sent by FMan remain readable after its socket closes. Start a
   successor on the same data root with a different `first_port_base`; its
   metrics bind does not conflict.
3. Startup loads the codeless attempt. `Seat::start` maps
   `guardian_code.is_none()` to `wipe_first`, and `supervise` executes
   `remove_dir_all(seat_data_dir)` before spawning the replacement. This unlinks
   the old child's open `database.db.lock` inode; Linux advisory locks attach to
   that inode, not to the now-vacant pathname. The old child retains its old
   lock and in-memory `SetupApi`, whose database is explicitly not needed by
   `setup_status`, so deletion does not stop its listener or action.
4. The replacement recreates the directory and a new lock inode, acquires it,
   opens fresh RocksDB, and serves setup API on the different port grid. An FI
   retry makes the successor process its setup flow while the delayed old child
   processes the honest request from step 1. Both deterministic Iroh
   keys and the derived `api_auth` also repeat, but credential equality is no
   mutual-exclusion mechanism.

The trace does not depend on a failed orderly `RestartDKG` stop: the vulnerable
codeless interval is deliberately durable from the append until
`ensure_setup_code` records the returned guardian code. PDEATHSIG eventually
kills the old child, but supplies no bound or barrier before steps 3–4. Thus the
metrics port is configurable and the DB lock is unlinkable at precisely the
startup state which requests a wipe.

**Conclusion.** L1–L4 establish direct child coupling and stronger ordering
after successful Tokio wrapping, while identifying an availability-only
untracked-child gap; they do not order successor-daemon spawn after
old-child death. L5–L6 give a reachable same-root, changed-grid, codeless-attempt
trace in which startup replaces the only remaining child-held lock and both old
and replacement setup authorities accept network actions concurrently. The
claim is false. ∎

## Residual windows (outside the claim; acceptance stated per window)



## Weakest links

The falsified joint is cross-daemon exclusion: the daemon flock can release
before child teardown, the port grid is not durable, and the child DB lock is
inside the replayed-wipe subtree. L2/L3's Linux thread-parent semantics and
L1's full reachable-process enumeration remain weaker supporting links but
cannot repair that counterexample. Successful wrapped-child stop/wait ordering (L4) is stronger and irrelevant
to the failing trace; its post-exec wrapping-error gap is contained only by the
same-grid metrics gate.

## Falsification procedure

Run cold from the claim and axioms, without relying on the argument above.

1. Enumerate every production child spawn and every process-creation API
   reachable from bundled `fedimintd::run(default_modules())`; look especially
   for daemonization or grandchildren which do not inherit PDEATHSIG.
2. Crash the daemon before and after each of fork, `prctl`, `getppid`, exec,
   child-handle construction, flock release, and successor spawn. Seek a trace
   in which an executing child lacks PDEATHSIG or the successor opens authority
   Iroh/P2P/API endpoints first.
3. Account for Linux's creator-*thread* rule. Establish that the dedicated
   process-lifetime spawner thread calls `Command::spawn`, then fail that thread
   while runtime workers and the supervisor remain live; check whether
   replacement can start before wait observes exit. Separately retire the Tokio
   worker which requested a child and confirm it cannot affect child lifetime.
4. Force natural exit, shutdown, `RestartDKG`, task cancellation, child wait or
   kill errors, panic/unwind, and the supervisor backoff loop. Also fail Tokio
   child wrapping after successful exec but before `kill_on_drop` attaches;
   confirm the same-grid metrics gate contains the untracked child.
5. During hard restart, and separately during partial-open/error unwind, delay
   old-child teardown while allowing the successor daemon to take the flock.
   Try both the same and a changed `first_port_base`; with a codeless attempt,
   check whether startup wipe replaces the different-grid child's locked inode
   before either setup authority has stopped.
6. Compare the old and new derivations of the two Iroh secrets and `api_auth`
   for the same root and seat, including pre-DKG, formed, and `RestartDKG`
   states. Try to make a stale child accept an authentication secret the
   successor does not derive.
