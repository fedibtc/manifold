# SPEC-abuse-limits: rate limits, caps, and source identity

## Record justification

The abuse-control contract spans handler checks, config validation,
in-process limiter state, and operator deployment expectations, so no single
implementation artifact can own it coherently.

## In-memory fixed-window limits

Because the gateway uses the single-process topology in
[`ARCH-push-gateway`](./ARCH-push-gateway.md), production abuse limits live in
process memory: counters are fixed-window and reset on restart, and must never
be described as distributed or HA-safe without database/external coordination.
Limiter state is owned by `AppState`.
Each limiter family has an independent key space and capacity, prunes expired
windows, and never evicts a live window to admit a new key. A saturated family
continues enforcing existing keys and fails closed for new keys; saturation in
one family cannot reset another.

## Hook rate-limit default and override boundary

New hooks default to a production-conservative fixed-window policy of 2
accepted invocations per 3600-second window; omitting
`policy.rate_limit.window_seconds` or `.max_requests` uses this policy
rather than disabling limiting. Tests and local demos that need faster
repetition must set an explicit per-hook policy.

The management API accepts explicit per-hook policies in the broad validated
ranges 1–86,400 seconds and 1–10,000 requests for local/dev ergonomics.
Production mode has no privileged high-rate exception flow: hook creation
rejects policies more permissive than the 2-per-3600-seconds default, and
hook invocation applies the same conservative effective cap to any permissive
persisted hook row.

## Caps and backlog pressure

Storage-owned admission transactions take a durable database admission mutex,
reclaim a bounded batch, and atomically enforce active and physical-row
high-watermark ceilings. The process-wide database-write coordinator remains
the single-process serialization boundary shared with worker mutations, but
handlers do not define durable eligibility or cap SQL. Per-recipient and global
active-resource caps, global physical hook/registration row ceilings, and a
nonzero admission-GC batch are mandatory in production. Global admission or
physical-row exhaustion returns `503`; recipient exhaustion returns `429`.
If bounded reclamation cannot restore headroom, admission fails closed. Backlog
caps are checked in the hook-invocation transaction before outbox rows are
inserted: global backlog pressure returns `503`, recipient-scoped pressure
returns `429`.

Compact accepted hook-idempotency markers have their own global row count,
bounded expired-row reclamation, and fail-closed admission check in the same
invocation transaction. The ceiling uses the configured physical hook-row cap;
exhaustion returns `503 idempotency_capacity_exceeded` before hook counters,
events, or outbox rows change. Marker retention extends through each hook's
lifetime plus a seven-day cleanup margin, so operators must size and monitor the
database for the accepted keyed invocation rate rather than relying on the
shorter sensitive event/outbox retention horizon.

Registration writes first consume the low-cardinality source-only boundary and
only then allocate a recipient-plus-source key. Recipient-plus-source prevents
one signer from churning its rows, while source-only limiting prevents an origin
from evading the first limit by generating new Nostr keys. FCM validation occurs
only after these in-memory source checks, so rejected origins cannot consume an
unbounded number of Google API requests. Provider validation uncertainty fails
closed and does not persist a registration.

Every otherwise-valid NIP-98 management event consumes a trusted-proxy-aware
source-prefix budget after signature/tag/timestamp/admission validation but
before insertion into the global replay cache. This prevents one open-mode
signer from monopolizing the cache; a throttled source consumes no replay
capacity and a distinct source remains serviceable.

Production registration admission is fail-closed unless the operator chooses an
emergency static recipient allowlist or explicitly enables open authenticated
self-registration. Production rejects neither-selected and both-selected
configurations: the modes are XOR. Open mode is required for the FI pre-quote
flow, but NIP-98 key control and FCM project membership are not an FI-attestation
mechanism. The registration wire contract carries no attestation field.

Registration eligibility expires after a configured signed-refresh horizon (30
days by default). Stale rows do not count as active delivery targets. Bounded
on-admission GC removes stale registrations, their orphaned owner rows, and
terminal unreferenced hooks;
startup GC also sweeps them. Token moves are atomic and use separate
recipient/global active deltas plus their physical delta at saturation. Durable
token-owner rows bind live tokens to stable installation ids and become stale-GC
candidates once their registration is gone and their own refresh timestamp is
older than the same cutoff. Both owner and
registration rows count toward the registration physical-row high-watermark.
Every hook must have a finite TTL and target an active installation owned by the
authenticated recipient.

## Source identity

Public caller source identity is the direct socket peer IP unless that peer
is inside configured trusted-proxy CIDRs; only then may `X-Forwarded-For` or
RFC `Forwarded` influence it, and the gateway selects the rightmost
untrusted forwarded entry so a client-prepended spoofed address is not
accepted in common append-style configurations. Trusted proxies are expected
to strip/overwrite inbound forwarding headers. Rate-limit source keys
normalize IPv4 to the address and IPv6 to the `/64` prefix.
