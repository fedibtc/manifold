# CLAIM-fleet-manager-value-moves-use-client-authority: Payment and fee value moves use only client authority

Every value-moving Fedimint request causally started by a production accepted-FI-
setup-payment claim, guardian-fee collection, or payment- or
guardian-fee payout is authorized only as an ordinary federation client. This
includes native Lightning v1/v2 starts, replay, exact-operation observation,
dependency refund/reclaim state machines, and operations resumed during lazy
client recovery. None uses the guarded seat's mnemonic-derived `SeatApiAuth`,
invokes an authenticated guardian or admin mutation, or directly accesses the
child database to create, redirect, claim, refund, reclaim, or withdraw value.

The domain includes every production caller of accepted setup-payment claim
work, `GuardianFeeVault::collect`, and `EcashPayoutWorker` payment and
guardian sweep/status/await operations, plus every value-moving request reachable
from them. Read-only local-child discovery and ordinary public client operations
do not violate the property unless they authorize a later value move with local
guardian authority.

## Assumptions

- **A1 — language and dispatch integrity:** Safe Rust ownership, trait dispatch,
  and asynchronous task capture behave as written; no unsafe memory corruption
  or alternate binary changes the production call graph.
- **A2 — pinned client authority boundary:** A `fedimint_client::Client` joined
  from a public invite, its operation executor, and operations resumed while
  opening or recovering it use only ordinary client, module, note, supplied
  `BtcDepositor`, or existing-operation authority owned by that ordinary client,
  with no server, guardian, or admin capability. This includes
  stability-pool account withdrawal and deferred outputs; mint-v1 reissue,
  mint-v2 receive, and mint-note spend/cancellation/reclaim; native Lightning
  v1/v2 funding, payment, change, refund, and reclaim state machines; and
  observation/await of an existing operation.
- **A3 — storage and process integrity:** The configured wallet database and a
  seat child's data directory are distinct stores, and the operating system does
  not grant one client access to the other unless the implementation supplies an
  explicit access path.
