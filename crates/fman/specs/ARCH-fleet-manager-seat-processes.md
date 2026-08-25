# ARCH-fleet-manager-seat-processes: Seat hosting

## Subprocess model

Each seat's `fedimintd` runs as a separate OS process spawned directly by
the daemon — never inside the daemon's own process, not containerized,
not delegated to an init system. Target hosts (Start9/Umbrel, bare VPS)
cannot be assumed to have a container runtime; a process boundary
isolates crashes; and destroying a seat's state is a directory delete.
There is no kernel-level sandboxing between seats. The bundled, pinned
`fedimintd` is consequently a trusted FMan TCB component; the process boundary
isolates lifetime and crashes but does not contain a malicious child. This is an
accepted defense-in-depth residual under the single-tenant host model
([ARCH-fleet-manager](./ARCH-fleet-manager.md) *Trust boundaries*), not a reason
to weaken the separate host, operator, storage, or network boundaries.

The program spawned is the daemon's own binary: fedimintd is linked into
it and entered through a `fedimintd` argv[0], so a deployment cannot
pair the FMan with a fedimintd build it was not tested against. Its pinned
implementation integrity remains a TCB premise.

The current release hosts exactly one bundled fedimintd version
([SPEC-fi-rpc](./SPEC-fi-rpc.md) release policy).

## Child environment

Children run under a scrubbed environment the daemon builds, never an
inherited shell ([ARCH-fleet-manager](./ARCH-fleet-manager.md) *Trust
boundaries*). The bundled `fedimintd` composition root is the module-set
availability point: it registers the upstream modules plus the fixed Manifold
SPv2 initializer. The formation leader makes the policy decision by sending
`mintv2`, `walletv2`, `lnv2`, `meta`, and SPv2 in
the leader-carried module set encoded in its setup code, so every newly formed federation
commits exactly those five modules. Mintv2 is also the generation advertised
and accepted by the key-locked seat-payment protocol
([SPEC-locked-payment](./SPEC-locked-payment.md)). The composition-root test
pins the SPv2 configuration, and the live seven-guardian formation gate checks every committed
`consensus.json` for the exact set rather than merely checking initializer
availability. (un
implementation-driven spec change surfaced by the paid E2E — awaiting
owner read-through.)

## Lifetime coupling

A child never outlives the daemon. Children are lifecycle-bound (kill-on-drop
plus Linux parent-death signal — even SIGKILL of the daemon takes them
down), supervision is a direct parent-child wait, and daemon startup spawns a
child for every non-decommissioned seat. A fedimintd running without its manager would
squat the fixed seat port grid and answer the next daemon instance's
clients with stale credentials; coupling makes daemon restart semantics
trivial (no orphan discovery, no PID-reuse hazards).

Linux scopes a parent-death signal to the particular thread that creates the
child. The daemon therefore creates every child from one dedicated OS thread
that lives for the process lifetime; async-runtime workers may retire without
being mistaken for daemon death. The seat task still owns the returned process
handle, direct wait, graceful stop, and respawn policy.

The seat loop owns the process handle, driven client, and acknowledged-ceremony
phase as one runtime enum, so an ending child cannot leave a separately stored
client or acknowledgement behind. Its one watch-state projection says only
whether the current child has acknowledged DKG; the set-once formed record has
a deliberately independent durable lifetime. This reverses
the recent purposeful-process decision: every non-decommissioned seat keeps a
child, including the rare minutes-long interval before `StartDkg` or after a
failed ceremony. A parked `NeedsParams` child holds no keys, ports, or network,
so conditional spawning saved little while branching every lifecycle path.
`StartDkg` delivers `RunDkg` to that idle child; every child failure gets the
same capped-backoff replacement policy. A failed consuming stop leaves the
process slot permanently unreplaceable until daemon exit.
As an accepted tradeoff, a verb awaiting a bounded Fedimint API timeout can
delay a formed seat's respawn until it completes.

Because a ceremony child binds its iroh endpoint only when `StartDkg` delivers
its keys, first peer contact still has a discovery-publication ramp. Resolution
now races an uncached HTTP GET to the official n0 pkarr relay against default n0
DNS discovery, shrinking that ramp to pkarr publish latency while retaining DNS
as a concurrent fallback. This expected convergence under the unlimited p2p
dial retries is not a hang; it is not a reason to add a ceremony timeout.

The accepted cost of lifetime coupling is that restarting the daemon
(upgrade, crash, config change) briefly
takes down every guardian seat on the host at once. Fedimint guardians
tolerate a member blinking (consensus needs a threshold, not constant
full attendance), and the restart window is seconds; the rejected
alternative bought its uptime with re-adoption machinery whose failure
modes (adopting a stale child, losing one, PID confusion) are worse
than the blip.

## Consequences accepted with coupling

- Formed-seat supervision has no give-up: respawn forever with capped backoff
  (no `Failed` terminal health).
- A daemon crash-restart cycle is also a full fleet restart.

## Ordinary seat logging: per-child pipes, pumped into the daemon's stream



- **Line integrity is structural.** The daemon's reader does the
  framing, so no cross-child interleaving is possible — regardless of
  write sizes, panic output, or writer discipline in the child.
- **The FMan is out of ordinary log management.** It emits one
  stream; rotation and retention belong to whatever runs it (journald,
  a container platform's log capture).
- **The pump is the future policy point**: teeing to files, level
  filtering, rate limits, or an exit-time tail in the seat process's
  crash report can all be added in the daemon without touching
  fedimintd or any protocol surface.
- Accepted: pump tasks are per-child moving parts, and if the daemon's
  own output stalls, pipes fill and a child eventually blocks on
  logging — the same failure mode as any stdout logger.

## Explicitly shareable event journals

The bundled binary attaches the same Manifold-owned structured tracing layer
in FMan and fedimintd process modes. Only an event with the event-local typed
field `safe_to_share = true` enters this channel; span fields are excluded.
`fi-client` is outside this channel and does not emit shareable events.
Ordinary child output continues through the pipes above and is never parsed to
infer shareability.

Each process exclusively owns a `bounded-rolling-file` size-segmented JSONL
journal: FMan under
`safe-events/fman`, and a child under `seats/<seat-no>/safe-events`, outside the
fedimintd `data/` directory. A journal retains at most two 2.5 MiB segments
(5 MiB total record data) and rejects an event over 64 KiB. Segments use
readable numeric names such as `events-42.jsonl`. After formatting and
validation, the event emitter performs a nonblocking send into a fixed
128-record queue. A dedicated OS thread owns the appender, all filesystem I/O,
and the nonblocking advisory writer lock for the layer's lifetime. A full or
disconnected queue drops the event. Rotation removes the oldest complete
segment before creating its replacement; a write failure disables the
non-critical sink while its worker continues draining the bounded queue and
retaining the lock. Startup removes segments over the current configured limit
before reading at most that limit plus one byte to truncate an incomplete crash
tail. The seat journal is retained across DKG restart and terminal
decommission with the other on-disk seat data. The standard
`tracing_subscriber` JSON formatter writes the event fields and its normal
timestamp, level, and target metadata with current span and span-list output
disabled.

Formation diagnostics use paired events at failure boundaries. The ordinary
daemon or child stream receives the complete error chain and other local-only
detail. Its `safe_to_share = true` companion never formats that error; it names
only a fixed operation, stage, and failure category plus audited opaque ids,
numeric attempt/module/peer positions, counts, and booleans. Success milestones
use the same safe vocabulary. Consequently an exported journal shows the last
completed FMan seat, child process, setup-API, peer-connectivity, checksum,
module-DKG, persistence, consensus, and invite step, while potentially secret
configuration, DKG messages and shares, endpoint codes, bearer values, paths,
names, and dependency/free-form errors remain only in the ordinary local log.

The capability-scoped telemetry service reads these files without joining the
writer path. One FMan-wide bearer lists the global journal and all per-seat
journals. Each journal advances independently with a `(segment, byte offset)`
cursor. Reads seek and scan only for newline boundaries; they do not parse or
reserialize the already validated JSON. The active partial tail remains
unconsumed, and a cursor outside retained canonical segments reports a
continuity gap before restarting from the oldest segment.
