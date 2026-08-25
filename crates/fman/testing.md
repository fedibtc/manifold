# Fleet Manager testing

## Seat processes

FMan core tests run shell-script `fedimintd` doubles through the real
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
- FMan core integration tests combine `FakeFedimintd` with a fake callback
  invoker to cover retry policy, operator-blocked recovery, terminal rejection,
  Restart-versus-formation, decommission, concurrent isolation, and harmless
  late results. Binary-adapter tests separately pin the
  exact HTTP request shape, status classification, and bounded response handling.
- FI tests pin schema migration, private durable state, exact all-guardian
  fanout, callback-free compatibility, and clearing at `Formed`.

Use shutdown/reopen tests whenever a property depends on durable ownership:
retry resumption, terminal bearer clearing, callback retention, FI
pre-`Formed` retention, or FI terminal clearing. In-memory assertions alone do
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

- `service-fleet-manager` pins exact list, current-fetch, and
  incarnation-changed wire shapes plus UUIDv7 validation and diagnostic
  redaction.
- `bounded-rolling-file` owns crash/reopen storage tests: atomic incarnation
  publication, legacy initialization, malformed state, durable non-reused
  segment reservation, retention, restart, directory replacement, and
  descriptor-relative link rejection.
- `fman-core` owns the FMan/current/retained-seat selector-to-directory mapping;
  restore tests establish that data-only crash adoption remains valid while a
  pre-existing sibling safe-event journal makes onboarding restore fail closed.
- `fman-telemetry` owns request/cursor incarnation comparison, current reads,
  cross-journal and mixed-cursor discontinuities, recreation, and sanitized
  service errors.
