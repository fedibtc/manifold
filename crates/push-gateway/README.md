# Push gateway

HTTP push notification hook gateway.

This crate is still a local/development-safe service slice by default. The hook
registry and fake delivery flow described below are not a production exposure
claim: management authentication is Nostr/NIP-98 signed-request based, rate
limiting and replay protection are single-active-process memory state,
production multi-process / multi-replica operation is not supported, and real
FCM delivery is opt-in only.

## Production usage

Management, registration, and legacy direct notification endpoints use `Authorization: Nostr <base64(event)>`, where the event is a NIP-98-style kind `27235` HTTP auth event. The gateway verifies the event signature, exact method, exact URL built from `PUSH_GATEWAY_PUBLIC_BASE_URL` plus request path/query, fresh timestamp, body `payload` hash for body methods, and one-time event-id replay cache. The authenticated `recipient_id` is always the canonical lowercase hex Nostr public key from the signed event; request body/path/query `recipient_id`, `npub`, NIP-21, or uppercase encodings are not identity proof. Client recipient keys are derived by the app from its root-secret model using label `fedi-push-gateway/recipient-auth-nostr/v1` and the same environment separation rule as `docs/fi-nostr-backups.md`. Key rotation/multi-device recovery are handled by that derivation model; server-side in-place recipient-key rotation is not an MVP feature.

See [`OPERATIONS.md`](./OPERATIONS.md) for production configuration, operator
listener/token guidance, readiness/metrics scraping, secret handling,
backup/restore, and operational playbooks.

## Intended production direction

The intended production direction is a webhook-to-mobile-push gateway. Mobile
app users create shareable HTTPS hook URLs and give those URLs to external
callers that should be able to notify them. When an external caller invokes a
hook URL, the gateway resolves the hook to the one app installation selected by
its owner, sends a mobile push notification, and
includes enough context for tapping the notification to open the app at the
relevant workflow or action.

Hook URLs are bearer capabilities: possession of a valid, unexpired,
unrevoked URL authorizes the holder to invoke that hook. Treat the full URL and
its token path segment as sensitive.

## Current state

The crate currently provides an Axum HTTP service with health checks, legacy
notification hook ingestion, persistent app installation registration, a
stateful hook registry, public hook invocation, durable database-backed delivery
outbox/retry worker, fake/no-op push delivery, and an opt-in FCM HTTP v1 provider.
FI formation uses an installation-scoped hook so only the installation that
started formation receives the eventual generic update.

When the complete telemetry configuration is supplied, the same deployment
also exposes a verified FMan telemetry registration endpoint and an
operator-protected live guardian metrics adapter. This path is separate from
mobile push: it stores one verified FMan endpoint and capability encrypted, lets
Fedi discover that FMan's seats, and pulls a selected guardian's byte-preserved
Prometheus response through Iroh. See
[`SPEC-guardian-telemetry-receiver`](./specs/SPEC-guardian-telemetry-receiver.md).

Implementation is split across workspace crates: `push-gateway-types` for
service-local DTO/value types, `push-gateway-storage` for SQLx repositories and
migrations, `push-gateway-provider` for the provider trait/no-op/fake providers,
`push-gateway-provider-fcm` for Google/FCM HTTP/OAuth integration, and this crate
for Axum service orchestration and the binary.

### Architecture

```mermaid
flowchart LR
    subgraph MobileSide[Mobile app / app backend]
        App[Mobile app installation]
        Mgmt[Temporary trusted management caller]
    end

    subgraph Gateway[push-gateway HTTP service]
        API["Axum routes<br/>16 KiB body limit<br/>sanitized errors/logs"]
        Hooks["Hook registry<br/>token hashes, TTL, revocation,<br/>max-use, per-hook rate limit"]
        Regs["Registration store<br/>recipient + installation + FCM token"]
        Events["notification_events<br/>sensitive accepted-event snapshots"]
        Idempotency["hook_idempotency_tombstones<br/>compact accepted-key markers"]
        Outbox["delivery_outbox<br/>per-registration delivery rows"]
        Worker["background delivery worker<br/>claim, retry, dead-letter"]
    end

    subgraph Storage[Configured SQL database]
        DB[(SQLite by default / PostgreSQL)]
    end

    subgraph External[External hook caller]
        Caller[Holder of hook URL]
    end

    subgraph Provider[Push provider]
        Noop[noop/fake provider]
        FCM["FCM HTTP v1<br/>opt-in"]
    end

    App -->|POST /registrations| API
    App -->|DELETE /registrations/{installation_id}| API
    App -->|POST /registrations/{installation_id}/disable| API
    Mgmt -->|POST/GET /v1/hooks| API
    Mgmt -->|DELETE /v1/hooks/{hook_id}| API
    API --> Hooks
    API --> Regs
    Hooks <--> DB
    Regs <--> DB
    Events <--> DB
    Idempotency <--> DB
    Outbox <--> DB
    Caller -->|POST /hooks/{hook_id}/{hook_secret}| API
    API -->|validate bearer capability + policy| Hooks
    API -->|snapshot the hook's active installation| Regs
    API -->|durably enqueue accepted invocation| Events
    API --> Outbox
    Worker -->|claim due rows| Outbox
    Worker -->|deliver| Noop
    Worker -->|deliver when PUSH_GATEWAY_PROVIDER=fcm| FCM
    FCM -->|invalid/unregistered token| Worker
    Worker -->|disable matching token snapshot| Regs
```

For a new public hook invocation, the HTTP handler returns success only after the
hook secret and current policy checks pass and the notification event plus all
target outbox rows are committed. A missing, disabled, or stale target returns
retryable `503 target_installation_unavailable` before hook counters, events, or
the idempotency key are mutated; a signed refresh allows the same key to retry
and enqueue once. Duplicate `idempotency_key` replays for an accepted event still
verify the hook secret, then return the prior target count without re-checking
mutable hook policy or enqueueing new rows. Compact accepted-key markers preserve
that result after sensitive event/outbox rows are purged, through the hook
lifetime and a seven-day cleanup margin. Provider delivery
is asynchronous: a successful invocation response means durable enqueue, not that
FCM or the target device has already accepted the push. The current implementation
is scoped to exactly one active gateway process, including exactly one active
outbox worker, per configured database; SQLite is the default local/dev backend
and PostgreSQL is the hosted-deployment-oriented backend, but multi-replica
coordination is not claimed.

### APIs and interaction flows by consumer

Handler errors for management, registration, legacy notification, and public hook
invocation use the sanitized JSON envelope described below. Readiness uses its
health response shape even when returning `503`. Management, registration, and
legacy notification endpoints require Nostr HTTP auth as described above. Public hook invocation does not use Nostr auth; the full hook URL
is the bearer capability and must be protected as a secret.

#### Operators and health checkers

Endpoints:

- `GET /health`
  - liveness compatibility endpoint; returns `{ "ok": true }`.
- `GET /live`
  - liveness endpoint; returns process health and may remain on the public
    listener.
- `GET /ready`
  - operator readiness endpoint; checks database/migration metadata, configured
    provider mode, outbox worker running state, worker concurrency, and outbox
    queue depth by status.
  - not exposed on the public listener by default. Serve it on
    `PUSH_GATEWAY_OPERATOR_BIND`, or protect it with
    `Authorization: Bearer $PUSH_GATEWAY_OPERATOR_TOKEN` when no separate
    operator listener is configured.
- `GET /metrics`
  - operator Prometheus-compatible text metrics with request counters, status-class
    counters, provider mode, outbox worker state, delivery success/failure
    counters, sanitized provider outcome reason-class counters
    (`auth`, `quota`, `network`, `invalid_token`, `invalid_payload`,
    `transient`), rate-limit rejection and invalid-token cleanup failure
    counters, outbox claim/claim-query counters, queue-depth gauges, oldest
    due/pending/retry age gauges, and dead-letter current/total gauges/counters.
    If database-backed outbox gauges cannot be queried, `/metrics` returns HTTP
    500 and emits `push_gateway_metrics_scrape_db_error 1` instead of silently
    reporting zero queue depth.
  - not exposed on the public listener unless explicitly enabled with
    `PUSH_GATEWAY_PUBLIC_METRICS_ENABLED=true`, or protected with
    `Authorization: Bearer $PUSH_GATEWAY_OPERATOR_TOKEN` when using token-only
    operator endpoints on the public listener.

Basic flow:

1. Deploy the gateway with a configured database and `noop` or `fcm` provider.
2. Use public `/live` for process liveness and protected/operator `/ready` for
   dependency/worker readiness before sending traffic.
3. Scrape protected/operator `/metrics` for low-cardinality counters and gauges. Do not configure
   proxies or monitoring to log full hook URLs or query strings.

These metrics describe the push-gateway process and delivery queue only. They
are unrelated to the separately governed FI telemetry path that gives Fedi
access to guardian/fedimintd Prometheus responses.

#### Guardian telemetry control plane

Endpoints:

- `POST /v1/telemetry/registrations`
  - public network route with exact-body NIP-98 authentication;
  - verifies the current Holder authorization against the selected environment
    and requires its subject to equal the NIP-98 signer;
  - idempotently replaces one AES-256-GCM-encrypted FMan endpoint/capability.
- `GET /v1/telemetry/fmans/{fman_pubkey}/seats`
  - operator-only authenticated seat discovery, returning optional invites.
- `GET /v1/telemetry/fmans/{fman_pubkey}/seats/{seat_id}/metrics`
  - operator-only live pull that mirrors guardian status and body bytes.

Telemetry is disabled unless both
`PUSH_GATEWAY_TELEMETRY_MANIFOLD_ENVIRONMENT` and
`PUSH_GATEWAY_TELEMETRY_ENCRYPTION_KEY` are present and valid. It also
requires an operator bind or token for the collector routes. FMan repeats its
idempotent registration periodically, which recovers receiver state without
acknowledgements or capability rotation.

#### Mobile app installations / app backend registration

Endpoints:

- `POST /registrations`
  - accepts one app installation registration containing `installation_id`,
    `fcm_token`, and optional `platform`; recipient is derived from Nostr auth.
  - in FCM mode, sends an [FCM HTTP v1 `validate_only: true`](https://firebase.google.com/docs/reference/fcm/rest/v1/projects.messages/send) data-only request
    before persistence. This verifies the token against the configured Fedi
    Firebase project without delivering a notification; invalid tokens return
    `422`, while provider/auth/network uncertainty fails closed with `503`.
  - stores the recipient/device/FCM-token mapping in the configured database.
    FCM tokens are globally unique. A signed same-recipient request may move a
    token between that recipient's installation ids atomically. Fedi retains
    both token and stable installation id across account switches. Every valid
    signed registration for that exact pair is authoritative, and the latest
    serialized mutation owns both durable rows atomically. Another still-valid
    clone may register later and take ownership back, so ownership can oscillate
    until only one clone continues. Reusing the
    token under a different installation id returns `409`. A signed unregister
    or rotation releases the pair while it is live; stale GC removes an orphaned
    owner after the refresh cutoff.
  - updates `last_seen_at` and re-enables the row if a disabled token is seen
    again.
- `DELETE /registrations/{installation_id}`
  - unregisters/deletes one installation mapping and releases token ownership,
    including after stale-row GC has already reclaimed the refreshable mapping.
- `POST /registrations/{installation_id}/disable?reason=...`
  - disables one installation without deleting the lifecycle row.

Basic flow:

1. The app obtains or refreshes its FCM registration token and chooses a stable
   installation id.
2. The app or app backend calls `POST /registrations` for the signed-in
   recipient. Repeated calls are safe: token rotation and same-recipient
   installation moves update the persistent mapping and clear disabled state.
   After a Fedi account switch, the newly authenticated recipient re-registers
   the unchanged token and stable installation id. If that registration commits
   latest, the route and owner row move atomically and hooks owned by the previous
   recipient no longer resolve a target.
3. On logout, account removal, or explicit push disable, call `DELETE` to remove
   the mapping or `POST .../disable` to keep lifecycle state while excluding it
   from delivery lookups.
4. Refresh the signed registration before `PUSH_GATEWAY_REGISTRATION_TTL_DAYS`
   (30 days by default). Stale rows are excluded immediately and removed by
   bounded on-admission garbage collection and the supplementary startup sweep.
5. Later hook invocations snapshot only the active, non-disabled, non-stale
   installation named by the hook.

#### Temporary trusted hook management caller

Endpoints:

- `POST /v1/hooks`
  - management endpoint for creating hook records scoped to the authenticated recipient.
  - every request must name an active owned `installation_id` and a positive
    `policy.ttl_seconds`.
  - requires Nostr HTTP auth.
  - stores only a hash of the generated hook secret.
  - returns the raw hook secret and full invocation URL once.
  - response includes `Cache-Control: no-store` and `Pragma: no-cache`.
- `GET /v1/hooks`
  - management endpoint for listing the authenticated recipient’s hook metadata.
  - never returns raw tokens or token hashes.
- `DELETE /v1/hooks/{hook_id}`
  - management endpoint for revoking one hook owned by the authenticated recipient.

Basic flow:

1. A trusted app-side caller creates a hook for a recipient and supplies any
   app-owned mobile-open contract grouped as `notification`, `open`, fixed
   `data`, and gateway `policy`.
2. The gateway persists hook metadata, stores only the hook secret hash, and returns
   the invocation URL exactly once. Treat that URL like a password.
3. The app/user shares the URL with the external service that should be able to
   notify them.
4. The management caller can list non-secret metadata for the recipient and
   revoke a hook. Revoked hooks reject every invocation, including replay of an
   idempotency key accepted before revocation.

Create-hook groups fields by purpose:

```json
{
  "installation_id": "initiating-installation-id",
  "label": "GitHub deploy alerts",
  "notification": {
    "kind": "deploy_alert",
    "title": "Deployment event",
    "body": "Something happened",
    "privacy": "display_text"
  },
  "open": {
    "behavior": "open_workflow",
    "workflow": "deployments",
    "action": "open_deploy_status",
    "deep_link": null
  },
  "data": { "project": "my-app" },
  "policy": {
    "ttl_seconds": 86400,
    "max_uses": 10,
    "rate_limit": { "window_seconds": 3600, "max_requests": 2 }
  }
}
```

The create response exposes the public `hook_id`, one-time `hook_secret`, and an
invocation URL shaped as `/hooks/{hook_id}/{hook_secret}`. The hook id is safe for
logs and metrics; the full URL remains a bearer capability and must be redacted.

#### External hook callers

Endpoint:

- `POST /hooks/{hook_id}/{hook_secret}`
  - public bearer-capability hook invocation endpoint.
  - for new invocations, validates the hook secret, TTL, revocation state,
    max-use count, and the hook's fixed-window rate limit.
  - accepts a small JSON body with optional `idempotency_key` and
    constrained caller `data`.
  - resolves the hook owner to the active installation fixed at hook creation.
  - durably enqueues one notification event plus one outbox row for that
    installation, then returns accepted. Provider delivery happens
    asynchronously in the background worker.

Basic flow:

1. The caller receives a hook URL from the app/user and stores it as a bearer
   secret; it should not be logged or exposed in analytics.
2. When an external event occurs, the caller `POST`s JSON to the hook URL. Supply
   `idempotency_key` to make retries idempotent for that hook through its lifetime
   and cleanup margin. Invocation callers cannot supply visible `title`/`body`; display
   text remains controlled by the hook owner.
3. For a new event, the gateway rejects unknown hook ids or wrong hook secrets as
   `404 hook_not_found`, rejects expired or revoked hooks after the correct secret
   is presented, and returns `429` for max-use or rate-limit exhaustion. A
   missing, disabled, or stale target returns retryable
   `503 target_installation_unavailable` without consuming the key or hook
   counters. A
   duplicate `idempotency_key` replay for an already accepted key verifies
   the secret and current revocation/expiry state, then returns the prior target
   count without enqueueing duplicate rows. Max-use or rate-limit exhaustion
   does not prevent that replay; revocation or expiry does.
4. On success for a new event, the caller receives `{ "accepted": true, ... }`
   after durable enqueue. This does not mean the mobile platform has delivered the
   push yet.

#### Legacy generic notification hook caller

Endpoint:

- `POST /hooks/notification`
  - accepts a generic notification hook payload.
  - requires Nostr auth and derives the recipient from the signer.
  - normalizes the request into a `Notification` value.
  - returns the normalized notification in the response.
  - does not send push notifications yet.

Basic flow:

1. A local/dev caller submits the legacy notification payload with Nostr auth.
2. The gateway validates the signed request and returns the normalized notification shape.
3. Use the stateful `/v1/hooks` plus `/hooks/{hook_id}/{hook_secret}` flow for the
   webhook-to-mobile-push path.

#### Background delivery worker and provider

The worker is not an external HTTP API consumer, but it is part of the observable
service flow:

1. The binary starts one worker after database/provider initialization and before
   serving HTTP. Startup resets interrupted `in_progress` rows.
2. The worker claims due `pending`/`retrying` rows with bounded concurrency and a
   claim fencing token, sends through the configured provider, and records
   terminal or retry state in `delivery_outbox`.
   Hook invocations wake the worker shortly after newly due rows commit.
3. Transient provider failures are retried with bounded backoff and may
   eventually become `dead_letter`; permanent non-token payload failures also
   become `dead_letter` immediately. Invalid/unregistered-token responses disable
   the matching current registration only if it still has the snapshotted token.
4. When no row is due, the worker sleeps until the nearest `next_attempt_at`
   retry/reclaim deadline, capped by a bounded fallback poll for missed
   in-process notifications, manual database changes, and coarse wall-clock
   deadlines.
5. Shutdown stops claiming new rows and waits for current delivery attempts to
   finish.

### Configuration

The binary accepts the same settings as command-line flags and environment
variables. Command-line flags take precedence over environment variables; run
`fedi-decentralized-push-gateway --help` for the generated `clap` reference.

- `--bind` / `PUSH_GATEWAY_BIND`
  - bind address for the public HTTP server.
  - defaults to `127.0.0.1:3000`.
- `--operator-bind` / `PUSH_GATEWAY_OPERATOR_BIND`
  - optional bind address for the operator HTTP server exposing `/ready` and
    `/metrics` (plus `/live`/`/health`).
  - bind it to loopback or a trusted private monitoring network, e.g.
    `127.0.0.1:9100`.
- `--operator-token` / `PUSH_GATEWAY_OPERATOR_TOKEN`
  - optional bearer token required on operator endpoints as
    `Authorization: Bearer ...`.
  - if set without `PUSH_GATEWAY_OPERATOR_BIND`, `/ready` and `/metrics` are
    available on the public listener only with this bearer token, except that
    explicit public metrics opt-in below intentionally leaves `/metrics`
    unauthenticated.
- `--public-metrics-enabled` / `PUSH_GATEWAY_PUBLIC_METRICS_ENABLED=true`
  - explicitly exposes unauthenticated `/metrics` on the public listener.
  - leave disabled for production unless a trusted network/reverse-proxy policy
    provides equivalent protection.
- `--app-id` / `PUSH_GATEWAY_APP_ID`
  - legacy/deprecated option retained for CLI compatibility; management and
    registration auth no longer uses request `app_id` equality.
- `--unsafe-allow-any-app-id-for-tests` /
  `PUSH_GATEWAY_UNSAFE_ALLOW_ANY_APP_ID_FOR_TESTS=true`
  - legacy/deprecated option retained for CLI compatibility; signed Nostr auth is
    still required.
- `--database-url` / `PUSH_GATEWAY_DATABASE_URL`
  - SQLite or PostgreSQL database URL.
  - defaults to `sqlite://push-gateway.sqlite?mode=rwc`.
- `--public-base-url` / `PUSH_GATEWAY_PUBLIC_BASE_URL`
  - public HTTPS origin prepended to generated hook invocation paths and used as
    the trusted base for Nostr auth URL verification.
  - production-facing CLI/env configuration rejects path-bearing, userinfo-bearing,
    malformed, or non-HTTPS values; use an origin such as
    `https://push.example.com`.
  - defaults to non-routable `https://push-gateway.invalid`; production operators
    must set the deployed HTTPS origin explicitly.
  - `--allow-insecure-public-base-url` /
    `PUSH_GATEWAY_ALLOW_INSECURE_PUBLIC_BASE_URL=true` permits empty or local
    `http://localhost`/loopback origins only for local tests.
- `--provider` / `PUSH_GATEWAY_PROVIDER`
  - push provider mode: `noop` by default, or `fcm` to enable real FCM HTTP v1.
  - unknown explicit values fail startup/config loading.
  - default CI, `defe`, and local tests should keep the default `noop`/fake mode.
- `--production-mode` / `PUSH_GATEWAY_PRODUCTION_MODE=true`
  - configuration gate for production deployments. When enabled, startup rejects
    `noop` provider mode, the default placeholder public URL, admission without
    anything other than exactly one admission mode (open XOR emergency
    allowlist), zero source,
    recipient, global, or outbox caps, the legacy direct notification endpoint,
    FCM endpoint overrides, and non-default OAuth token URIs.
  - set explicit `PUSH_GATEWAY_PUBLIC_BASE_URL`, set
    `PUSH_GATEWAY_PROVIDER=fcm`, configure FCM credentials, set nonzero
    `PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG` and
    `PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG`, set
    `PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED=false`, and choose exactly
    the intended admission mode below.
- `--open-self-registration-enabled` /
  `PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED=true`
  - explicitly enables the FI MVP admission model: any installation with a
    valid NIP-98 signer may self-register before quote/formation.
  - defaults to `false`, so production fails closed unless this flag or the
    emergency static allowlist is set. Production rejects setting both. FCM validate-only plus the limits below
    are admission/resource checks, not proof that a signer is a trusted FI.
- `--admission-allowed-recipients` /
  `PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS`
  - optional emergency comma-separated canonical Nostr public-key restriction
    for management and registration APIs. It is an alternative to open
    self-registration, not the normal FI MVP admission mechanism.
- `--legacy-notification-hook-enabled` /
  `PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED`
  - controls `POST /hooks/notification`, which acknowledges legacy direct
    notifications without durable delivery. Defaults to `true` for compatibility;
    production mode requires `false`.
- `--outbox-worker-concurrency` / `PUSH_GATEWAY_OUTBOX_WORKER_CONCURRENCY`
  - optional maximum number of concurrent outbox deliveries processed by the
    background worker.
  - defaults to `4`.
- `--retention-days` / `PUSH_GATEWAY_RETENTION_DAYS`
  - retention horizon for sensitive terminal push data purged at startup.
  - defaults to `7`.
- `--registration-ttl-days` / `PUSH_GATEWAY_REGISTRATION_TTL_DAYS`
  - signed-refresh horizon for active installations; defaults to `30` days.
    Stale registrations are not delivery targets and are purged by bounded
    on-admission GC as well as the supplementary startup sweep.
- In-memory production abuse controls (single-process only; reset on restart):
  - authenticated management events per source prefix before replay-cache
    insertion: `--auth-events-per-source-prefix` /
    `PUSH_GATEWAY_AUTH_EVENTS_PER_SOURCE_PREFIX` (default `120` per 60 seconds),
    with `--auth-event-window-seconds` controlling the window. This source-only
    budget is trusted-proxy aware.
  - hook invocations per source prefix:
    `--hook-invocations-per-source-prefix` /
    `PUSH_GATEWAY_HOOK_INVOCATIONS_PER_SOURCE_PREFIX` (default `60/hour`).
    Sources are normalized as IPv4 addresses or IPv6 `/64` prefixes.
  - hook invocations per hook:
    `--hook-invocations-per-hook` / `PUSH_GATEWAY_HOOK_INVOCATIONS_PER_HOOK`
    (default `60/hour`); persisted per-hook policy is still enforced and may be
    lower.
  - invocation window:
    `--hook-invocation-window-seconds` /
    `PUSH_GATEWAY_HOOK_INVOCATION_WINDOW_SECONDS` (default `3600`).
  - hook creations per recipient:
    `--hook-creations-per-recipient` (default `5/hour`).
    `--hook-creation-window-seconds` controls the window (default `3600`).
  - registration writes per recipient/source:
    `--registration-changes-per-recipient-source` (default `10/hour`).
  - registration writes per source across changing recipient keys:
    `--registration-changes-per-source-prefix` (default `30/hour`).
    `--registration-change-window-seconds` controls the window (default `3600`).
  - live resource caps:
    `--max-active-hooks-per-recipient` (default `20`) and
    `--max-active-installations-per-recipient` (default `8`), plus
    `--max-active-hooks-global` and `--max-active-installations-global`
    (both default `50000`). Global exhaustion returns `503`.
  - physical row high-watermarks:
    `--max-hook-rows-global` / `PUSH_GATEWAY_MAX_HOOK_ROWS_GLOBAL` and
    `--max-registration-rows-global` /
    `PUSH_GATEWAY_MAX_REGISTRATION_ROWS_GLOBAL` (defaults `100000` and
    `200000`, respectively). The
    registration ceiling counts both refreshable installation rows and durable
    token-owner rows. Each
    admission transaction first reclaims up to
    `--admission-gc-batch-size` / `PUSH_GATEWAY_ADMISSION_GC_BATCH_SIZE`
    terminal/stale rows (default `1000`) and fails closed with `503` if headroom
    is still unavailable.
  - backlog caps: `--max-global-outbox-backlog` returns `503`, and
    `--max-recipient-outbox-backlog` returns `429`; `0` disables either cap.
  - `--trusted-proxy-cidrs` / `PUSH_GATEWAY_TRUSTED_PROXY_CIDRS`: only direct
    socket peers in these CIDRs may supply `X-Forwarded-For` or `Forwarded`.
    Untrusted peers' forwarding headers are ignored. Configure trusted proxies to
    strip or overwrite inbound forwarding headers from clients.
- `--fcm-service-account-file` / `FCM_SERVICE_ACCOUNT_FILE`
  - path to a Firebase service-account JSON file used when
     `PUSH_GATEWAY_PROVIDER=fcm`.
  - preferred for secret-manager/file-based injection.
- `--fcm-service-account-json` / `FCM_SERVICE_ACCOUNT_JSON`
  - raw Firebase service-account JSON used when `PUSH_GATEWAY_PROVIDER=fcm`.
  - useful for env-based secret injection; do not log or commit it.
- `FIREBASE_CREDENTIALS_JSON`
  - legacy alias for `FCM_SERVICE_ACCOUNT_JSON`, only read in `fcm` mode.
- `--fcm-send-endpoint-base` / `FCM_SEND_ENDPOINT_BASE`
  - optional FCM endpoint base override, intended for local fake-server tests.
  - defaults to `https://fcm.googleapis.com`; production mode rejects overrides.
- `--fcm-max-concurrency` / `FCM_MAX_CONCURRENCY`
  - optional maximum concurrent FCM HTTP requests.
  - defaults to `16`.

See [`OPERATIONS.md`](./OPERATIONS.md) for deployment examples, production
configuration guidance, secret handling, readiness/metrics usage, backup/restore,
migration guidance, the dead-letter `outbox` admin CLI, and operational
playbooks.

Handler API failures are returned as a sanitized JSON envelope:

```json
{ "error": { "code": "stable_code", "message": "sanitized message" } }
```

The envelope is used by the management, registration, legacy notification, and
public hook invocation handlers for validation, authentication, JSON
parse/body-limit, and database errors; internal details and token-bearing values
are not returned to clients. Readiness failures use the `/ready` health response
shape instead.

### FCM HTTP v1 delivery

Real Firebase delivery is disabled unless `PUSH_GATEWAY_PROVIDER=fcm` is set.
In FCM mode the gateway parses a Firebase service-account JSON object, validates
that it is a `service_account`, and keeps credential/debug output redacted. The
provider creates Google OAuth2 JWT bearer assertions with the
`firebase.messaging` scope, exchanges them for access tokens, caches tokens until
shortly before expiry, and sends FCM HTTP v1 token messages over a bounded
reqwest/rustls HTTP client with connection and request timeouts.

Hook notification title/body become the FCM notification title/body when present.
Hook-owned app-open context and non-reserved invocation data are forwarded as FCM
`data` along with `recipient_id`, `notification_id`, and `kind`, which lets the
mobile app route taps to the relevant workflow/deep-link context. FCM data values
are sent as strings;
non-string local JSON values are compact-JSON encoded before send. The gateway
rejects FCM-reserved data keys (`from`, `message_type`, `google*`, and `gcm.*`)
and validates the final FCM data payload size after gateway-added keys. Invalid
or unregistered-token FCM responses disable the corresponding registration with
reason `invalid_token`. Generic bad-payload FCM `INVALID_ARGUMENT` responses are
dead-lettered as payload failures and do not disable registrations.
Provider/network/quota/auth failures are treated as
transient delivery failures recorded on outbox rows for retry/backoff; they do
not disable registrations. Invalid or unregistered-token classifications disable
the corresponding registration and mark that outbox row terminal.

### `defe` resource

`defe` can allocate the gateway as a `PushGateway` resource. The descriptor gives callers the HTTP `url` / `host` / `port` and the per-resource SQLite `database_path`.

`defe-cli --request-push-gateway[=shared|exclusive]` exports these values as `DEV_DEFE_PUSH_GATEWAY_URL`, `DEV_DEFE_PUSH_GATEWAY_PORT`, and `DEV_DEFE_PUSH_GATEWAY_DATABASE_PATH`.

The defe-managed e2e test in `tests/defe_e2e.rs` is ignored by default because
it requires `defe exec` or a persistent `defe serve` configured with
push-gateway resource support. It exercises registration, hook creation, hook
invocation, and no-op-provider outbox completion without real FCM credentials.

### Persistence

The gateway uses SQLx with a shared portable schema for SQLite and PostgreSQL.
SQLite is the default local/dev, `defe`, and CI backend. PostgreSQL is the
production-oriented backend for hosted deployments. This does not by itself make
the single-active-process operational model production-complete.

The initial schema creates a `push_registrations` table:

- primary key: `(recipient_id, installation_id)`.
- unique index: `fcm_token`.
- `installation_id` is no longer globally unique; it is scoped by recipient so a
  different recipient cannot steal/disrupt a registration merely by reusing an
  installation id. The FCM token remains globally unique. A separate durable
  owner row binds the live token to that stable installation id and is reclaimed
  after it becomes an old orphan. Every valid signed registration for the exact
  pair is authoritative across authenticated recipients.
- lifecycle fields: `created_at`, `updated_at`, `last_seen_at`,
  `disabled_at`, and `disabled_reason`.

Registration upsert behavior:

- upserts by `(recipient_id, installation_id)`.
- atomically moves an existing token between installation ids owned by the same
  authenticated recipient. Across recipients, the latest serialized valid
  registration for the exact existing token/installation pair owns both durable
  rows; another still-valid clone can take it back later. Cross-recipient reuse
  under a different installation id returns `409` without deleting the prior
  route.
- updates the token, platform, and updated timestamp for the same recipient and
  installation.
- sets `last_seen_at` on every accepted registration and clears disabled state.
- recipient lookup requires an explicit freshness cutoff and returns only active,
  non-disabled, non-stale registrations.
- admission uses separate recipient/global net active deltas and the physical
  row delta for moves,
  performs bounded stale-row GC, and enforces physical as well as active caps in
  one transaction serialized by a database admission mutex. Durable token-owner
  rows count toward the physical ceiling and are never removed merely because a
  refreshable registration became stale.

SQLite is intentionally tuned for the current local/dev service: the pool is
bounded, `busy_timeout` is set, foreign keys are enabled, and WAL/normal
synchronous mode is enabled. Hook create/revoke, registration
upsert/delete/disable, invocation acceptance, and outbox-worker database writes
share the clone-shared database handle's process-wide coordinator so deferred
SQLite transactions cannot race while upgrading to writers. At most 64 request
writes may wait or run; saturation returns `503`. Once a worker queues, later
request mutations cannot pass it, and provider calls remain concurrent outside
the coordinator.
SQLite bounds engine contention with its five-second `busy_timeout`; PostgreSQL
connections use a five-second statement timeout. PostgreSQL is supported by the
same repository code path for
persistent hosted deployments. HA/multi-replica production deployment is
explicitly out of scope for this release; run exactly one active gateway process
and worker per database.


The schema also creates `notification_events`, `delivery_outbox`, and compact
`hook_idempotency_tombstones` tables:

- `notification_events` records the accepted hook invocation, hook id, optional
  caller `idempotency_key`, recipient id, serialized notification, target count, and
  creation time. `(hook_id, caller_idempotency_key)` remains unique while the
  event is retained.
- `hook_idempotency_tombstones` records only hook id, caller key, accepted target
  count, accepted time, and retention deadline. A marker is committed in the same
  transaction as each keyed accepted event and survives sensitive event/outbox
  cleanup. It is retained until the finite hook lifetime plus seven days (or a
  bounded one-year legacy-hook fallback plus seven days) and fences hook GC until
  it expires.
- `delivery_outbox` stores one row per target registration snapshot with status,
  attempt count, next retry time, last sanitized error, and serialized
  notification. Statuses are `pending`, `in_progress`, `retrying`, `succeeded`,
  `invalid_token`, and `dead_letter`. `succeeded` means the configured
  provider accepted the request; it does not assert device receipt, app
  handling, or user display. Every accepted target has a persisted five-minute
  resolution deadline, after which it becomes the actionable
  `dead_letter` outcome `resolution_deadline_exceeded`.
- Terminal `notification_events` / `delivery_outbox` rows can contain serialized
  notification payloads and push-token snapshots. At startup the server applies
  the configured retention horizon (`--retention-days` /
  `PUSH_GATEWAY_RETENTION_DAYS`, default 7 days) by deleting terminal outbox rows,
  old disabled registration rows, and old notification events that no longer have
  retained outbox rows. Pending/retrying/in-progress delivery state is not purged.
- The portable schema enforces non-negative timestamps and counters, positive
  persisted rate-limit/max-use values where those policies are present, the known
  outbox status set, and one delivery row per `(event_id, recipient_id,
  installation_id)`.

Hook invocation response semantics are accepted-after-durable-enqueue: once the
database transaction commits the gateway returns `{ accepted: true, reason:
"accepted", delivery_attempts: N }` even if the provider is down. The background
worker resets interrupted `in_progress` rows on startup, claims due
`pending`/`retrying` rows and expired `in_progress` leases with claim fencing,
delivers with bounded concurrency, retries transient provider failures with
bounded backoff, dead-letters rows after retry exhaustion or immediate permanent
payload failures, and disables registrations for provider errors classified as
permanent invalid tokens. Generic bad-payload FCM `INVALID_ARGUMENT` responses
dead-letter without disabling registrations. Shutdown asks the worker to stop
claiming new rows and wait for current deliveries to finish. Stored hook and
outbox JSON is fail-closed: corrupted hook data returns a sanitized internal
error, and corrupted claimed outbox notification JSON is dead-lettered without
calling the push provider.

Idempotency scope is intentionally narrow: it applies only when a hook caller
supplies `idempotency_key` and only per hook id. It deduplicates durable event/outbox
creation, not external caller HTTP retries without an `idempotency_key`, and not a
future multi-replica deployment. The hook secret is still checked before an
idempotent replay is accepted. New accepted invocations commit hook counter
consumption, event storage, registration snapshotting, outbox rows, and the compact
accepted-key marker in one database transaction. Replaying an already accepted
idempotency key returns the recorded target count without consuming max-use or
rate-limit counters even after sensitive event/outbox retention cleanup and even
if those counters have since been exhausted. A new idempotency key is evaluated
against current TTL, revocation, max-use, and rate-limit policy.

The marker table has an independent global fail-closed ceiling equal to the
configured physical hook-row ceiling. Each new keyed invocation first reclaims a
bounded expired batch, then returns `503 idempotency_capacity_exceeded` without
mutating hook state if no marker headroom remains. FI formation hooks use an exact
30-day TTL, so a DKG or wallet-service completion key remains replay-safe across
the whole formation callback window and its cleanup margin.

The migrated schema contains a `notification_hooks` table:

- primary key: `hook_id`.
- stores `hook_secret_hash`; raw hook secrets are returned once and not persisted.
- stores owner `recipient_id`, optional label/default notification metadata,
  app-owned workflow/action/deep-link context, app-open behavior, privacy
  posture, optional `expires_at`, optional `max_uses`, revocation state, use
  count, and last-used timestamp.
- stores per-hook fixed-window rate-limit policy and current window counters.

### Hook abuse controls

New hooks default to a production-conservative fixed-window rate limit of **2
accepted invocations per hour** (`max_requests = 2`,
`window_seconds = 3600`). Management callers may set
`policy.rate_limit.window_seconds` and `policy.rate_limit.max_requests` when creating a hook;
values must be in the accepted local/dev ranges of 1–86,400 seconds and
1–10,000 requests. In production mode, hook creation rejects policies that are
more permissive than 2 accepted invocations per 3600-second window; there is no
privileged per-hook high-rate exception flow. Production invocation also applies
the conservative effective cap to any pre-existing or manually edited permissive
persisted hook rows. The limiter is
enforced in the same database `UPDATE` that checks the hook secret, TTL, revocation,
and max-use policy and increments `use_count`, so racing invocations in the
current single-active-process deployment cannot both consume the same remaining
rate-limit slot.

Rate limits are per hook, not per IP address or per caller. They reset when the
configured fixed window has elapsed since that hook's current window start. A
rate-limited invocation returns the sanitized error envelope with HTTP `429` and
code `hook_rate_limited`. Unknown hook secrets return the same
`404 hook_not_found` response; expired and revoked hooks are only distinguished
after the correct token is presented.

The service also enforces a 16 KiB HTTP body limit and validates hook invocation
fields: idempotency key length, top-level data JSON size, data key count,
and data key length. These limits are intended to bound local/dev abuse while
production auth, provider, deployment, and distributed rate-limit decisions are
still open.

### Mobile notification/deep-link contract

The hook owner, through the temporary management API, owns mobile routing context.
External hook callers may trigger the hook and may supply an
`idempotency_key` plus non-reserved diagnostic/reference `data`;
they cannot supply or override app-open routing fields.

Create-hook accepts these app-owned contract fields under `notification` and `open`:

- `kind`: application notification kind, copied to `Notification.kind` and FCM
  `data.kind`.
- `workflow`: optional app workflow identifier.
- `action`: optional action within the workflow.
- `deep_link`: optional mobile app link. The gateway currently accepts only
  non-empty `fedi://...` links or absolute in-app paths beginning with one `/`;
  arbitrary web URLs and protocol-relative `//...` paths are rejected.
- `open.behavior`: `open_app` (default), `open_workflow`, or `open_deep_link`.
  `open_workflow` requires `workflow`; `open_deep_link` requires `deep_link`.
- `privacy`: `display_text` (default) or `data_only`. `data_only` suppresses
  notification title/body from the stored hook defaults. This keeps display text
  out of the provider payload, but provider- and
  platform-specific background wake behavior is not guaranteed by this contract.

Accepted invocations produce a normalized `Notification` whose `data` contains
reserved gateway-owned keys:

- `pg.open_behavior`
- `pg.privacy`
- `pg.workflow` when configured
- `pg.action` when configured
- `pg.deep_link` when configured

The FCM provider also adds top-level FCM data fields `recipient_id`,
`notification_id`, and `kind` from the normalized notification. Mobile apps
should treat `pg.*` keys as gateway-owned routing hints and ignore conflicting
non-reserved caller fields. Hook creation and invocation reject `data` keys
starting with `pg.` and the reserved names `recipient_id`, `notification_id`,
`kind`, `workflow`, `action`, `deep_link`, `open_behavior`, and `event_id`; app
routing context must be supplied through the typed hook fields above, and
notification identity is gateway-generated, not supplied by the public caller.
They also reject FCM-reserved names (`from`,
`message_type`, `google*`, and `gcm.*`) and reject invocations whose final FCM
data map would exceed 4096 bytes after gateway-added keys.

Example create-hook body:

```json
{
  "notification": {
    "kind": "federation.setup",
    "privacy": "display_text",
    "title": "Federation update"
  },
  "open": {
    "behavior": "open_deep_link",
    "deep_link": "fedi://workflows/federation-setup/review",
    "workflow": "federation_setup",
    "action": "review_guardians"
  },
  "data": { "federation_id": "fed-1" }
}
```

## Not implemented yet

- Distributed/multi-replica rate limiting. Current abuse limits are in-memory,
  single-process only, and reset on restart.
- Distributed/multi-process Nostr auth replay protection.
- Multi-provider APNs/WebPush/UnifiedPush support.
- Real FCM smoke tests in default CI; any real-FCM validation must be
  manual/ignored and use sandbox credentials.
- Multi-process or multi-replica production storage semantics.

## Target hook model

Prefer stateful server-side hook records rather than stateless-only signed URLs.
The generated HTTPS URL contains a public hook id and separate opaque
unguessable bearer secret, for example:

```text
/hooks/{hook_id}/{hook_secret}
```

The gateway should look up by public `hook_id` and store only a unique indexed
hash of the secret, never the raw secret. The `hook_id` remains non-secret
management metadata for listing, logs, metrics, and revocation; the full URL is
still a bearer capability.

The current implementation uses this model for the local/dev MVP. Hook ids and
hook secrets are generated from OS randomness and URL-safe base64; secret hashes are
SHA-256 because hook secrets are high-entropy bearer tokens.

Hook record metadata should support at least:

- owner/recipient/app user id;
- user-visible label or caller description;
- `created_at`;
- optional `expires_at` / TTL;
- revoked/deleted state;
- optional rate-limit policy;
- optional max-use count;
- optional notification kind, workflow, and deep-link context.

This model supports identifying hooks and callers, applying rate limits,
cancelling or deleting hooks, enforcing TTLs, and carrying scoped app context
without exposing meaningful metadata in the URL.

See the Linked Specs records in [`specs/`](./specs/) —
[`ARCH-push-gateway`](./specs/ARCH-push-gateway.md) is the entry point — and
[`testing.md`](./testing.md) for the default fake/stub provider testing
strategy. See [`OPERATIONS.md`](./OPERATIONS.md) for deployment and runbook
guidance.

## Operational observability

The router attaches a gateway-generated `x-request-id` on every response and
logs one structured line per request using HTTP method, sanitized route template,
status code, latency, and request id. It intentionally does not trust or log
client-supplied request ids and does not log raw URLs, query strings, hook secrets, FCM tokens, credential material, or registration tokens. Proxies in
front of the service must also avoid logging full hook URLs.

The `/ready` response is the primary operator diagnostic surface for dependency
health and queue depth. `/metrics` exposes low-cardinality counters and gauges
only; it never includes hook ids, recipients, tokens, secrets, raw provider
errors, or full URLs. In-process counters reset on restart; database-backed
outbox gauges come from the configured database, which remains the durable source
of truth for registrations, hooks, events, and outbox state. Treat an HTTP 500
metrics scrape with `push_gateway_metrics_scrape_db_error 1` as an explicit
scrape failure, not as an empty queue.
