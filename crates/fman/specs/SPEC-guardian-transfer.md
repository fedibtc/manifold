# SPEC-guardian-transfer: Cooperative guardian transfer between Fleet Managers

## Status

Only the protocol exists. `crates/service-fleet-manager` defines the grant,
the four verbs, the four seat statuses, and the grant's validity rule. Every
Fleet Manager answers the four verbs with `UnsupportedVerb`, never reports the
new statuses, and `fi-client` does not drive a transfer yet.

## Record justification

The transfer contract spans the FI's grant and driver, the destination FMan's
pull, the source FMan's stop and hand-over, and every guardian's directory
vote, so no single implementation artifact can own it coherently.

## Contract

An FI moves one formed guardian seat from a source FMan to a `New` seat it
bought on a destination FMan. The move is cooperative: the source must come
online once. The FI does not have to be online at the same time.

1. The FI signs a `SeatTransferGrant` naming the federation, the source
   locator and seat, the destination service key and seat, and an expiry. It
   sends the grant to the destination seat with `PullSeat`.
2. The destination persists the grant, reports `AwaitingTransfer`, and
   presents the grant to the source with `TransferSeat` until the source
   answers or the grant expires. On expiry the seat drops the grant and
   reports `TransferGrantExpired`. The source may already have answered, so
   the FI does not treat the seat as a free spare: it renews the move with
   `PullSeat` and a new grant. A `TransferGrantExpired` seat accepts a grant
   like a `New` seat.
3. The source checks the FI signature, that the FI owns the source seat, and
   that the grant names this seat and its federation. It durably records
   `Transferred`, stops its guardian, and publishes `Transferred` in the
   seat's backup before it answers. A source that cannot publish does not
   answer yet. It then returns the guardian's config and secrets sealed to
   the destination service key and bound to both seat ids. A `Transferred`
   seat never starts again, and it answers every later valid grant for the
   seat with the same guardian.
4. The destination opens the package, checks it against the grant, installs
   the guardian, publishes it in the seat's backup, and only then reports
   `Running`. It refuses a grant for a federation it already hosts. It then
   invokes the `PullSeat` completion callback. `Running` ends the
   destination's part of the transfer.
5. The FI sends `ReplaceSeatBinding` to every seat. Each FMan replaces only
   the moved peer's directory entry and fee recipient and votes. The FI
   confirms by consensus readback.
6. The FI sends `RemoveSeat` to the source. The source deletes the seat's
   guardian data and drops it from the seat's backup, which keeps the
   tombstone. It keeps the seat row and fee-wallet data, and reports
   `Removed`.

## Invariants

- The source stops before any package exists, and the destination cannot
  start without a package. At most one host runs the guardian unless the FI
  grants the same seat to two destinations.
- The grant is the only authority. Possession of a grant gives no access to
  the guardian, because the package is sealed to the destination.
- The moved guardian keeps its peer id, consensus keys, and Iroh node ids.
  Network checks cannot tell the two hosts apart.
- A restore never starts a `Transferred` guardian: the source's backup
  records `Transferred` before any package exists.
- The guardian is never only on one disk: the destination backs it up
  before it reports `Running`, and the FI removes the source only after that.
- No step undoes a transfer once the source has answered. A source that
  answered never runs the guardian again, so the only way forward is the
  destination.

## Backup and restore

The moved guardian's config and secrets, including its `api_auth`, do not
derive from the destination's mnemonic. The destination's seat backup
([SPEC-nostr-backup-restore](./SPEC-nostr-backup-restore.md)) therefore
carries them. It does not carry the grant: a restored seat that was
`AwaitingTransfer` comes back as `TransferGrantExpired`, and the FI renews
the move.

The source records the move as a tombstone in its seat backup, not as a
deletion: relays may keep a deleted event. A restored `Transferred` seat
stays stopped, and `RemoveSeat` still applies to it.

## Custody

The FI chooses the destination and is the only party that checks it. Before
it signs a grant, it refuses a destination whose FMan or verified badge
holder hosts, or ever hosted, a peer of the federation. A past host may still
hold its old key share, and no credential proves that it deleted it. The FI
keeps past hosts in its own backup for this check. The source checks only the
FI's signature and ownership.

