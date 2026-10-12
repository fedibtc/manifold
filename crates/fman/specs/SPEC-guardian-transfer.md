# SPEC-guardian-transfer: Cooperative guardian transfer between Fleet Managers

## Status

Only the protocol exists. `crates/service-fleet-manager` defines the grant,
the four verbs, the three seat statuses, and the grant's validity rule. Every
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
   answers or the grant expires. On expiry the seat returns to `New`; the FI
   keeps it as a spare.
3. The source checks the FI signature, that the FI owns the source seat, and
   that the grant names this seat and its federation. It durably records
   `Transferred` and stops its guardian before it answers. It then returns
   the guardian's config and secrets sealed to the destination service key and
   bound to both seat ids. A `Transferred` seat never starts again, and it may
   answer later presentations of a valid grant again.
4. The destination opens the package, checks it against the grant, installs
   the guardian, and reports `Running`. It refuses a grant for a federation
   it already hosts. It then invokes the `PullSeat` completion callback.
5. The FI sends `ReplaceSeatBinding` to every seat. Each FMan replaces only
   the moved peer's directory entry and fee recipient and votes. The FI
   confirms by consensus readback.
6. The FI sends `RemoveSeat` to the source. The source deletes the seat's
   guardian data and backup, keeps the seat row and fee-wallet data, and
   reports `Removed`.

## Invariants

- The source stops before any package exists, and the destination cannot
  start without a package. At most one host runs the guardian unless the FI
  grants the same seat to two destinations.
- The grant is the only authority. Possession of a grant gives no access to
  the guardian, because the package is sealed to the destination.
- The moved guardian keeps its peer id, consensus keys, and Iroh node ids.
  Network checks cannot tell the two hosts apart.
