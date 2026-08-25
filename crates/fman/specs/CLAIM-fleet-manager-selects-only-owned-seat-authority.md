# CLAIM-fleet-manager-selects-only-owned-seat-authority: FI requests select only their owner's seat authority

For every invocation of the official daemon's FI-authenticated RPC service whose
`SignedRequest` verification succeeds, let K be the verified outer `FiId`.

A post-creation request cannot obtain any registered seat's local `Seat`
authority beyond resolving its own typed `seat_id` for the ownership comparison,
and cannot reach seat-specific behavior, except on the seat named by that typed
`seat_id` and only when the seat's durable `seats.fi_id` is K. An absent seat or
owner mismatch produces semantic `UnknownSeat` before any seat operation or
seat-specific output.

`CreateSeat` cannot return another identity's stored commitment or construct,
register, or start a seat unless its row was first durably inserted with
`seats.fi_id = K`. Payment verification and settlement continuations obtain no
Fleet Manager-local `Seat`, registry or database seat ID, API credential or
port, supervisor or process handle, key, or seat data path. Capacity computation
may read only aggregate decommission liveness and the fleet-wide port cursor; it
does not select a seat.

This property covers arbitrary valid signed inputs, known victim IDs, replay,
crash and restart, and concurrent FI or trusted operator activity. Captured
valid envelopes remain attributed to their actual signer. Ordinary public
Fedimint traffic does not constitute local seat selection.

## Status

Falsified: the claim worker reads a persisted payment record's `seat_id` and
uses it to record the claim outcome, contrary to the stated ban on payment
continuations obtaining a Fleet Manager database seat ID. See
[the current counterexample](CLAIM-fleet-manager-selects-only-owned-seat-authority/falsification-claim-worker-seat-id.md).

## Assumptions

- BIP-340 signatures are unforgeable without the private key, SHA-256 has the
  required collision and preimage resistance, and successful request
  verification binds the exact direction, verb, and payload to K.
- Fleet Manager's signing key is uncompromised; its signatures and SHA-256 have
  the same properties; and distinct exact quote payloads do not share a quote
  ID, so a verified quote and ID fix its quoted FI identity.
- SQLite and SQLx writes are atomic and durable and enforce their declared
  constraints, triggers, and connection settings; persisted rows decode
  faithfully; the data-root lock excludes a second daemon; and no alternate
  writer, prior corruption, unsafe code, or memory corruption bypasses these
  mechanisms.
- The official daemon binary, startup configuration, production service and
  wallet wiring, and trusted local operator, admin, startup, and supervision
  principals behave as documented. Alternate service or wallet implementations
  are outside the property.
- Tokio task spawning, detachment, and polling, and process/runtime termination,
  behave as documented. Client disconnect may fail response I/O but does not
  abort service work while the daemon continues running.
