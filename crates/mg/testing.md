# Manifold Guardian testing

## Seat processes

MG core tests run shell-script `fedimintd` doubles through the real
`SeatProcess` boundary. Supported Unix targets cover start, stop, output
pumping, exit observation, and seat-loop respawn. Linux-only
tests additionally exercise signal reporting and recreate Tokio worker
replacement/retirement to prove that it cannot trigger a child's parent-death
signal. Actual daemon SIGKILL cleanup remains grounded in Linux
`PR_SET_PDEATHSIG` semantics rather than an end-to-end subprocess test.

## Completion callbacks

Test the callback state machine at four boundaries:

- Protocol unit tests pin strict decoding, signed-verb separation, field bounds,
  and redacted formatting.
- Database tests exercise first-write retention, eligibility checks, terminal
  bearer clearing, and decommission races.
- MG core integration tests combine `FakeFedimintd` with a fake callback
  invoker to cover retry policy, operator-blocked recovery, terminal rejection,
  Restart-versus-formation, decommission, concurrent isolation, and harmless
  late results. Binary-adapter tests separately pin the
  exact HTTP request shape, status classification, and bounded response handling.
- FA tests pin schema migration, private durable state, exact all-guardian
  fanout, callback-free compatibility, and clearing at `Formed`.

Use shutdown/reopen tests whenever a property depends on durable ownership:
retry resumption, terminal bearer clearing, callback retention, FA
pre-`Formed` retention, or FA terminal clearing. In-memory assertions alone do
not establish these properties.


## Guardian-fee collection

The collection phase runner is the fault-injection boundary. Colocated tests
replace its balance read, idle claim, and unlock operations while exercising the
same orchestration that the Fedimint adapter calls. They cover failures before
submission, after an operation ID, after earlier terminal success, during the
final refresh, and during the best-effort post-failure refresh. Separate stream
tests pin which stability-pool terminal states contribute confirmed value.

These tests do not reproduce the dependency's transaction commit. The security
argument separately records why a returned operation ID is the durable boundary;
recheck that dependency correspondence when the pinned Fedimint client changes.


## Durable payout jobs

The payout foundation currently has focused coverage at both persistence
boundaries:

- SQLite tests directly pin request identity, immutable scope/destination,
  set-once operation links, no-delete triggers, and reopen.
- Core tests inject a payment-wallet lost response between native commit and
  SQLite linking, rebuild `Fleet` around the reopened database, race concurrent
  same-ID calls, and prove unlinked status has no start authority. A guardian
  test performs the same lost-response/reopen recovery from its stored invite
  without any live seat, then successfully observes and awaits the linked
  operation without another start.
- Fedimint adapter tests pin v1 request/destination metadata, the common v1/v2
  matcher's binding conflict, and concurrent use of the production scope fence.
- A cancellation test pins the process-lifetime payment open fence in both
  join-then-lazy and lazy-then-join orderings.

When the payout implementation changes, extend this matrix with an on-disk
retained payment reopen. An in-memory
wallet reused across a `Fleet` restart does not establish that a native operation
remains enumerable after process restart.

## Safe-event journal telemetry

Coverage is split at the ownership boundaries:

- `service-manifold-guardian` pins exact list, current-fetch, and
  incarnation-changed wire shapes plus UUIDv7 validation and diagnostic
  redaction.
- `bounded-rolling-file` owns crash/reopen storage tests: atomic incarnation
  publication, legacy initialization, malformed state, durable non-reused
  segment reservation, retention, restart, directory replacement, and
  descriptor-relative link rejection.
- `mg-core` owns the MG/current/retained-seat selector-to-directory mapping;
  restore tests establish that data-only crash adoption remains valid while a
  pre-existing sibling safe-event journal makes onboarding restore fail closed.
- `mg-telemetry` owns request/cursor incarnation comparison, current reads,
  cross-journal and mixed-cursor discontinuities, recreation, and sanitized
  service errors.

## Guardian transfer

In-tree unit and integration tests stop at the `fedimintd` process boundary:

- `transfer_native` tests validate and install handed-over material built from
  an in-memory native `ServerConfig`; they run no guardian.
- Database tests (`db/transfers`) pin admission, that a waiting seat stays
  bound to its move and takes a grant for the same move as a replay,
  acceptance with its formed row, monotone retirement, and the migration.
- Source tests (`transfer_source.rs`) drive `TransferSeat` and `RemoveSeat`
  against `FakeSeatProcessSpawner` children. The session-boundary exit is a
  scripted exit code (`SESSION_BOUNDARY_EXIT_CODE`, or the legacy shutdown's
  code), not a real `fedimintd` reaching a session boundary.
- Destination tests (`transfer_import.rs`) run the transfer worker against a
  controlled in-process source and a gated validator: pull, retry until the
  source answers, park, same-move replay before and after import,
  cancellation, installation and the production gate.
- Binding, backup-format, publication and restore tests cover the metadata
  vote, the transfer document version, single-snapshot publication, and
  restore of imported and retired seats.

That a real `fedimintd` exits with the boundary code only after its last
session completed is the pinned Fedimint fork's contract, unit-tested there
at the request and consensus-engine level; no native end-to-end transfer
between two MGs runs in this tree. Treat the tests above as boundary
coverage, not as evidence of a native handover.

## Memory profiling

The daemon and every seat `fedimintd` use jemalloc with profiling compiled
in. MG passes `MALLOC_CONF` to its children, so one variable on the daemon
profiles every process. For example, against a local `defe env`:

```bash
MALLOC_CONF=prof:true,lg_prof_sample:16,lg_prof_interval:27,prof_prefix:/tmp/mgprof/p \
  target/release/defe --binary-path target/release env
```

Each process then writes `/tmp/mgprof/p.<pid>.*.heap` after every 128 MiB
allocated; add `prof_gdump:true` for a dump at each new peak. Read a dump with
`jeprof --text --show_bytes target/release/fleet-manager <dump>` (`jeprof`
ships with nixpkgs `jemalloc`). The symbols use the `_rjem_` prefix, which
`jeprof` does not recognize as allocator frames, so read `--collapsed` stacks
past the `_rjem_je_prof_backtrace` frames. To approximate a small operator host,
pin the environment with `taskset -c 0-3`; Tokio sizes its worker pool from
the visible CPUs.
