# SPEC-guardian-transfer: Cooperative guardian transfer between Manifold Guardians

## Status

`crates/service-manifold-guardian` defines the grant, the four verbs, the
transfer's seat statuses, the grant's validity rule, and `rebind_transferred_seat`.
`fa-client` does not drive a transfer yet. Manifold Guardian builds implement
the four verbs and resume durable transfers after restart, but only
development and staging deployments run them. Keyed on its Manifold
environment like `DecommissionSeat`
([ARCH-manifold-guardian-product-boundary](./ARCH-manifold-guardian-product-boundary.md)),
a production MG answers all four verbs `UnsupportedVerb`, in both the source
and the destination role, and never starts the transfer worker, so it
neither polls a source nor resumes a transfer stored in its database. A
production release can therefore ship this code without enabling transfer. Recovery assumes the latest published
backup is returned and stale local snapshots are never restarted, as specified
below; this implementation does not establish freshness against hostile rollback.

## Record justification

The transfer contract spans the FA's grant and driver, the destination MG's
pull, the source MG's stop and hand-over, and every guardian's directory
vote, so no single implementation artifact can own it coherently.

## Contract

An FA moves one formed guardian seat from a source MG to a `New` seat it
bought on a destination MG. The move is cooperative: the source must come
online once. The FA does not have to be online at the same time.

1. The FA signs a `SeatTransferGrant` naming the federation, the source
   locator and seat, and the destination service key and seat. A grant has
   no expiry. The FA sends it to the destination seat with `PullSeat`.
2. The destination persists the grant, reports `AwaitingTransfer`, and
   presents the grant to the source with `TransferSeat` until the source
   answers. The seat stays bound to this move: it treats any grant for the
   same move, also one the FA signed again, as a replay, and refuses a grant
   for another move. The source may already have answered, so the FA never
   treats the seat as a free spare.
3. The destination signs `TransferSeat` with the service key the grant names
   as its destination. The source checks that signature, the FA signature,
   that the FA owns the source seat, and that the grant names this seat and
   its federation. Each call answers from durable state and never waits;
   the destination retries until it gets the guardian:
   - While the guardian runs, the source asks it to stop between consensus
     sessions, never inside one (see *Session boundary*), and answers
     `AwaitingSessionEnd`. The stop durably records `Transferred`, and a
     `Transferred` seat never starts again.
   - Until the relays confirm a seat backup that records `Transferred`, it
     wakes the backup worker and answers `AwaitingBackup`. A restore from
     the relays therefore never starts the moved guardian again.
   - Then it returns the guardian's three config files and its two Iroh
     secrets in plain form; the Iroh connection between the two MGs keeps
     them confidential. It answers every later valid grant for the seat with
     the same guardian.
4. The destination validates the guardian against the grant, durably
   stores it, and starts it. From then on the seat is a formed seat that
   reports `Running`: its backup follows asynchronously as for a natively
   formed seat, and it accepts a grant for the same move as a replay. It
   refuses a grant for a federation it already hosts, but its own pending
   import does not count. It then invokes the `PullSeat` completion callback. `Running` ends the
   destination's part of the transfer.
5. The FA sends `ReplaceSeatBinding` with the destination's attestation to
   every seat except the stopped source. Each MG derives its vote from
   the consensus values in `expected_base` with `rebind_transferred_seat`:
   the moved peer's directory entry becomes the new attestation, and the
   peer's fee recipient share moves to the new account. Nothing else
   changes. The FA confirms by readback that consensus holds exactly those
   values.
6. The FA sends `RemoveSeat` to the source. The source deletes the seat's
   guardian data at once and reports `Removed`; its next backup publication
   drops the archive and keeps the tombstone. A failed removal leaves the
   seat `Transferred`, and the FA retries. It keeps the seat row and
   fee-wallet data.

## Invariants

- The source stops before it hands the guardian over, and the destination
  cannot start without it. At most one host runs the guardian unless the FA
  grants the same seat to two destinations.
- The source hands over only a guardian that exited at a session boundary, so
  the moved key never signs units in a session that the source has already
  signed units in.
- Only the session-boundary exit code retires a seat, and only
  `TransferSeat` asks for it. A guardian that stops in any other way,
  including a shutdown an operator schedules through the older `shutdown`
  endpoint, restarts and keeps its seat.
- The grant is the only authority. Possession of a grant gives no access to
  the guardian, because the source answers only the destination the grant
  names, authenticated by its service-key signature.
- The moved guardian keeps its peer id, consensus keys, and Iroh node ids.
  Network checks cannot tell the two hosts apart.
- A restore never starts a `Transferred` guardian: the source's backup
  records `Transferred` before any package exists.
- The destination backs the guardian up like any formed seat. Until that
  backup confirms, the source's stopped copy is the only other one; an FA
  that wants two copies before `RemoveSeat` waits for the destination's
  backup.
- No step undoes a transfer once the source has answered. A source that
  answered never runs the guardian again, so the only way forward is the
  destination.

## Session boundary

The destination starts the guardian with an empty database. It downloads the
signed outcomes of finished sessions and joins the federation's current
session at round 0 with the moved key. If the source had already signed
consensus units in that session, peers would see two different units from
one key, mark the guardian a forker for the rest of the session, and keep
that evidence across restarts. Each such guardian consumes one unit of the
federation's fault budget, so a few transfers in one session can halt the
federation. The source therefore hands over only a guardian that stopped
between sessions:

- On every `TransferSeat` it asks its running fedimintd, through the bundled
  fedimintd's `shutdown_after_current_session` admin endpoint, to exit once
  the session it is running completes, without starting another. The
  request is idempotent, and it lives only in the fedimintd process.
- fedimintd exits with a dedicated session-boundary exit code only on that
  path: its last session completed and no later one began. A shutdown
  scheduled for a given session through fedimintd's older `shutdown`
  endpoint exits with its ordinary code. On the session-boundary exit the
  source records `Transferred`, which also keeps the supervisor from ever
  starting the guardian again.
- Only the source MG, on `TransferSeat`, calls
  `shutdown_after_current_session`, so the exit code is the handover. This
  is an accepted risk: anyone holding the guardian's admin password can call
  that endpoint by hand, and the source then retires the seat as if it had
  been transferred. The same password can already stop the guardian.
- Any other exit is a crash, including a session timeout, a panic, and a
  kill by the watchdog or the operator. The guardian restarts from its own
  database, which continues its session rather than forking it, and the
  destination's next `TransferSeat` asks the new process.

The wait costs up to one session plus the minute fedimintd waits after its
last session for peers to finish it. A source whose guardian is down when
`TransferSeat` arrives asks nothing; the guardian restarts as usual and a
later retry asks it. While the federation cannot end the session, for
example with too many peers offline, the guardian keeps running and the
source keeps answering `AwaitingSessionEnd`. It never falls back to a
mid-session stop.

## Backup and restore

The moved guardian's config and its two Iroh secrets do not derive from the
destination's mnemonic. The destination's seat backup
([SPEC-nostr-backup-restore](./SPEC-nostr-backup-restore.md)) therefore
carries the Iroh secrets once the guardian is accepted, beside the archive it
carries for every formed seat. Every document carrying transfer facts is
sealed under an envelope version that builds from before guardian transfer
refuse, so they never restore a stopped source or an imported identity as an
ordinary seat. The guardian's admin password is not moved:
each MG runs the child under its own derived `api_auth`. It does not carry the grant: a
restored seat that was `AwaitingTransfer` comes back as `New`, and the FA
sends the move again with `PullSeat`.

The source records the move as a tombstone in its seat backup, not as a
deletion: relays may keep a deleted event. A restored `Transferred` seat
stays stopped, and `RemoveSeat` still applies to it. A confirmed publish does
not stop a relay from serving the older running-seat event instead. Safe recovery
therefore assumes the configured backup relay returns the latest published seat
state, including retirement/removal tombstones, and operators do not restart
stale local snapshots. Under this premise,
[CLAIM-manifold-guardian-restore-adopts-authentic-consistent-state](./CLAIM-manifold-guardian-restore-adopts-authentic-consistent-state.md)
states the authenticity and consistency obligation. It remains unverified.
There is no independent freshness checkpoint: violating the latest-backup
premise can resurrect the source alongside the destination. The source's
mandatory retirement publication is retained, but does not solve hostile
rollback.

## Custody

The FA chooses the destination and is the only party that checks it. Before
it signs a grant, it refuses a destination whose MG or verified badge
holder hosts, or ever hosted, a peer of the federation. A past host may still
hold its old key share, and no credential proves that it deleted it. The FA
keeps past hosts in its own backup for this check. The source checks only the
FA's signature and ownership.
