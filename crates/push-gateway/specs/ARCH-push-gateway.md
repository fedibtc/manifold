# ARCH-push-gateway: webhook-to-mobile-push gateway

The push gateway lets mobile app users create shareable HTTPS hook URLs that
external callers invoke to trigger mobile push notifications. It is auxiliary
callback transport used by the FI federation-formation flow, but it owns no
authoritative formation state, progress, or success decision. Its durable state
is limited to registrations, hook capabilities, accepted notification events,
and delivery work; the app must reconcile authoritative formation state.

Notification delivery is non-load-bearing: the gateway provides bounded
best-effort provider submission with provider acceptance, permanent invalid-token
handling, or actionable dead letter, not device or application receipt. Delayed
or dropped device delivery is expected and must not change the target's
provider-resolution outcome
([SPEC-hook-invocation](./SPEC-hook-invocation.md);
[CLAIM-production-ready](../../../specs/CLAIM-production-ready.md)).

## Crate boundaries

The gateway is split into layered workspace crates so reusable service-local
contracts and external boundaries are explicit:

- `fedi-decentralized-push-gateway-types` owns serde DTOs, opaque id/value
  types, hook secret generation/hash helpers, other opaque URL-safe token/id
  generation helpers, and notification/registration record shapes. It must
  not depend on HTTP, SQL, async runtime, CLI, or provider client crates.
- `fedi-decentralized-push-gateway-storage` owns durable database state:
  `Database`, repositories, delivery outbox state, and migrations. It
  depends on the types crate, SQLx, and serde for sanitized admin/metrics
  read-model serialization. Storage records sanitized delivery failures
  through its own `DeliveryOutboxFailure` input instead of depending on
  push-provider error types.
- `fedi-decentralized-push-gateway-provider` owns the provider trait,
  sanitized provider error classification, and lightweight no-op/fake
  providers.
- `fedi-decentralized-push-gateway-provider-fcm` owns the Google/FCM
  external boundary: credentials, provider configuration, OAuth/FCM HTTP
  calls, response classification, and transport-agnostic outbound FCM
  payload validation. The server may use provider-fcm's reserved-key helper
  as an early create-hook prefilter, but final outbound validation remains
  owned by provider-fcm.
- `fedi-decentralized-push-gateway` is the Axum server/binary crate. It owns
  routing, handlers, Nostr auth extractors/replay cache, CLI/config parsing,
  in-process rate limits, observability, worker startup, provider
  selection, and translation between HTTP/provider/storage errors.

## Surfaces

Three request classes share the server:

- public hook invocation (`POST /hooks/{hook_id}/{hook_secret}`), authorized
  by possession of the bearer-capability URL
  ([SPEC-hook-invocation](./SPEC-hook-invocation.md));
- management, registration, and legacy direct notification endpoints,
  authenticated by Nostr NIP-98-style signed requests
  ([SPEC-recipient-auth](./SPEC-recipient-auth.md));
- operator diagnostics (`/ready`, `/metrics`), partitioned from public
  traffic as described below.

Production registration admission remains fail-closed at configuration time.
Operators must choose exactly one of a restrictive emergency recipient allowlist or the FI
MVP's explicit open-self-registration mode. Open mode accepts arbitrary NIP-98
signers only after source/recipient/global capacity checks and provider
validation; it is not device or FI attestation. The FCM provider uses HTTP v1
`validate_only` to check project/token acceptance without notification delivery.
Registration attestation is not part of the current wire contract.

Hooks in every mode bind to one active installation and expire. This is
the initiating-installation boundary used by FI formation callbacks. Registration
freshness also expires unless refreshed by another signed registration request.
Storage-owned admission transactions use portable database mutex rows, perform
bounded stale/terminal reclamation, and enforce physical-row high-watermarks in
addition to active-resource caps; startup GC is supplementary, not the only
reclamation path. Registration TTL cleanup removes orphaned token-owner rows at
the same cutoff, so normal expiry cannot consume physical capacity forever.
While live, the owner row binds the token to its stable installation id. Valid
signed registrations for the exact token/installation pair use latest-serialized-
registration-wins ownership across app-root recipients; another live clone can
take it back later. A different installation remains refused until rotation or
unregister.

Storage also owns compact accepted hook-idempotency markers. They retain only the
hook/key pair, prior target count, and lifecycle timestamps after sensitive
event/outbox cleanup, survive through the hook lifetime plus a seven-day cleanup
margin while the hook remains usable. Revocation/expiry rejects replay, so
terminal hook GC can cascade markers once retained events are gone. Their table has an independent global
ceiling tied to the configured physical hook-row ceiling; bounded reclamation and
the capacity check occur in the invocation transaction before hook state changes.

Storage is SQLx `AnyPool` over SQLite (local/dev/CI default, with
local-process pragmas for WAL, normal synchronous mode, busy timeout, and
foreign-key enforcement) or PostgreSQL (production-oriented), selected from
`PUSH_GATEWAY_DATABASE_URL` (`sqlite:`, `postgres:`, `postgresql:`).
Repository code uses one shared SQL path with portable `$N` bind parameters,
and migrations must stay portable across both backends: prefer common types
such as `TEXT` and `BIGINT`, common indexes and foreign keys, and portable
`ON CONFLICT` forms; no SQLite-only syntax such as `INSERT OR IGNORE` and no
PostgreSQL-only DDL unless the code branches by backend and tests cover both
paths. Exactly one gateway process, including its outbox worker, may be
active per database. The gateway does not support HA or multiple replicas:
Nostr replay tracking, idempotency locking, fixed-window rate limits, and
interrupted-outbox recovery have no cross-process coordination.
Within that process, hook create/revoke, registration upsert/delete/disable, and
hook-invocation acceptance writes share one database-write mutex with outbox-worker
claim, expiry, or completion writes. The clone-shared `Database` handle owns the
coordinator, so every `AppState` built from one database handle shares it. Requests
admit at most 64 waiting or active mutations; excess requests return `503` rather
than accumulating unbounded waiters. Workers bypass request admission. Once one
queues, the write-preferring mutation lock prevents later requests from passing
it, so only a request that already crossed request serialization can precede it.
Provider calls occur outside the coordinator and retain their configured
concurrency. This prevents deferred SQLite transactions
from racing during read-to-write upgrades while preserving the portable SQL
transaction boundaries used by PostgreSQL. SQLite bounds engine lock waiting with
its five-second `busy_timeout`; PostgreSQL pools set a five-second statement timeout
on each connection. Direct repository/admin use is outside this process coordinator
and must not run concurrently with the active gateway process.

## Worker lifecycle

The binary starts one background outbox worker after database/provider
initialization and before serving HTTP. Startup resets interrupted
`in_progress` rows to `pending`, then the worker claims due rows with
bounded concurrency. Hook invocations retain a notification permit immediately
after committing newly due outbox rows and releasing the database-write mutex. When no
row is immediately claimable, the
worker sleeps until the nearest known `next_attempt_at` retry/reclaim
deadline, capped by a bounded fallback poll used only to recover from missed
in-process notifications, manual database changes, and coarse wall-clock
deadlines. Transient errors are retried with bounded backoff and eventually
dead-lettered. Each provider call is bounded and each accepted target has an
absolute five-minute deadline derived from its durable enqueue timestamp;
restart cannot extend it. See
[`SPEC-durable-state-lifecycle`](./SPEC-durable-state-lifecycle.md) for the
inventory and terminal contract. Shutdown uses retained cancellation state so the signal
cannot be missed; the worker stops claiming new rows and drains currently
running provider calls before returning.

## Observability and listener partition

`AppState` owns shared in-process `Observability` counters and worker state.
Router middleware generates a gateway-owned request id for each HTTP
response, records status-class counters, and writes one sanitized structured
log line with the HTTP method, route template, status, latency, and request
id. It does not trust client-supplied request ids (public callers could
place bearer material there) and does not log raw URLs, query strings, hook
ids, hook secrets, recipient ids, FCM tokens, or credential material.
Hook ids, hook secrets, full hook URLs, FCM tokens, credentials, recipient
ids, and registration tokens are never metric labels or log fields.

The public listener always carries hook/API routes plus `/health` and
`/live`; `/live` may remain unauthenticated because it only reports process
liveness. `/ready` (database readiness, provider mode selected at startup,
worker running state and concurrency, outbox status counts) and `/metrics`
(low-cardinality Prometheus text: request/status classes, provider mode,
worker state, delivery outcomes by sanitized reason class, rate-limit
rejections, invalid-token cleanup failures, queue depth, oldest
due/pending/retrying ages, dead-letter current/total, claim query counts)
are not on the public listener by default. Operator deployments should
prefer a separate `PUSH_GATEWAY_OPERATOR_BIND` listener, optionally
protected by `PUSH_GATEWAY_OPERATOR_TOKEN`; if only the token is configured,
`/ready` and `/metrics` mount on the public listener token-protected.
`PUSH_GATEWAY_PUBLIC_METRICS_ENABLED=true` is the explicit opt-in for
unauthenticated public `/metrics`. If the database-backed metrics query
fails, the scrape returns an explicit HTTP 500 and scrape-error gauge
instead of reporting zero outbox rows. In-process counters reset on restart;
the database remains the durable source of truth.

Gateway `/metrics` is operator telemetry for this service and its outbox only.
It is deliberately distinct from FI remote guardian telemetry and never exposes
or proxies guardian/fedimintd Prometheus endpoint responses.

The `outbox` admin CLI is an operator-only database tool: it connects by
database URL without running migrations, lists/counts dead-letter rows using
sanitized metadata, and never includes FCM tokens or serialized notification
content in default output. Replay/delete operations are bounded, require
explicit confirmation, and must run only while the gateway service is
stopped.

Generated hook invocation URLs are bearer capabilities. Production
configuration must use a validated absolute `https://` public origin;
local/test code must use the explicitly named insecure public-base-url
escape hatch for empty or loopback HTTP origins. The create-hook response is
the only intentional plaintext hook-token exposure and is marked
no-store/no-cache.

Operational procedure, configuration reference, deployment modes, and
playbooks are in [`OPERATIONS.md`](../OPERATIONS.md); the endpoint/API
reference is [`README.md`](../README.md); security rules are
[`SECURITY.md`](../SECURITY.md); the testing strategy is
[`testing.md`](../testing.md).
