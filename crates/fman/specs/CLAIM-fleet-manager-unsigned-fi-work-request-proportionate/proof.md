# Proof: Unsigned FI work is request-proportionate

## Evidence maintenance

The mint-v2 receive-replay narrow review found that one exact protected
operation-log lookup occurs only inside a selected wallet's inbound receive
after `AlreadyReceived`. It adds no unsigned FI verb, trust request, seat
fan-out, permit retention, or network wait. The documented trust-material
counterexample and the claim's `Falsified` status remain unchanged. This local
maintenance does not re-verify the claim.

## Scope and model

This proof supports
[CLAIM-fleet-manager-unsigned-fi-work-request-proportionate](../CLAIM-fleet-manager-unsigned-fi-work-request-proportionate.md).
It reads `crates/fman/core/src/{fleet,service,seat,fedimint_api}.rs`,
`crates/fman/core/src/db.rs`, `crates/fman/fedimint/src/{lib,payee}.rs`,
`crates/fman/bin/src/main.rs`,
`crates/locked-payments/src/{denominations,locked_payment,locked_payment_v2,refund}.rs`,
`crates/fedi-iroh-rpc/src/iroh_protocol.rs`,
`crates/service-fleet-manager/src/{locked_payment,service}.rs`, and
`crates/domain/src/fman_federation_directory.rs`.

The model grants bounded raw request volume but keeps daemon-side amplification
in scope. One child may delay a local request until its timeout. The property
requires work to remain proportionate to a small request and prevents that one
slow dependency from consuming shared capacity needed by unrelated FI verbs.
Authorization and disclosure are outside this cost-and-liveness argument.

## Assumption boundary

The proof grants the claim record’s three assumptions. It does not treat a fixed
handler-sized request batch as an excluded volumetric flood, and it does not
assume that a slow child fails before the configured timeout.

## Argument

**L1 (code) — availability is a bounded snapshot but is repeated uncached.**
`GetAvailability` opens one SQLite transaction and performs three reads: all
admitted payment-federation ids, the singleton offer row, and an aggregate over
the seats table. It then tests the admitted ids until one locally open wallet
client is found. Production `receivable` rebuilds/scans the wallet's in-memory
payment-client id set; it does not contact guardians. With at most 16 admitted
ids, the work is bounded by those repeated wallet-map scans plus the database
aggregate, allocates ids and plans, and is not cached per request. The
setup-payment document bounds its admitted set;
there is no child or remote wallet IO in this verb.

**L2 (code) — quote work is local and input-bounded, but also uncached.** After
constant-size version/size checks, `GetQuote` takes the same SQLite offer
snapshot. A free quote generates a nonce, composes terms, and signs once. A
paid quote additionally selects one already joined payment client, derives
locked issuance across that federation's configured denomination tiers,
re-derives expected refund denominations and validates the supplied issuance
nonces, serializes the terms, and signs. These are wallet touches and
cryptographic allocations on every request, but do not submit to or wait for
federation guardians. Quote and refund expansion reject representations above
64 notes before allocating their output vectors; the refund remainder search
examines at most 1,000,000 msat across the client's finite configured tier set.
Together with request frame/semantic bounds, these explicit work bounds limit
one call; repetition alone is the request volume A-abuse-controls owns.

**L3 (code) — trust material ignores the cheap filter until after fleet-wide
child IO.** The request is semantically capped at 4 KiB and 64 peer ids. Despite
that small input, `federation_bindings` first clones **every** seat in the fleet
and sequentially calls `Seat::federation_binding` on each, then compares that
seat's returned federation id/config hash before moving to the next seat. Each
call occupies that seat's serialized
command loop and, for a running seat, performs local child RPC for `status`,
`client_config`, and `invite_code`, deriving and parsing the whole federation
configuration. This per-seat federation comparison cannot avoid probing any
seat. Only after the fleet scan does service code build the peer-id filter. It
then signs one attestation per match, clones every cached holder
authorization, digests the complete response, and signs it. Neither the
request's federation mismatch nor a one-peer filter avoids the scan. No binding
or final response is cached.

The configured `max_seats` is an unconstrained `u32`; actual port allocation
places a host-dependent ceiling on live seats, but the cost is still O(all
installed seats), not O(the small request or returned matches). Consumer-side
limits of 64 attestations and 128 KiB do not cap server-side scanning before a
response is built.

**L4 (code) — transport turns repeated fan-out into a global permit amplifier.**
Iroh grants 128 global stream permits. A permit covers request read, decoded
service execution, and typed-service response serialization; it is released
before the outer response-frame encoding and response write. Each seat command
channel has capacity one and its loop processes
commands serially. Consequently concurrent trust-material handlers aimed at
any federation queue behind the same first slow seat while retaining their
global permits. The ten-second Iroh timeout covers only initial frame reading;
the child client's separate five-second connection and ten-second request
timeouts bound each active child operation but do not bound time spent waiting
behind earlier seat commands.

The first two verbs therefore have bounded local work under the assumptions.
L3 and L4 do not establish the property for `GetFederationTrustMaterial`:
they expose fleet-wide child I/O before filtering and retention of the global
permit while handlers queue behind a capacity-one seat loop. The current
counterexample is preserved in
[falsification-trust-material-seat-fanout.md](falsification-trust-material-seat-fanout.md).

## Residual windows

- The exact delay observed by an unrelated request depends on semaphore wake
  order and when it arrives. The
  [counterexample](falsification-trust-material-seat-fanout.md) needs only the
  reachable initial interval in which all permits are held; once the first
  timeout releases a permit, a queued unrelated request can run.
  It does not claim every unrelated request waits for all 128 child timeouts.
- `GetAvailability`'s SQLite aggregate and `GetQuote`'s denomination/signing
  work are uncached, but one request is bounded by the quote/refund output caps
  and fixed refund-repair search. This record does not label ordinary bounded
  healthy cost a violation once A-abuse-controls limits repetition.
- A trust response can itself exceed verifier limits if the fleet/source
  contains too many matching bindings or authorizations. That correctness gap
  is not needed for the documented counterexample; the amplification occurs
  before response validation or writing.
- Child RPC deadlines rely on Tokio monotonic time. V9 clock behavior is owned
  by
  the persisted-wall-clock recovery gap.

## Weakest links

The qualitative word “disproportionate” is grounded here in two concrete
ratios: O(all seats) work for a constant-size mismatch and 128 accepted calls
retaining the whole fixed handler pool while a capacity-one seat queue performs
serial timed IO. The 1,280-second aggregate/last-request tail is illustrative for a withholding
`status` request; connection refusal or an earlier failure is cheaper. The
source reading has no load regression test, but the scan/filter order, channel
capacity, timeouts, and permit scope are explicit.
