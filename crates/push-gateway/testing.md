# Push gateway testing strategy

Default tests must not contact Google FCM or require real Firebase
credentials: fake/no-op delivery for ordinary hook tests, local fake HTTP
servers for FCM provider contract tests. Most HTTP, persistence,
security-negative, and abuse-control coverage lives in handler-level and
repository-level tests over isolated temporary SQLite databases, so hook
secrets, TTL/revocation/max-use checks, rate limits, registration lifecycle,
and sanitized error envelopes are exercised deterministically without real
credentials, tokens, or network exposure. Tests that need more than the
production default of 2 accepted hook invocations per hour
([SPEC-abuse-limits](./specs/SPEC-abuse-limits.md)) must set explicit
per-hook test limits.

## Test layers

- Unit tests cover validation, config parsing, and secret redaction.
- Observability tests cover request-id propagation, readiness diagnostics,
  metrics shape, explicit `/metrics` database scrape failures, operational
  outbox gauges/counters, and absence of hook secrets/full hook URLs in
  operator surfaces.
- Router tests cover the public/operator partition: public `/ready` and
  `/metrics` absent by default, public `/metrics` requiring explicit opt-in,
  operator-listener configuration not leaking operator endpoints onto the
  public router, and operator bearer tokens rejecting missing/wrong tokens.
- Repository/handler tests use temporary SQLite databases and the real
  migrations. A populated upgrade test seeds registrations, safely narrowable
  and ambiguous legacy hooks, and accepted keyed events; it verifies owner and
  tombstone backfills plus ambiguous-hook invalidation. SQL must stay portable
  to PostgreSQL, and backend-specific behavior needs explicit tests for both
  backends or a documented manual/integration check.
- Hook-flow tests use `FakePushProvider` or no-op delivery; handler tests
  that need provider results start the outbox worker and poll persisted
  status rather than depending on synchronous delivery. They must cover
  app-owned mobile context: creation with workflow/action/deep-link/open
  behavior/privacy fields, invocation producing the expected provider and
  FCM data payload context, and rejection of reserved caller `data` keys.
- Management/registration tests sign Nostr/NIP-98 auth events with generated
  keys and cover method, URL/query, raw-body payload hash, timestamp,
  replay, recipient-spoof rejection, source-only limits across rotated keys,
  pre-replay source throttling with a distinct source still serviceable,
  partitioned/non-evicting limiter saturation, every mandatory production cap
  at zero, active and physical row exhaustion, bounded no-restart reclamation,
  count-neutral same-recipient token moves, same-installation account handoff,
  new-recipient capacity accounting, different-installation token-theft refusal
  even after stale-row GC, signed unregister release of the durable owner row,
  rejection of undeclared attestation fields, and installation-scoped hook ownership across
  absent, cross-owner, disabled, stale, and refreshed targets. Invocation tests
  require missing/disabled/stale targets to return retryable 503 with no
  counter/event mutation, then accept the same idempotency key exactly once
  after refresh. They also prove a 30-day formation hook remains idempotent after
  sensitive event/outbox retention cleanup, expired markers are reclaimed, and
  marker-cap exhaustion fails closed before hook state changes. Public hook invocation stays bearer
  hook-token URL auth.
- FCM provider tests use a local Axum fake server for OAuth token exchange
  and FCM HTTP v1 send responses, covering OAuth token caching, request
  shape, `validate_only: true` registration checks that never include a visible
  notification, pre-persistence invalid-token rejection, ambiguous/transient
  validation returning 503 with zero persistence, successful send,
  invalid/unregistered-token cleanup, token-scoped
  FCM `INVALID_ARGUMENT` cleanup, generic bad-payload `INVALID_ARGUMENT`
  dead-lettering without registration disablement, local payload validation
  before OAuth/send, transient classification, timeout/network handling
  where practical, and credential/token redaction.
- The ignored PostgreSQL migration smoke test runs by setting
  `PUSH_GATEWAY_POSTGRES_TEST_URL` to a disposable database and running
  ignored tests explicitly; that is the only PostgreSQL test mechanism —
  default CI runs SQLite only.
- `defe` e2e keeps fake/no-op delivery, stays independent of Firebase
  credentials, and exercises the real managed-resource binary through
  registration, hook creation, hook invocation, and persisted outbox
  completion. The defe test is an ignored smoke/e2e test because it depends
  on an external local `defe` server.
- The Nix-generated OCI image is a Linux runtime artifact with a mandatory
  Linux CI contract check (`.#ci.<system>.pushGatewayOciImage`, run
  unconditionally by `selfci` on Linux): it builds the image without Docker
  and checks the image config contract — binary entrypoint, non-root
  `65534:65534` user, CA-certificate environment, persistent/secrets
  volumes, Linux OS, `SIGTERM` stop signal, working directory, exposed
  ports, OCI labels, and no baked secret-like environment variables. Keep it
  updated with intentional image contract changes.
- Any real FCM smoke test must be ignored/manual, gated by explicit sandbox
  credentials, and excluded from default CI.

## Durable outbox tests

Outbox/reliability tests cover durable enqueue before delivery, startup
recovery of pending rows, transient retry/backoff, retry exhaustion or
permanent payload failure to `dead_letter`, transactional invalid-token
registration cleanup fenced by the outbox claim and token snapshot, partial
failure across multiple target registrations, and per-hook
`idempotency_key` idempotency. They also cover the durable-state schema
inventory, terminal-data retention boundary, five-minute resolution deadline,
15-second provider-call cap, and shorter remaining-deadline cap. They cover the
notification/deadline-driven worker scheduler: idle workers must not
continuously issue claim queries while no row is due, committed invocations
wake the worker, and future `next_attempt_at` deadlines are honored without
an explicit notify; tests that manually edit retry deadlines notify the
worker unless intentionally exercising fallback behavior. Temporary SQLite
databases and fake/scripted providers only. Hook create/revoke, registration
upsert/delete/disable, invocation acceptance, and worker database writes must
remain synchronized by the clone-shared database-write coordinator. Tests use its
test-only boundary observer rather than timing delays, verify two `AppState`
instances built from one database share the coordinator, bound request admission
and cancellation, prevent later requests from passing a queued worker, and show
production hook acceptance wakes an idle worker well before an injected
sixty-second fallback poll.

Installation-target tests register two devices under one recipient, create a
hook for the initiating device, invoke it, and assert that exactly one outbox
delivery targets that installation. Registration TTL/GC tests use bounded
configured horizons and startup reconnects; they must not wait on wall-clock
month-scale defaults.

Outbox observability tests keep labels low-cardinality and secret-free:
provider reason classes, rate-limit rejection counters, retry/dead-letter
gauges, invalid-token cleanup failure counters where practical, and the
fail-closed `/metrics` behavior for database errors so broken scrapes cannot
be confused with an empty queue.

## Outbox admin CLI tests

Admin tooling tests cover parser behavior, bounded selector validation,
confirmation/dry-run behavior, and sanitized output. Repository-level tests
use temporary SQLite databases to verify dead-letter listing/reason
aggregation, replay back to `pending`, and permanent delete paths without
returning `fcm_token` or `notification_json`. CLI/parser tests assert the
`outbox` subcommands use database-only configuration, reject server-only
parser misuse, require bounded selectors for mutations, and escape control
characters in human-readable output.
