# Proof: Each seat remains confined to its local authority

## Stale proof

The numbered argument below describes the last-reviewed implementation and is
not asserted to describe current source. The `Seat` and `SeatLoop` surfaces have
expanded. Current filesystem and port identity derive from `SeatFacts.seat_no`
rather than the seat-ID path and immutable `port_base` described in R3 and R4.
Regenerate the construction, method, database, resource, and child-input
enumerations and repair those mechanisms before relying on this argument or
removing the claim's `Unverified` status.

## Scope and model

This proof supports
[CLAIM-fleet-manager-confines-seat-local-authority](../CLAIM-fleet-manager-confines-seat-local-authority.md).
It covers production `Seat` construction and every operation causally triggered
through a retained seat, including its durable writes, filesystem paths, local
API, supervisor, child process, derived keys, crash/restart behavior, and
concurrency with independently initiated operations.

The protected boundary is local control-plane authority for another seat in the
same `Fleet`. Ordinary public Fedimint traffic and independently initiated
trusted local operations are outside that causal boundary; neither may retarget
the retained resources of the seat under analysis.

## Assumption boundary

The proof grants the claim record's four assumptions. They bottom out durable
store integrity, the unsandboxed bundled child's behavior under hostile input,
key and credential separation, and Rust/operating-system path, port, executable,
and process semantics. The argument does not establish those premises.

## Argument

**R1 (type + enum + code) — one seat object retains one immutable fact set.**
`Seat::start` constructs an `Arc<Seat>` handle whose fields are exactly the
immutable `facts`, one lifecycle command sender, one status `Notify`, and one
watch sender for the runtime mirror. For an active seat it spawns a `SeatLoop`
retaining those same facts plus fleet-wide `db`, common `process`/`policy`,
separately derived `keys`, S's supervisor, and a clone of that watch sender. A
terminal startup seat has no loop or supervisor. `SeatState` contains exactly
`attempt`, `consensus_ever_observed`, `decommissioned_at_ms`, `child_alive`,
`probe_generation`, and `last_probe`. Neither object can replace the facts and
neither holds the fleet registry.

Its crate-visible method surface is `facts`, `is_decommissioned`,
`reject_decommissioned`, `stop`, `dkg_code`, `start_dkg`, `restart_dkg`,
`report`, `summary`, `decommission`, and `invite_code`. The handle's private
`request`/`verb_request` helpers and status path only borrow or subscribe to
the watch value, send an owned command, or notify the retained probe.
`SeatLoop`'s command/probe dispatcher and private
lifecycle/client/supervisor helpers operate only on the same retained S.

`Fleet::authorize` is the RPC route to that object: it returns the resolved
`Arc<Seat>` itself after the ownership comparison and cannot retarget what it
resolved. Fresh creation and startup
reconstruction likewise pass one complete `SeatFacts` to `Seat::start` before
any supervisor is spawned; both production call sites query attempt state and
derive keys from that same facts id. `Seat::start`'s signature does not
type-bind its separately supplied facts, keys, and attempt, so their alignment
is this complete call-site enumeration. No listed method accepts another
`Seat`, fleet registry, or replacement `SeatFacts`. Other production callers
are limited to
authorization reading `facts().fi_id`; startup reading the retained facts and
decommission state to populate `accepted_quotes` and the registry;
admission/availability reading `is_decommissioned` across the accepted-quote
index and the fleet-wide lifetime port cursor under the allocation lock;
accepted `CreateSeat` replay reading
`facts().commitment`; RPC handlers operating on their authorize-selected
seat; operator report/summary/decommission; listing summary; and fleet
shutdown independently calling `stop` on each registry entry. Each reader
observes the same retained S, and no `Seat` method initiates traversal to the
next registry entry.

**R2 (schema + enum + code) — durable effects use S's immutable id.** The DB
methods reachable from `SeatLoop` commands and notified probes are
`append_dkg_attempt`, `record_guardian_code`, `record_dkg_codes`,
`record_dkg_started`, `record_consensus_observed`, and `decommission_seat`.
Every call supplies
`self.facts.seat_id`; no reachable method accepts a second seat id or holds the
fleet registry. Schema immutability keeps that stored id stable. Consequently
DKG history, observation, and decommission effects cannot resolve a sibling
row.

**R3 (type + enum + code + axiom) — filesystem effects stay in S's namespace.**
`SeatId` has private storage and accepts only non-empty ASCII alphanumeric
text, making it one safe path segment. `seat_data_dir` joins the fixed data
root, `seats`, S's id, and `data`. Both wipe sites and every child data-dir
argument derive from that function; no capability request field supplies a raw
local path. A4 makes the resulting namespace boundary effective.

**R4 (schema + enum + code + axiom) — local API/process resources are
seat-specific.** S's immutable `port_base` selects its checked four-port block.
Equal bases are schema-rejected; monotonic aligned `+4` allocation prevents
overlap and never reuses historical blocks. `FedimintApi` is constructed
from S's local API port and `facts.api_auth`; the supervisor receives S's id,
ports, data path, and derived keys. API credentials are generated independently
and process identities derive with S's id (A3). No `Seat` method has another
seat's credential, supervisor handle, port block, or key material to substitute
(A4).

The complete continuing process chain is `SeatSupervisor::spawn`, detached
`supervise`, optional wipe-before-spawn, `SeatProcess::start`, `spawn_child`,
stdout/stderr pump tasks, the respawn loop, and `SeatSupervisor::{stop,Drop}`;
the captured child is operated through
`SeatProcess::{status,wait,stop,Drop}` with `Command::kill_on_drop(true)`.
Each stage captures or receives S's same id, keys, ports, derived directory,
and owned child handle. Output pumps only log lossy text tagged with S and do
not parse it as a command, path, credential, id, or address.

The regenerated launch enumeration has one production launch. `SeatProcessConfig`
carries no program choice outside `cfg(test)`, where a test double replaces it:
production resolves `current_exe`, sets its `argv[0]` to `fedimintd`, and the
executable dispatches to the bundled entry before parsing FMan's CLI. A1 identifies that resolved
executable as the trusted official binary and A4 supplies the process/path and
`argv[0]` semantics. The dispatch changes no seat argument or environment:
`spawn_child` still derives all paths and ports from S, clears the environment,
and supplies only S's keys plus the common bitcoind configuration.

**R5 (enum + code + axiom) — attacker-controlled child inputs do not become
sibling host capabilities.** Guardian names/codes, metadata, and peer messages
enter S's external child, but FMan passes no sibling local credential, path,
supervisor/registry handle, or environment capability. A2 is the explicit
point where the argument bottoms out: the unsandboxed child must confine those
inputs to its supplied resources. Guardian-code peer connections are ordinary
recipient-side protocol traffic under the claim definition, not acquisition of
a local sibling capability.

For the highest-risk operation, S's single-owner `SeatLoop` dispatches
`restart_dkg`, appends an attempt under S's id (R2), stops S's retained
supervisor (R4), removes S's derived directory (R3), and respawns that
supervisor with S's resources. It never returns to the fleet registry or
accepts a second seat id.

**Conclusion.** R1 fixes the immutable facts and resources of each `Seat`.
R2–R4 show every FMan-local durable, filesystem, API, supervisor, process, and
key effect is derived from that same seat. R5 states the external-child bottom
of the guarantee and separates public protocol traffic. Thus constructing or
operating the `Seat` for S cannot obtain local control-plane access to T. ∎

## Residual windows (accepted, outside the claim)

- **R6 — independent sibling control:** another FI, operator/admin, or
  startup/supervision may independently act on T. Such an action is outside a
  call causally triggered through S's `Seat`; concurrency remains in scope only
  to ensure it cannot make S's retained identity/resources retarget T.
- **R7 — public recipient behavior:** a vulnerability in another seat's
  public Fedimint protocol implementation may affect that recipient. The claim
  covers acquisition/use of FMan-local sibling authority, not the safety of
  ordinary federation protocol traffic.
- **R8 — independent bundled entry invocation:** anyone holding the daemon
  executable can invoke its hidden `fedimintd` subcommand or run it with a
  `fedimintd` basename. That starts an independently configured process rather
  than an operation causally triggered through S's `Seat`, so it is outside the
  entry domain. Accessing an existing seat's data or localhost API that way
  additionally requires host resources covered by A1; possession of the binary
  alone supplies none. The official supervisor path remains in-claim and is
  covered by R3--R5 above.

The same-Fleet qualifier is the existing “this FMan's local control plane”
boundary made explicit, not an accepted in-scope window. Another daemon with a
different data root—and the operator's responsibility to allocate it a
non-overlapping host port grid—is not a seat registered in this `Fleet`.

## Weakest links

In order: A2's unsandboxed-child behavior; A3/A4 resource and
platform separation; R1–R5's construction, DB/path/process/input
enumerations—especially the fleet-wide `Db` and the separately supplied
`Seat::start` arguments. `SeatId` construction and schema-immutable seat
facts are the strongest rungs. Tests are regression evidence, not the proof.

## Regression attack

To attack this argument independently:

1. Enumerate every `Seat` construction site, field, public/crate-visible
   method, private helper, and caller, plus every `Fleet::authorize`
   call site. Try to replace `SeatFacts`, inject another `Seat`,
   reach the fleet registry, or make fresh/startup construction spawn resources
   for an id other than the constructed facts.
2. Starting from every exposed `Seat` method, enumerate all direct and
   indirect reads/mutations, DB methods, spawned work, local API clients,
   supervisor/process handles, ports, credentials, derived keys, and
   filesystem paths. For each, trace the target back to S and attempt to inject
   a second seat id, raw path/address, sibling credential, or sibling handle.
3. Regenerate all `Seat`-reachable DB calls and every construction/update of
   `SeatFacts.seat_id`, `port_base`, and `api_auth`. Attempt retargeting through
   restart attempts, observation writes, decommission races, crash/restart,
   stale runtime state, port overlap/reuse, credential/key collision, symlink
   or path traversal, and child respawn.
4. Treat guardian names/codes, DKG rosters, metadata, child stdout/stderr, and
   public peer messages as hostile. Attempt localhost sibling API access,
   environment/path injection, supervisor confusion, and public traffic to a
   locally hosted sibling. Identify precisely whether a blocked trace relies
   on code/type/schema or the load-bearing A2.
5. A counterexample is a causal trace from a `Seat` whose facts name S to
   a read, mutation, operation, direct local API call, credential use,
   supervisor/process control, or data-path access belonging to T != S. Public
   recipient-side protocol processing alone is not a counterexample unless it
   yields one of those FMan-local capabilities.
