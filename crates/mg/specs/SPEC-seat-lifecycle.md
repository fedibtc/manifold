# SPEC-seat-lifecycle: Guardian Seat Lifecycle

## Record justification

SQLite, the per-seat runtime loop, the driven child protocol, restore, and the
FA RPC boundary jointly implement this lifecycle; no one implementation file
can own the contract.

## Durable lifecycle facts and ceremony inputs

A guardian seat has three lifecycle facts:

1. its immutable `seats` identity row (owner, quote, plan, federation size,
   never-reused seat number, and creation time);
2. an optional set-once `formed_seats` row containing
   `{ federation_invite, formed_at_ms }`; and
3. an optional terminal `decommissioned_seats` row.

The complete validated guardian-code set for an accepted `StartDkg` or
`RestartDkg` is also stored per seat for automatic replay after child or daemon
death. DKG round progress, start acknowledgements, interruption reasons, and
ceremony latches are not persisted by MG. Restore is not a separate lifecycle
case: after atomically installing
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
upstream Fedimint base32 setup code and performs no child or database write.
Running and
decommissioned seats refuse it.

`StartDkg` validates the complete code set before sending `RunDkg` to its parked
`NeedsParams` child. It locates
this seat's code by its embedded Iroh API key, decodes that code's federation
name, recomputes the complete bare setup code byte-for-byte, and refuses
a mismatch. The sorted upstream setup codes determine `our_index`.
The validated code set and first callback choice commit before `RunDkg` is sent. Success is returned only after the current driven child emits `DkgStarted`;
parameter rejection is synchronous. Only the in-memory seat loop serializes a
ceremony. On child or daemon restart, an unformed, non-decommissioned seat
revalidates its stored set and automatically sends it to a `NeedsParams` child.
`AlreadyConfigured` instead repairs the formed record without a new DKG.

MG deliberately does not endpoint-sign or cross-verify other guardians'
setup codes. Fedimint's DKG peer-to-peer handshake authenticates the endpoint
keys inside those codes, while the FA is only ferrying the code set and cannot
gain an additional trust guarantee by having each endpoint pre-sign its own
input. Post-DKG fee-account attribution instead uses the configured endpoint
key's `SeatEndpointProof` over the account-carrying MG attestation.

`ConfigPersisted` follows fedimintd's atomic rename of a complete staging
configuration into the final data directory. MG inserts the formed record and
marks backup/callback work. `AlreadyConfigured` performs the same insertion, so
a crash after rename but before the database write self-heals.

An interrupted child has no cryptographic state to resume. Its replacement
starts a fresh local attempt using the retained envelope; there is no guarantee
that peers in later DKG rounds can still converge without a coordinated retry.
MG bounds the local `DkgStarted` acknowledgement, not the full ceremony.

`RestartDkg` accepts both an idle and an acknowledged in-memory ceremony. It
validates and commits the replacement code set before stopping and reaping the
current child. If MG dies during that stop, the next process replays the new
set. The replacement child's initial `Hello` decides the result: `NeedsParams`
replays the stored set with a bounded wait for `DkgStarted` and returns
`DkgInProcess`.
`AlreadyConfigured` repairs the formed record and returns `Running` without
starting a second ceremony. Running, `DataLoss`, and decommissioned seats refuse
restart before the child is touched. Restart never removes the final directory.
There is no standalone FA cancellation verb. Operator decommission is the only
release path in production; development and staging also let the seat's own FA
request that same terminal decommission
([SPEC-fa-rpc](./SPEC-fa-rpc.md)).

## Structural destruction invariant

The final `seats/{seat_no}/data` directory exists iff fedimintd has atomically
installed a complete configuration. Formation and restart may discard
only staging directories. **Only FA-authorized removal of an already
transfer-retired guardian may remove a final seat data directory**, under the
narrow exception below. Decommission is a decision about capacity, not about
the guardian material a federation may still depend on, so it retains that
directory along with the durable seat, payment evidence, and DKG history.
It never acquires the transfer-removal exception implicitly.

### Transfer-retired cleanup exception

The transfer implementation serializes retirement, export and removal in the
seat owner ([SPEC-guardian-transfer](./SPEC-guardian-transfer.md)). Retirement
asks the child to exit once its current consensus session completes and
commits its permanent fence, which releases capacity, only when the child
exits with fedimintd's session-boundary exit code; any other exit leaves no
fence, the child restarts, and the destination's retry asks again. Export
requires that fence and a relay-confirmed seat backup that records it; until
then the source answers "not yet" without waiting.

`RemoveSeat` deletes the final guardian data directory and the known
guardian-transfer work subtree of a `Transferred` seat, then commits the
removal fence that rejects all later exports; the archive-less tombstone is the
seat's next publication. The accepted import's Iroh-key row is dropped with
that fence, but the outgoing tombstone, formed invite, seat/payment facts, fee
wallets and safe-event journal remain. A failed removal leaves the seat
`Transferred`, and the FA's retry deletes again. This does not prove erasure of
historical backups or source copies, and does not authorize general cleanup
from formation, restart or decommission.

Transfers are enabled in ordinary builds. Recovery assumes latest published
backups and no stale local snapshots, as specified in SPEC-guardian-transfer.
Publication confirmation does not establish freshness against hostile rollback.

## Status and reads

The FA-facing lifecycle statuses are:

- `New`: no formed record and no live DKG session;
- `DkgInProcess`: the current in-memory session has acknowledged start;
- `Running { invite }`: a formed record exists and the final directory exists;
- `DataLoss`: a formed record exists but the final directory is absent; and
- `Decommissioned`: the terminal record exists.

At startup, every non-decommissioned seat spawns a child. An unformed child
with retained inputs automatically starts a fresh attempt when the child reports
`NeedsParams`; without retained inputs it stays parked. A `DataLoss` child stays
parked and never replays inputs; a formed child serves its configuration.
All use capped-backoff respawn, which resets after a sufficiently long run.

`GetStatus` derives its response from the shared source facts and performs only
an inline filesystem stat so disappearance of the final directory is
immediately visible as `DataLoss`; it never waits on the seat loop or contacts
the child. For a formed seat whose final directory exists, a watchdog probes
the local consensus API under a one-second timeout every 60 seconds while
healthy and every 5 seconds
while unavailable. The faster unavailable cadence bounds stale refusal after a
child recovers. Unformed, `DataLoss`, and decommissioned seats are never probed.
A configured child whose local API fails every probe for 15 minutes is killed
and respawned: the API is local, so such an outage means a wedged child, which
would otherwise never exit on its own.

`GetInviteCode` reads the formed record alone and remains available while the
child is down. A configured child's `Hello` is the sole repair path for a
missing formed record: every spawn reports `AlreadyConfigured { invite_code }`
before serving, and the seat loop records that invite. The watchdog never
writes the formed record. A formed record whose directory is absent is never
treated as a new seat.

## Formation endpoint proof

DKG carries only the upstream guardian code set. After DKG, each MG signs its
peer attestation with both its stable MG key and, separately, the configured
seat endpoint key. The FA supplies each attestation paired with its endpoint
proof to `ProposeFormationMeta`; each MG constructs the bounded canonical
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

The lifecycle store retains immutable seat identity, optional set-once formed
record, terminal decommission, and the latest validated code set. DKG progress
remains process-local.

`GetDkgCode` is pure computation. `StartDkg` owns one driven conversation in the
seat loop; a child or daemon restart loses its progress, but MG reconstructs
the `RunDkg` request from retained validated codes when the new child is
parked. The seat loop directly owns the ceremony child and driven client and
consumes its initial state and lifecycle events in order. `RestartDkg` commits
the replacement set before stopping the child, then starts a fresh ceremony
only when the replacement reports `NeedsParams`.

The final data directory is structural evidence: driven DKG creates it only by
atomically renaming a complete staging configuration. `AlreadyConfigured`
repairs a missing formed record. Conversely, a formed record with no final
directory is `DataLoss`, never an invitation to form again.

Formation and restart discard only staging, and decommission retains the final
directory. Transfer-retired cleanup is the exception above, protected by its
permanent durable fence even after the directory disappears. No missing
directory on a retired seat can authorize another ceremony or child start.

After formation, the seat loop owns a periodic watchdog. One local consensus
API probe has a one-second outer bound; a healthy seat is checked every 60
seconds and an unavailable seat every 5 seconds. The faster retry bounds how
long a recovered seat can still be refused by a stale unavailable snapshot.
Unformed, `DataLoss`, and decommissioned seats are ineligible for the watchdog
and are never probed.

Requests only read the watchdog's health snapshot. They never initiate a
probe: an unavailable seat is refused immediately, while a healthy seat makes
only the substantive API call its verb requires. A child can fail after the
snapshot, but the substantive call then reports that failure normally. Thus FA
request volume cannot become probe volume and no request waits on a missing
child merely to establish liveness.

The watchdog's only lifecycle side effect is that unresponsive-child restart; it
never writes the formed record. `ConfigPersisted`, restore, and an
already-configured child's `Hello` are the only formed-record writers; every
spawn therefore repairs a missing record without an FA status request. Durable
invite reads use only that record. Callback delivery uses its own durable work
row and begins only after formation is durable.
