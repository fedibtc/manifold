# push gateway security notes

The push gateway crate is currently a local development/test gateway by default.
It should bind to loopback in `defe` and use isolated SQLite databases per test
resource. PostgreSQL is supported as the production-oriented storage backend, but
the current hook registry and default fake/no-op delivery mode is still
local/dev-safe only and must not be treated as production-ready public
infrastructure. Real FCM delivery exists but is opt-in by configuration.

Do not use real Firebase credentials or production push tokens in tests. Default
CI and `defe` must use fake/no-op delivery or local fake HTTP servers, never
Google FCM. The defe-managed resource app id and database are local test data
only. Firebase credentials, FCM registration tokens, full hook URLs, hook secrets, and provider credential material must be redacted from `Debug`, logs,
traces, client errors, and metrics. The only intentional raw hook secret exposure
is the one-time create-hook response to the temporary management caller. That
response is marked `Cache-Control: no-store` and `Pragma: no-cache`.

Application request logs must use sanitized route templates and low-cardinality
fields only. Do not log raw request URLs, query strings, hook path tokens, FCM
tokens, credential JSON, OAuth access tokens, recipient identifiers, or
registration tokens in metrics labels. The request-id middleware generates
gateway-owned `x-request-id` values and must not log or propagate client-supplied
request ids, because public hook callers could otherwise place bearer material in
that header.

## Production-target trust boundaries

The intended production model includes user-created HTTPS hook URLs that
external callers invoke to trigger mobile push notifications. Those hook callers
are untrusted or semi-trusted, even if a user intentionally shared a hook URL
with them.

Hook URLs are bearer capabilities: possession of the full URL authorizes
invocation until the hook expires or is revoked/deleted. Revocation and expiry
also reject retained idempotency-key replays. URL tokens must be
unguessable, stored only as hashes at rest, and redacted from logs, errors,
metrics, traces, and tests. Do not log raw hook URLs. Public hook URLs generated
from CLI/env configuration must use an absolute `https://` origin with no
userinfo, path, query, or fragment; the insecure public-base-url escape hatch is
for local loopback tests only.

Before hook handling is exposed beyond local development, production code must
include payload validation, request size limits, rate limiting, TTL and
revocation checks, and abuse controls.

Readiness and metrics are operator surfaces. `/ready` reveals dependency and
queue state, and `/metrics` reveals process/queue behavior. They must stay on a
trusted operator listener (`PUSH_GATEWAY_OPERATOR_BIND`), require the configured
operator bearer token (`PUSH_GATEWAY_OPERATOR_TOKEN`), or both. Do not expose
unauthenticated public `/metrics` with `PUSH_GATEWAY_PUBLIC_METRICS_ENABLED=true`
unless an equivalent trusted network or reverse-proxy policy protects it.

Guardian telemetry adds a second sensitive capability boundary. The public
registration route is not authorized by possession of the URL: it requires a
fresh exact-body NIP-98 proof and accepts state only after complete Holder
authorization verification. The NIP-98 signer must equal the verified Holder
subject, binding the exact submitted FMan endpoint and capability to that
credentialed identity. Never weaken this to signature-only admission.
“Live Holder authorization” includes complete authority, revocation,
credential, holder, subject, schema, issuer-policy, and minimum-trust checks.
Telemetry startup fails closed when that policy cannot be constructed.

The FMan-wide capability, Holder authorization envelope, encryption key, Iroh
locator, FMan and seat identifiers, invites, and raw Prometheus bodies are
sensitive. They must not appear in logs, errors, traces, debug output, or metric
labels. The database stores the endpoint and capability only as AES-256-GCM
ciphertext with a random nonce and FMan-specific associated data; the
deployment key is separate and backed up outside the database.

The seat-list and raw collector endpoints must remain on the protected operator
router. Neither is a public discovery or Prometheus API.
It mirrors source bytes and status without filtering, so its operator audience
is trusted to see the complete pinned guardian metric surface. Reverse proxies
must not cache or log collector response bodies. The current FMan capability authorizes all its seats and explicitly shareable
journals and is compared in constant time before resource selection.

## Current hook MVP controls

The current implementation stores hook secrets only as hashes, redacts hook
token, FCM token, and Firebase credential debug output, returns stable
sanitized JSON error envelopes to HTTP clients, limits request bodies, validates
small registration/hook/invocation fields, and enforces hook TTL, revocation, and
optional max-use counts. New hooks also have a per-hook fixed-window rate limit
by default, currently 2 accepted invocations per hour unless the temporary
management caller supplies a different validated policy at creation time. In
production mode, hook creation rejects policies that are more permissive than 2
accepted invocations per 3600-second window; there is no privileged per-hook
high-rate exception flow.

Registration storage tracks `last_seen_at`, `disabled_at`, and
`disabled_reason`; recipient lookup returns only active, non-disabled, non-stale
tokens, and
authenticated recipients can unregister/delete or disable their own installation.
These endpoints derive recipient identity from Nostr auth and do not trust
caller-supplied `recipient_id` or `app_id` fields.

Hook consumption must keep the token, TTL, revocation, max-use, and per-hook
rate-limit checks in the same database state transition that increments
`use_count`. Future changes to this path must preserve concurrent max-use and
rate-limit tests so racing invocations cannot over-consume constrained hooks.

Mobile app-open context is app-owned hook metadata, not untrusted caller input.
External hook invocations must not be allowed to inject or override deep links,
workflow/action routing, app-open behavior, notification kind, recipient,
notification id, visible title/body text, or gateway-reserved `pg.*` data keys. Create-hook and
invoke-hook validation reject these reserved names and FCM-reserved keys
(`from`, `message_type`, `google*`, and `gcm.*`) in free-form `data`; routing
context must be supplied through the typed hook management fields and validated
there, and notification identity must be gateway-generated rather than public
caller-supplied. The final FCM data payload is also
size-checked after gateway-added keys. Prefer `privacy: "data_only"` hooks for
sensitive workflows where push title/body should be omitted and the app should
fetch details after opening.

Persisted JSON corruption must fail closed without exposing stored payloads.
Corrupted hook `data_json` returns the stable sanitized `internal_error` response
and logs only low-cardinality decode context. Corrupted outbox
`notification_json` is not replaced with `{}` or a fallback notification; the
claimed row is marked `dead_letter`, the provider is not called, and only
sanitized reason codes/counters/log fields are emitted.

Management endpoints require Nostr/NIP-98 signed requests. The trust boundary is
the signing key: the gateway verifies only the signature, method, configured-base
URL plus path/query, fresh timestamp, body payload hash, and event-id replay; it
does not derive private keys. The effective recipient is the canonical lowercase
hex Nostr public key from the event. Caller-supplied recipient strings, `npub`,
NIP-21, and uppercase encodings are rejected as identity proof. Production
admission is an exclusive explicit choice: an emergency static allowlist may be configured
with `PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS`, or FI deployments may set
`PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED=true` to accept any valid NIP-98
signer. Both options default off in production, and startup rejects enabling
both because an emergency restriction must not be silently bypassed by open
mode. Open admission proves only
control of a fresh signing key and a token accepted by Fedi's configured
Firebase project; it does not prove that the signer is an FI, that the device is
uncompromised, or that the registration is benign. Every otherwise-valid
management auth event consumes a trusted-proxy-aware source budget before it
can enter the replay cache. Per-source (including across
rotating recipient keys), per-recipient/source, recipient, global, and outbox
ceilings therefore remain mandatory production controls. Limiter families have
independent bounded maps, never evict live windows for new keys, and fail closed
on family saturation. Rate limiting and replay protection remain in-memory and
single-process only. Accepted auth event ids are retained for the full timestamp
freshness window in a bounded 4096-entry cache; if the cache is full of
still-fresh ids, authorization fails closed
instead of evicting replay protection. The cache is not persisted, resets on
restart, and is not shared between gateway processes. Source-based limits use
the socket peer IP unless that direct peer is in configured trusted-proxy CIDRs;
only then are `X-Forwarded-For`/`Forwarded` honored. There is no distributed
multi-replica rate-limit or replay backend. The durable
delivery outbox is scoped to the same
single-active-process deployment policy: exactly one active gateway process,
including exactly one active outbox worker, may run against the configured
database. The default delivery provider is no-op/fake; real FCM must be
explicitly enabled with
`PUSH_GATEWAY_PROVIDER=fcm` and credentials supplied through
`FCM_SERVICE_ACCOUNT_FILE` or `FCM_SERVICE_ACCOUNT_JSON`. Production
deployments should enable `PUSH_GATEWAY_PRODUCTION_MODE=true`; this rejects
no-op delivery, the default/non-HTTPS public base URL, admission with neither an
allowlist nor explicit open-registration opt-in, disabled source, recipient,
global, or backlog caps, the legacy ack-without-delivery
`/hooks/notification` endpoint, FCM
send-endpoint overrides, and non-default OAuth token URIs at startup/config-load
time.

## FCM provider controls

The FCM provider parses Firebase service-account JSON into typed fields and
redacts credential material in `Debug` output and sanitized errors. Prefer
file/secret-manager injection through `FCM_SERVICE_ACCOUNT_FILE`; environment
JSON is supported for deployment systems that expose secrets that way but should
not be logged, committed, or copied into issue trackers.

The command-line help output must not print runtime environment values because
configuration can contain database URLs, internal origins, or raw Firebase
service-account JSON. Prefer file-based credentials over
`--fcm-service-account-json`; raw JSON passed on a command line can be exposed by
process listings, shell history, and service-manager diagnostics.

FCM OAuth access tokens are cached in memory and used only as bearer
authorization for FCM HTTP v1 requests. Treat them as bearer credentials:
provider and token-cache debug/log output must redact them just like service
account material. Invalid or unregistered-token provider responses atomically
mark the outbox row invalid-token and disable only the affected registration when
the current token still matches the outbox row token snapshot. Generic
bad-payload `INVALID_ARGUMENT` responses must not disable registrations. Quota,
network, auth, timeout, generic provider 404, and provider errors are treated as
transient delivery failures and must not delete or disable registrations; they
are retried through the durable outbox and may eventually become dead-letter
rows.

Before a registration is persisted, FCM mode submits a complete data-only HTTP
v1 message with `validate_only: true`. Google validates the request/token against
the configured Fedi project but does not deliver it. A token-specific rejection
returns `422`; provider/auth/network ambiguity returns `503` and is never treated
as successful admission. The test/no-op providers accept validation only for local development.
Registration attestation is not part of the current wire contract.

Hook creation in every mode requires an active owned `installation_id` and a finite
TTL. Invocation may enqueue only that installation, which prevents a formation
callback from notifying every installation that shares the recipient key.
Registration freshness is also finite (30 days by default); signed refreshes
advance `last_seen_at`, stale rows stop receiving new notifications, and bounded
admission GC plus startup GC remove them. Admission transactions enforce
physical-row ceilings and fail closed when bounded GC cannot restore headroom.
Token moves use their recipient/global net row deltas. A durable owner row binds
a live token to its stable installation id. Stale GC removes an orphaned owner
after its registration disappears and the same refresh cutoff passes, preventing
normal expiry from exhausting the physical-row ceiling.
Fedi retains that pair across account switches. Every valid signed registration
for the exact pair is authoritative: serialized registration commits use
latest-valid-wins ownership and atomically update both durable rows. Another
still-valid clone may later register the pair and take ownership back, so a set
of active clones can oscillate until only one continues. Presenting the token
under a different installation id is refused; a signed rotation or unregister
remains its release path. Both registration and owner rows count toward the
physical high-watermark.
An account switch changes future target resolution only. An outbox row accepted
before the switch already contains the old recipient's token and notification
snapshot, so it may still reach the same physical installation within the
five-minute delivery-resolution window.
Missing, disabled, or stale invocation targets return retryable 503 before hook
counters or events are mutated. Callback payloads remain generic, non-sensitive, and
non-authoritative: receipt is only a reason for the app to reopen and reconcile
durable formation state.

Caller-provided idempotency keys are secret-adjacent correlation material and
must not appear in logs or metrics. A compact accepted-key marker is retained
after sensitive event/outbox cleanup through the hook's finite lifetime plus a
seven-day cleanup margin while the hook remains usable. Revocation/expiry rejects
replay, so terminal hook GC removes now-useless markers after retained events are
gone. A marker contains no notification JSON or push token, but the table, database
WAL, backups, and restored copies still require sensitive operational handling.
Marker admission is globally capped and fails closed before hook counters or
delivery state are mutated when bounded reclamation cannot restore headroom.

Provider acceptance is ordinary best-effort transport, not device receipt, app
handling, or user display. The worker caps every provider future at 15 seconds
and the remaining durable five-minute resolution deadline; overdue active rows
become `resolution_deadline_exceeded`. Durable `created_at` fixes that deadline
across restart, and `claim_id` fences stale retry/completion against expiry.
Process or database unavailability can delay persisting the terminal state and
belongs to recovery, never successful delivery. Changes to cancellation, replay,
clocks, status transitions, or multi-worker coordination must re-check the
safeguards and tests in
[`SPEC-durable-state-lifecycle`](./specs/SPEC-durable-state-lifecycle.md).

SQLite is configured with WAL, foreign-key enforcement, a busy timeout, and a
bounded connection pool for the current local/dev service. Hook create/revoke,
registration upsert/delete/disable, and invocation acceptance share the
clone-shared database handle's process-wide coordinator with every outbox-worker
mutation; external provider calls remain outside it. The coordinator admits at
most 64 waiting or active request mutations, returning `503
database_write_queue_full` on saturation. Worker mutations bypass request
admission. Once a worker queues, the write-preferring mutation lock prevents
later request mutations from passing it; only a request that had already crossed
request serialization can precede recovery, expiry, claim, or completion. SQLite
bounds engine lock contention with its
five-second `busy_timeout`; PostgreSQL pools apply a five-second statement
timeout per connection so a stalled statement cannot hold the coordinator
indefinitely. These bounds do not provide multi-process/multi-replica
coordination, and direct repository/admin mutations must not run beside the
active gateway process. PostgreSQL is the
production-oriented storage backend, but this is not yet a multi-replica
production coordination policy or HA design. Outbox rows contain FCM token
snapshots and serialized notification content, and idempotency markers contain
caller correlation keys, so the configured database and all
backups/restored copies must be protected as sensitive operational data.

The `outbox` admin CLI is a secret-adjacent operator surface because it reads the
same sensitive database. Default human and JSON output must not include FCM
tokens, `notification_json`, Firebase credentials, full hook URLs, hook secrets,
or raw provider payloads. Human-readable output must escape DB-controlled strings
so control characters cannot forge terminal/log rows. Replay/delete commands must
remain bounded and explicitly confirmed, and operators must run mutating admin
commands only with the gateway service stopped.

## Container image controls

The Nix-generated production OCI image is intentionally minimal: it contains the
gateway binary, CA certificates for FCM HTTPS, and minimal NSS files. It runs as
the non-root uid/gid `65534:65534`, uses the gateway binary as its entrypoint,
and bakes no FCM credentials, app ids, operator tokens, database passwords, or
other secrets into the image. Supply deployment-specific configuration through
runtime environment variables and mount secrets at runtime, preferably as
read-only files under `/run/secrets`.

Container deployments must ensure persistent SQLite/database directories are
writable by uid/gid `65534` and mounted FCM service-account files are readable by
that user or group. PostgreSQL credentials, operator bearer tokens, and FCM
credential material remain secrets even when they are supplied by an orchestrator
rather than baked into the image.

The image declares `SIGTERM` as its stop signal. Process shutdown handling must
continue to route SIGTERM through Axum graceful shutdown and
`worker.shutdown().await` so `docker stop`, Kubernetes termination, and
systemd-style supervisors do not bypass delivery-worker cleanup.
