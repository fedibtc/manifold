# Proof: Unsigned FI work is request-proportionate

## Evidence maintenance

The FMan-scoped trust-material redesign removed federation/peer selectors and
all fleet/seat traversal from the public trust read. The former seat-fanout
counterexample was checked against the new handler and is no longer reachable:
the handler reads one bounded in-memory authorization snapshot and the current
endpoint, then signs and validates one bounded response without child I/O. The
obsolete falsification record was removed. This maintenance sets the claim to
Unverified rather than treating repair of one counterexample as full
re-verification.

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

**L3 (code) — FMan trust material is one bounded identity snapshot.**
`GetFmanTrustMaterial` accepts only `ProtocolV1`; unknown fields fail decoding.
The handler performs no fleet lookup, seat scan, child command, database access,
or federation-client operation. It clones the current public endpoint and the
FMan-wide retained holder-authorization set, whose retention limit equals the
64-envelope response limit, signs the canonical response, and validates its
size, URL, subject, freshness, and signature bounds before returning. Work is
therefore bounded by the fixed authorization and response caps rather than by
the number or health of hosted seats.

**L4 (code) — response writing is isolated from handler execution.** Iroh's
bounded handler task owns a global stream permit only through request decoding,
service execution, and typed response serialization. It hands the encoded
response to a separately bounded per-method writer partition before releasing
handler capacity. A slow reader can consume that method's writer capacity, but
cannot retain all handler permits or the writer capacity of an unrelated
unsigned FI verb.

L3 removes the previously recorded trust-material amplification path. L1, L2,
and the transport argument have not been comprehensively re-audited in this
maintenance pass, so the parent claim remains Unverified.

## Residual windows

- `GetAvailability`'s SQLite aggregate and `GetQuote`'s denomination/signing
  work remain uncached; their existing local bounds have not been re-verified
  here.
- The response writer pools are finite. Saturating one unsigned method's writer
  partition can delay that method, while the per-method partition prevents it
  from consuming another unsigned method's writers.
- Trust-material verification still performs work proportional to the retained
  authorization set and serialized response, both bounded by shared constants.

## Weakest links

L1 and L2 retain manual cost inventories for availability and quote preparation.
L3 relies on the shared authorization-retention and response-size constants
remaining aligned with the producer, while L4 relies on the transport's
per-method writer partitions remaining distinct. This maintenance reran the
former source-level seat-fanout counterexample against the new call graph, not a
complete load or scheduler verification of all three verbs.
