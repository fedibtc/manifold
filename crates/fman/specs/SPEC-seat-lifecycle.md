# SPEC-seat-lifecycle: Guardian Seat Lifecycle

## Record justification

SQLite, the per-seat runtime loop, the driven child protocol, restore, and the
FI RPC boundary jointly implement this lifecycle; no one implementation file
can own the contract.

## Three durable facts

A guardian seat has exactly three lifecycle facts:

1. its immutable `seats` identity row (owner, quote, plan, federation size,
   never-reused seat number, and creation time);
2. an optional set-once `formed_seats` row containing
   `{ federation_invite, formed_at_ms }`; and
3. an optional terminal `decommissioned_seats` row.

Formation attempts, guardian codes, federation names, submitted code sets,
start acknowledgements, interruption reasons, and ceremony latches are not
persisted. Restore is not a separate lifecycle case: after atomically installing
the recovered final directory it writes the same formed record as driven DKG.

A completion callback is delivery work rather than ceremony state. The current
optional callback and its retry/terminalization fields live in
`completion_callbacks`. The first `StartDkg` choice is retained across later
attempts for the same formation.
Formation, restore, callback delivery, and decommission never mutate or delete a
formed record.

## Formation

`GetDkgCode` is a pure deterministic function of the seat's derived Iroh keys,
the guardian name `fm-` followed by the first eight hexadecimal characters of
the seat id, and the optional leader-only federation name. It returns the bare
upstream Fedimint base32 setup code and performs no child or database write. Running and
decommissioned seats refuse it.

`StartDkg` validates the complete code set before sending `RunDkg` to its parked
`NeedsParams` child. It locates
this seat's code by its embedded Iroh API key, decodes that code's federation
name, recomputes the complete bare setup code byte-for-byte, and refuses
a mismatch. The sorted upstream setup codes determine `our_index`. Success is
returned only after the current driven child emits `DkgStarted`; parameter
rejection is synchronous. Only the in-memory seat loop serializes a ceremony.
No request is retained or replayed after child death.

FMan deliberately does not endpoint-sign or cross-verify other guardians'
setup codes. Fedimint's DKG peer-to-peer handshake authenticates the endpoint
keys inside those codes, while the FI is only ferrying the code set and cannot
gain an additional trust guarantee by having each endpoint pre-sign its own
input. Post-DKG fee-account attribution instead uses the configured endpoint
key's `SeatEndpointProof` over the account-carrying FMan attestation.

`ConfigPersisted` follows fedimintd's atomic rename of a complete staging
configuration into the final data directory. FMan inserts the formed record and
marks backup/callback work. `AlreadyConfigured` performs the same insertion, so
a crash after rename but before the database write self-heals.

An interrupted conversation has nothing to resume. Once its session is gone the
parked child projects as `New`; any last child error is diagnostic
memory only and disappears on FMan restart. The FI owns ceremony patience and
human retry policy; FMan bounds only the local `DkgStarted` acknowledgement.

`RestartDkg` accepts both an idle and an acknowledged in-memory ceremony. It
stops and reaps the current child, drops only that child's staging state, starts
a replacement, and reads its initial `Hello`. `NeedsParams` causes the supplied
complete code set to be validated exactly as for `StartDkg`, followed by
`RunDkg` and a bounded wait for `DkgStarted`; the response is `DkgInProcess`.
`AlreadyConfigured` repairs the formed record and returns `Running` without
starting a second ceremony. Running, `DataLoss`, and decommissioned seats refuse
restart before the child is touched. Restart never removes the final directory.
There is no standalone FI cancellation verb. Operator decommission is the only
release path in production; development and staging also let the seat's own FI
request that same terminal decommission
([SPEC-fi-rpc](./SPEC-fi-rpc.md)).

## Structural destruction invariant

The final `seats/{seat_no}/data` directory exists iff fedimintd has atomically
installed a complete configuration. Formation and restart may discard
only staging directories. **No FMan path removes a final seat data
directory.** Decommission is a decision about capacity, not about
the guardian material a federation may still depend on, so it retains that
directory along with the durable seat, payment evidence, and DKG history.
Removing guardian data stays a separate, deliberate operator step.

## Status and reads

The FI-facing lifecycle statuses are:

- `New`: no formed record and no live DKG session;
- `DkgInProcess`: the current in-memory session has acknowledged start;
- `Running { invite }`: a formed record exists and the final directory exists;
- `DataLoss`: a formed record exists but the final directory is absent; and
- `Decommissioned`: the terminal record exists.

At startup, every non-decommissioned seat spawns a child. An unformed or
`DataLoss` child remains parked in `NeedsParams`; a formed child serves its
configuration. All use capped-backoff respawn, which resets after a sufficiently
long run.

`GetStatus` derives its response from the shared source facts and performs only
an inline filesystem stat so disappearance of the final directory is
immediately visible as `DataLoss`; it never waits on the seat loop or contacts
the child. For a formed seat whose final directory exists, a watchdog probes
the local consensus API under a one-second timeout every 60 seconds while
healthy and every 5 seconds
while unavailable. The faster unavailable cadence bounds stale refusal after a
child recovers. Unformed, `DataLoss`, and decommissioned seats are never probed.

`GetInviteCode` reads the formed record alone and remains available while the
child is down. A configured child's `Hello` is the sole repair path for a
missing formed record: every spawn reports `AlreadyConfigured { invite_code }`
before serving, and the seat loop records that invite. The watchdog has no
lifecycle side effects. A formed record whose directory is absent is never
treated as a new seat.

## Formation endpoint proof

DKG carries only the upstream guardian code set. After DKG, each FMan signs its
peer attestation with both its stable FMan key and, separately, the configured
seat endpoint key. The FI supplies each attestation paired with its endpoint
proof to `ProposeFormationMeta`; each FMan constructs the bounded canonical
directory and verifies every pair against the final config before
admitting the directory and initial fee policy. No guardian-code transcript is
persisted or re-supplied for later metadata maintenance.

## Decommission

Decommission is terminal and idempotent. It records the terminal row,
terminalizes any resumable callback, then stops the child, releases the
capacity slot, and closes the seat loop. Recording the mark before stopping
the child means a crash between the two cannot leave a seat still running
that is no longer recorded as live. Immutable identity, payment, formed, and
backup evidence remain for recovery and dispute handling, as does the seat's
on-disk guardian data; the lifetime port allocation is never reused.

## Ceremony and health state

The lifecycle store contains only immutable seat identity, the optional set-once
formed record, and terminal decommission. Ceremony state is deliberately
process-local.

`GetDkgCode` is pure computation. `StartDkg` owns one driven conversation in the
seat loop; a child or daemon restart loses that conversation and the seat is
`New`. FMan reconstructs neither a RunDkg request nor an interruption
latch. The seat loop directly owns the ceremony child and driven client and
consumes its initial state and lifecycle events in order. `RestartDkg` replaces
the child, derives its result from the replacement's initial state, and starts a
fresh ceremony only when that state is `NeedsParams`.

The final data directory is structural evidence: driven DKG creates it only by
atomically renaming a complete staging configuration. `AlreadyConfigured`
repairs a missing formed record. Conversely, a formed record with no final
directory is `DataLoss`, never an invitation to form again.

No path removes the final directory: formation and restart discard only
staging, and decommission retains it. Thus safety does not depend on a
procedural restart guard, attempt log, or crash-window replay protocol.

After formation, the seat loop owns a periodic watchdog. One local consensus
API probe has a one-second outer bound; a healthy seat is checked every 60
seconds and an unavailable seat every 5 seconds. The faster retry bounds how
long a recovered seat can still be refused by a stale unavailable snapshot.
Unformed, `DataLoss`, and decommissioned seats are ineligible for the watchdog
and are never probed.

Requests only read the watchdog's health snapshot. They never initiate a
probe: an unavailable seat is refused immediately, while a healthy seat makes
only the substantive API call its verb requires. A child can fail after the
snapshot, but the substantive call then reports that failure normally. Thus FI
request volume cannot become probe volume and no request waits on a missing
child merely to establish liveness.

The watchdog has no lifecycle side effects. `ConfigPersisted`, restore, and an
already-configured child's `Hello` are the only formed-record writers; every
spawn therefore repairs a missing record without an FI status request. Durable
invite reads use only that record. Callback delivery uses its own durable work
row and begins only after formation is durable.
