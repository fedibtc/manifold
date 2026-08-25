# CLAIM-duplicate-operator-withdrawal: Duplicate operator withdrawal

For one authenticated operator economic withdrawal intent, FLIP can never cause
(a) two logically distinct gatewayd `send_onchain` invocations or (b) two
settled on-chain payments, even when the same Admin HTTP request is delivered
more than once, the client retries after losing any FLIP response, calls execute
concurrently, gatewayd calls time out or their replies are lost, and FLIP
crashes at any instruction boundary and restarts.

An **economic intent key** must be an opaque, client-generated idempotency value,
scoped to the authenticated operator principal and stable across every delivery
or retry of that intent. Address, amount, and fee rate are intent data, not an
intent key: an operator must remain able to make two deliberately distinct,
identical withdrawals. A settled payment is a distinct Bitcoin transaction
output paying the requested address which reaches the configured settlement
criterion. The adversary may schedule or duplicate authenticated deliveries,
lose responses on either network boundary, and choose crash and concurrent-call
interleavings. The authenticated client may be buggy or retry aggressively, but
labels its duplicate deliveries with the same semantic intent. Gatewayd is
non-Byzantine: it may accept a call and lose or delay its reply, but one
`send_onchain` invocation creates at most one payment. An unauthenticated caller,
a malicious gatewayd that creates multiple payments for one invocation, and an
operator deliberately creating two different economic intent keys are not in
the quantified execution.

## Status

Unverified.

## Assumptions

- **A1 — transport/authentication.** Axum and Serde deliver each well-formed,
  bearer-authenticated `POST /admin/v1/request_withdrawal` body to the handler
  once per HTTP delivery. The one static bearer token identifies authorization
  to the installation, not a stable individual operator principal, and supplies
  no request identity. Malformed or unauthenticated deliveries do not reach the service method.
- **A2 — SQLite boundary.** A successfully committed SQLite transaction is
  durable across restart, primary-key and unique-index conflicts reject the
  write, and the daemon's data-directory lock permits one active daemon process
  for that database. SQLite may serialize concurrent writers in either order.
- **A3 — gatewayd boundary.** `fedimint_gateway_client::send_onchain` accepts no
  FLIP operation/idempotency key. Separate successful invocations may create
  separate transactions even when address, amount, and fee rate are identical;
  one invocation creates at most one payment. The call may have taken effect
  before returning an error, timing out, or losing its reply.
