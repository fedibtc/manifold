# Push gateway operations and deployment runbook

The push gateway is currently suitable only for a single-active-process deployment.
SQLite is supported for local/dev, `defe`, and simple test deployments.
PostgreSQL is the production-oriented storage backend for hosted deployments.
Management and registration endpoints use Nostr/NIP-98 signed requests and derive
`recipient_id` from the signer’s canonical lowercase hex pubkey. Exactly one active gateway process, including
exactly one active outbox worker, may run against a configured database. Do not
run multiple active gateway processes as a production HA topology until
worker/idempotency/rate-limit coordination is explicitly designed and tested.

## Production configuration reference

The binary uses `clap` for both command-line flags and environment variables.
Command-line flags take precedence over environment variables; run
`fedi-decentralized-push-gateway --help` for the generated reference.
Operator/admin subcommands use the same database URL but do not run migrations.
Keep the gateway service stopped while running mutating admin commands; this
preserves the current single-active-process assumption and avoids racing the
outbox worker.

- `--bind` / `PUSH_GATEWAY_BIND`: HTTP bind address. Defaults to
  `127.0.0.1:3000`. Use a reverse proxy/TLS terminator for Internet-facing
  HTTPS.
- `--operator-bind` / `PUSH_GATEWAY_OPERATOR_BIND`: optional operator HTTP bind
  address for `/ready` and `/metrics` (plus `/live`/`/health`). Prefer loopback
  or a trusted private monitoring network, such as `127.0.0.1:9100`.
- `--operator-token` / `PUSH_GATEWAY_OPERATOR_TOKEN`: optional bearer token
  required on operator endpoints as `Authorization: Bearer ...`. This is useful
  in addition to an operator listener, and required if you intentionally make
  `/ready`/`/metrics` available on the public listener instead of a separate
  operator listener.
- `--public-metrics-enabled` / `PUSH_GATEWAY_PUBLIC_METRICS_ENABLED=true`:
  explicitly exposes unauthenticated `/metrics` on the public listener. Leave it
  disabled in production unless equivalent network/reverse-proxy controls protect
  the public listener.
- `--telemetry-manifold-environment` /
  `PUSH_GATEWAY_TELEMETRY_MANIFOLD_ENVIRONMENT`: Manifold environment used to
  resolve the pinned PeerBadge issuer and trust profile.
- `--telemetry-encryption-key` / `PUSH_GATEWAY_TELEMETRY_ENCRYPTION_KEY`:
  exactly 32 random bytes encoded as 64 hexadecimal characters. Store this in a
  secret manager, never in the image or database.

The two telemetry settings are all-or-nothing. Missing, partial, or malformed
configuration fails at startup. Telemetry also fails closed when the
chosen environment has no configured PeerBadge issuer roots or its minimum
trust policy is invalid.
Telemetry collection also requires `PUSH_GATEWAY_OPERATOR_BIND` or an operator
token because its live target route is mounted only on the operator router.
- `--database-url` / `PUSH_GATEWAY_DATABASE_URL`: SQLite or PostgreSQL URL. Defaults to
  `sqlite://push-gateway.sqlite?mode=rwc`. `postgres://...` and
  `postgresql://...` are accepted. The database contains registrations, hook
  metadata, hook hashes, notification events, outbox rows, push-token snapshots,
  and notification content; protect it as sensitive data.
- `--app-id` / `PUSH_GATEWAY_APP_ID`: legacy/deprecated CLI compatibility option;
  management and registration auth no longer uses request `app_id` equality.
- `--unsafe-allow-any-app-id-for-tests` /
  `PUSH_GATEWAY_UNSAFE_ALLOW_ANY_APP_ID_FOR_TESTS=true`: legacy/deprecated CLI
  compatibility option; signed Nostr auth is still required.
- `--public-base-url` / `PUSH_GATEWAY_PUBLIC_BASE_URL`: public HTTPS origin used
  when returning one-time hook URLs from `POST /v1/hooks` and when verifying the
  `u` tag in Nostr auth events. Production-facing
  configs must use an absolute `https://` origin with no userinfo, path, query,
  or fragment. The default `https://push-gateway.invalid` is intentionally
  non-routable; set the deployed HTTPS origin explicitly. `defe` and local tests
  may set `PUSH_GATEWAY_ALLOW_INSECURE_PUBLIC_BASE_URL=true` as a local/test-only
  escape hatch for empty or loopback HTTP origins.
- `--provider` / `PUSH_GATEWAY_PROVIDER`: `noop` (default local/dev mode) or
  `fcm` (real FCM HTTP v1 delivery).
- `--production-mode` / `PUSH_GATEWAY_PRODUCTION_MODE=true`: enable startup-time
  production safety validation. Production mode requires `fcm`, explicit
  `PUSH_GATEWAY_PUBLIC_BASE_URL`, exactly one admission mode, nonzero auth/source,
  recipient, active-resource, physical-row, admission-GC, and outbox backlog caps, the legacy direct
  notification endpoint disabled, no `FCM_SEND_ENDPOINT_BASE` override, and the
  default Google OAuth service-account token URI.
- `--open-self-registration-enabled` /
  `PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED=true`: normal FI MVP admission.
  Any correctly NIP-98-authenticated signer may attempt registration; FCM mode
  then validates the token against the configured Fedi Firebase project without
  delivery. Defaults off so missing production configuration fails closed.
- `--admission-allowed-recipients` /
  `PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS`: comma-separated canonical Nostr
  public keys admitted to management and registration APIs. This is an emergency
  restrictive alternative to the open FI admission flag. Do not enable the open
  flag when the allowlist is intended to restrict access. Production rejects
  both modes together as well as neither mode.

Admission incident switch procedure:

1. Prepare the canonical 64-character lowercase Nostr public keys that must
   remain admitted.
2. Stop the single active gateway process.
3. Remove or set `PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED=false`; set the
   non-empty `PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS` value. Never overlap the
   two modes—the production config validator intentionally refuses to start.
4. Restart, confirm `/ready`, and verify a non-allowlisted signed management
   request receives `403 recipient_not_admitted`.
5. To restore normal FI admission, stop again, remove the allowlist completely,
   set `PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED=true`, then restart and
   verify readiness. Existing registrations/hooks are not deleted by switching
   admission mode.
- `--legacy-notification-hook-enabled` /
  `PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED`: defaults to `true` for
  compatibility; set `false` for production so `/hooks/notification` is not
  mounted.
- `--fcm-service-account-file` / `FCM_SERVICE_ACCOUNT_FILE`: preferred FCM
  credential source in `fcm` mode.
- `--fcm-service-account-json` / `FCM_SERVICE_ACCOUNT_JSON`: raw service-account
  JSON source in `fcm` mode for environments that cannot mount files. Avoid
  because environment variables are often exposed by process dumps or service
  managers.
- `FIREBASE_CREDENTIALS_JSON`: legacy alias for `FCM_SERVICE_ACCOUNT_JSON`.
- `--fcm-send-endpoint-base` / `FCM_SEND_ENDPOINT_BASE`: override for
  fake-server tests; production mode rejects this override.
- `--fcm-max-concurrency` / `FCM_MAX_CONCURRENCY`: max concurrent FCM HTTP
  requests. Defaults to `16`.
- `--outbox-worker-concurrency` / `PUSH_GATEWAY_OUTBOX_WORKER_CONCURRENCY`: max
  concurrent outbox rows processed by the worker. Defaults to `4`.
- `--registration-ttl-days` / `PUSH_GATEWAY_REGISTRATION_TTL_DAYS`: maximum
  interval between signed registration refreshes. Defaults to 30 days.
- `--auth-events-per-source-prefix` /
  `PUSH_GATEWAY_AUTH_EVENTS_PER_SOURCE_PREFIX`: valid management auth events
  admitted per trusted-proxy-aware source prefix before replay-cache insertion;
  defaults to 120 per `PUSH_GATEWAY_AUTH_EVENT_WINDOW_SECONDS=60` seconds.
- `--max-hook-rows-global` / `PUSH_GATEWAY_MAX_HOOK_ROWS_GLOBAL` and
  `--max-registration-rows-global` /
  `PUSH_GATEWAY_MAX_REGISTRATION_ROWS_GLOBAL`: physical row high-watermarks;
  hooks default to 100,000 and registration/owner rows default to 200,000. The
  registration high-watermark includes refreshable registration rows plus
  durable FCM-token owner rows.
- `--admission-gc-batch-size` / `PUSH_GATEWAY_ADMISSION_GC_BATCH_SIZE`: maximum
  stale/terminal rows reclaimed by one storage-owned admission transaction;
  defaults to 1,000. Admission fails closed if bounded GC cannot restore
  physical headroom.

In-memory limits are production guardrails for the currently supported
single-process deployment. They are not coordinated across processes and reset on
restart. Limiter families are partitioned: they prune expired windows, never
evict a live window for a new key, fail closed when a family is saturated, and
cannot reset another family. Defaults are: 8 active installations/recipient, 20 active
hooks/recipient, 50,000 active installations and 50,000 hooks globally, 5 hook
creations/hour/recipient, 10 registration writes/hour/recipient/source, 30
registration writes/hour/source across rotating recipient keys, 120 authenticated
management events/minute/source before replay insertion, and 60 public hook
invocations/hour/source prefix plus 60/hour/hook before any lower persisted
per-hook policy. Configure trusted reverse proxies with
`--trusted-proxy-cidrs`; `X-Forwarded-For`/`Forwarded` are used only when the
direct peer IP is trusted. Trusted proxies should strip or overwrite inbound
forwarding headers from clients before forwarding to the gateway.

## Database-write backpressure and timeouts

The gateway admits at most 64 request-side database mutations at once, including
requests waiting for the process-local write coordinator. A saturated request
receives `503` with the stable error code `database_write_queue_full`; callers
should use bounded exponential backoff rather than immediately retrying. Worker
recovery, expiry, claim, and completion writes bypass this request admission
limit. Once a worker queues, later request mutations cannot pass it; only a
request that had already crossed request serialization can run first.

Treat this response as gateway load backpressure, not database unavailability:
`/ready` remains healthy and `/metrics` does not set
`push_gateway_metrics_scrape_db_error` solely because the admission queue is
full. Inspect public request rate, `push_gateway_outbox_oldest_due_age_seconds`,
worker-running state, and sanitized request logs. Reduce incoming traffic or
scale down caller concurrency before retrying. In contrast, a failed `/ready`,
`push_gateway_metrics_scrape_db_error 1`, or database error logs indicate
database reachability/performance work rather than admission saturation.

SQLite uses a five-second engine `busy_timeout`. PostgreSQL pool connections set
`statement_timeout` to five seconds, so a stalled statement returns a sanitized
database failure instead of holding the coordinator indefinitely. Investigate
PostgreSQL lock waits, slow-query plans, connection saturation, and server logs;
do not raise this timeout without reassessing the durable-delivery deadline and
worker recovery latency.

New hooks default to a production-conservative fixed-window rate limit of 2
accepted invocations per hour. Management callers can set validated per-hook
limits at creation time for explicit test/dev deployments. When
`--production-mode` is enabled, hook creation rejects policies that are more
permissive than 2 accepted invocations per 3600-second window; there is no
privileged high-rate exception flow. Production invocation also applies the
conservative effective cap to any pre-existing or manually edited permissive
persisted hook rows. There is no distributed rate-limit backend.

## Operator endpoints and observability

- `GET /live`: liveness. Confirms the process can serve HTTP. This endpoint may
  remain unauthenticated on the public listener.
- `GET /ready`: readiness. Checks database/migration metadata, provider mode/config
  selected at startup, worker running state, worker concurrency, and outbox queue
  depths. Returns HTTP 503 when the database is unavailable or the worker is not
  running.
- `GET /metrics`: Prometheus text exposition with HTTP request/status counters,
  outbox queue-depth gauges, oldest due/pending/retrying age gauges,
  dead-letter current/total metrics, delivery success/failure counters,
  provider failure reason-class counters (`auth`, `quota`, `network`,
  `invalid_token`, `invalid_payload`, `transient`), invalid-token cleanup
  failures, hook rate-limit rejections, physical hook/registration row gauges by
  low-cardinality eligibility/ownership state, outbox claim/claim-query counters,
  worker running state, and configured provider mode. If the database-backed
  outbox query fails, the scrape returns HTTP 500 with
  `push_gateway_metrics_scrape_db_error 1` and logs a sanitized
`event=metrics_scrape_error`; it does not report queue depths as zero.

This `/metrics` surface is strictly push-gateway operator telemetry. It is not
the FI guardian telemetry mechanism and does not expose or proxy guardian or
fedimintd Prometheus endpoints.

Guardian telemetry adds low-cardinality state for whether the receiver is
configured and the current admitted FMan-target count.

Discover one FMan's seats, then pull a selected live guardian through the
protected operator routes:

```sh
curl --fail-with-body   -H "Authorization: Bearer $PUSH_GATEWAY_OPERATOR_TOKEN"   "https://operator.example/v1/telemetry/fmans/$FMAN_PUBKEY/seats"

curl --fail-with-body   -H "Authorization: Bearer $PUSH_GATEWAY_OPERATOR_TOKEN"   "https://operator.example/v1/telemetry/fmans/$FMAN_PUBKEY/seats/$SEAT_ID/metrics"
```

The metrics response mirrors guardian status and content metadata. Do not log
either response or promote identifiers into metric labels. A 404 means no
current FMan target; 503 means the Iroh/FMan/guardian path is unavailable; 500
can mean ciphertext authentication failed.

Back up the telemetry database and AES key through separate protected channels.
If receiver state or its key is lost, restore the matching pair or wait for the
FMan's periodic idempotent registration to replace its target. Never bypass
credential verification or decryption.

By default the public listener exposes `/health`, `/live`, and API/hook routes,
but not `/ready` or `/metrics`. Production deployments should either set
`PUSH_GATEWAY_OPERATOR_BIND` and scrape/probe that trusted listener, set
`PUSH_GATEWAY_OPERATOR_TOKEN` and send `Authorization: Bearer ...` from the
operator/monitoring system, or use both. If `PUSH_GATEWAY_OPERATOR_TOKEN` is set
without `PUSH_GATEWAY_OPERATOR_BIND`, `/ready` and `/metrics` are mounted on the
public listener but fail closed with HTTP 401 unless the bearer token matches.
Unauthenticated public `/metrics` requires the explicit
`PUSH_GATEWAY_PUBLIC_METRICS_ENABLED=true` escape hatch; when that escape hatch
is enabled, public `/metrics` is intentionally unauthenticated even if
`PUSH_GATEWAY_OPERATOR_TOKEN` is also set, while `/ready` remains token-protected.

Every HTTP response includes `x-request-id`; an incoming safe `x-request-id` is
not trusted or propagated because public clients could put bearer material in
that header. Request logs use sanitized route templates (for example
`/hooks/{hook_id}/{hook_secret}`), not raw URLs, so hook secrets and query strings are
not emitted by the application. Do not configure reverse proxies to log full hook
URLs.

Hook invocation URLs use `/hooks/{hook_id}/{hook_secret}`: lookup uses the
public hook id, while the separate secret is hashed and compared against the
stored verifier. The hook id may appear in logs/metrics; the secret and full URL
must be redacted. This is a breaking public URL/API shape for callers using the
previous one-segment hook capability.

`POST /v1/hooks` returns one-time bearer material and therefore also sends
`Cache-Control: no-store` and `Pragma: no-cache`; preserve these headers at any
reverse proxy.

Metrics are in-process counters/gauges and reset on restart; the configured
database is the durable source of truth for queue and registration state.

## Delivery outbox admin CLI

Use `fedi-decentralized-push-gateway outbox ...` for dead-letter inspection and
explicit repair. The commands support SQLite and PostgreSQL through the same
`--database-url` / `PUSH_GATEWAY_DATABASE_URL` setting as the server and connect
without running migrations.

Read-only commands may run while the service is up only when the database has
already been migrated by the deployed service version; for consistency prefer a
quiet maintenance window. Mutating commands (`replay-dead-letter` and
`delete-dead-letter`) must be run with the gateway service stopped so there is
only one active process touching outbox state.

Dead-letter list, capped and sanitized:

```sh
fedi-decentralized-push-gateway \
  --database-url "$DB_URL" \
  outbox list-dead-letter --limit 50
```

The list output includes outbox id, event id, recipient id, installation id,
platform, attempts, sanitized `last_error`, and timestamps. Text output
JSON-escapes string fields to avoid terminal/control-character injection; add
`--json` for automation. Output intentionally does not print FCM tokens or
`notification_json` content.

Aggregate sanitized reasons:

```sh
fedi-decentralized-push-gateway \
  --database-url "$DB_URL" \
  outbox dead-letter-reasons
```

Replay selected dead-letter rows back to `pending` only while their original
five-minute delivery-resolution deadline remains in the future:

```sh
# Preview exactly what would be selected.
fedi-decentralized-push-gateway \
  --database-url "$DB_URL" \
  outbox replay-dead-letter \
  --reason provider_unavailable \
  --limit 25 \
  --dry-run

# Stop the gateway service, then explicitly confirm the bounded replay.
systemctl stop fedi-decentralized-push-gateway
fedi-decentralized-push-gateway \
  --database-url "$DB_URL" \
  outbox replay-dead-letter \
  --reason provider_unavailable \
  --limit 25 \
  --yes
systemctl start fedi-decentralized-push-gateway
```

You can also select exact rows with one or more `--outbox-id ...` arguments.
Every replay/delete mutation requires either explicit ids or a bounded `--limit`.
Without `--yes`, non-dry-run mutations are refused.

Rows past their original resolution deadline cannot replay: the CLI rejects both
the dry run and mutation rather than silently restarting accepted work. Inspect
the sanitized terminal reason, correct the underlying condition, then create a
new hook invocation if a new best-effort provider delivery is appropriate.

The server also performs automatic retention cleanup at startup. Configure the
horizon with `--retention-days` / `PUSH_GATEWAY_RETENTION_DAYS`; the default is
7 days. Cleanup deletes only sensitive terminal data older than the cutoff:
terminal `delivery_outbox` rows (`succeeded`, `invalid_token`, `dead_letter`),
old disabled registration rows, and old `notification_events` after no outbox
rows remain for the event. Pending, retrying, and in-progress delivery state is
never purged by retention cleanup. Keyed accepted invocations also leave a compact
`hook_idempotency_tombstones` marker (hook id, caller key, target count, accepted
time, and deadline). The marker survives sensitive event/outbox cleanup through the hook's lifetime
and seven additional days while the hook remains usable. Revocation/expiry
rejects replay, so terminal hook GC cascades the marker once retained events are
gone.

Admission lifecycle GC runs at the same startup boundary: registrations whose
`last_seen_at` exceeds `PUSH_GATEWAY_REGISTRATION_TTL_DAYS` are deleted, and
expired/revoked hooks with no retained notification event are deleted together
with their now-useless idempotency markers. Stale
registrations are excluded from hook creation/invocation even before restart.
Startup is not the only reclamation boundary: each registration or hook
admission transaction deletes up to `PUSH_GATEWAY_ADMISSION_GC_BATCH_SIZE`
eligible stale/terminal rows before checking its physical row ceiling. Watch
`push_gateway_registration_rows{state="total|registrations|token_owners|orphaned_token_owners|stale|disabled"}` and
`push_gateway_hook_rows{state="total|terminal"}`. Repeated 503 row-capacity
errors after bounded GC mean the operator must investigate retained references,
TTL/retention policy, and database growth rather than raising caps blindly.
The compact marker table is independently capped at the configured physical hook
row ceiling. New keyed invocation attempts reclaim one bounded expired batch and
then fail closed with `503 idempotency_capacity_exceeded` before consuming hook
counters or creating delivery state if the cap remains full. Repeated failures
require investigation of marker deadlines, terminal-hook references, and database
growth.
An `orphaned_token_owners` row temporarily preserves the token/stable-installation
binding after its refreshable route was reclaimed. Every valid signed registration
for the exact pair is authoritative, and the latest serialized commit atomically
owns both rows. Another still-valid clone may take ownership back later, so
operators may observe ownership oscillation until only one clone continues. A
different installation must use signed unregister or rotation to release it.

Caller idempotency keys are secret-adjacent correlation data. Protect the live
database, WAL, backups, restored copies, and operator access accordingly; do not
place those keys in logs, metrics, or routine admin output.

Manual dead-letter deletion is available for explicit, confirmed cleanup after
backups or compliance review when an operator needs to remove selected rows
before automatic startup retention would purge them:

```sh
fedi-decentralized-push-gateway \
  --database-url "$DB_URL" \
  outbox delete-dead-letter \
  --outbox-id OUTBOX_ID \
  --dry-run

fedi-decentralized-push-gateway \
  --database-url "$DB_URL" \
  outbox delete-dead-letter \
  --outbox-id OUTBOX_ID \
  --yes
```

Prefer replay over deletion unless the row is known to be unrecoverable and no
longer operationally useful.

## Deployment modes

### Nix/dev shell

Build the bare binary with Cargo or Nix:

```sh
cargo build -p fedi-decentralized-push-gateway --release
nix build .#push-gateway
```

Run from a deployment directory containing the SQLite database path:

```sh
PUSH_GATEWAY_DATABASE_URL='sqlite:///var/lib/push-gateway/push.sqlite?mode=rwc' \
PUSH_GATEWAY_PRODUCTION_MODE=true \
PUSH_GATEWAY_PUBLIC_BASE_URL='https://push.example.com' \
PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED=true \
PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG=10000 \
PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG=1000 \
PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED=false \
PUSH_GATEWAY_PROVIDER=fcm \
FCM_SERVICE_ACCOUNT_FILE=/run/secrets/push-gateway-fcm.json \
./target/release/fedi-decentralized-push-gateway \
  --bind 127.0.0.1:3000
```

Emergency restrictive admission uses the same production configuration except
that open mode is absent/false and the non-empty allowlist is set:

```sh
PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED=false \
PUSH_GATEWAY_ADMISSION_ALLOWED_RECIPIENTS='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' \
./target/release/fedi-decentralized-push-gateway --bind 127.0.0.1:3000
```

This fragment is an incident-mode override, not a complete deployment example;
retain every other production variable from the primary example and do not set
both admission modes.

### Bare binary/systemd

Run one service instance. Example unit sketch using PostgreSQL:

```ini
[Unit]
Description=Fedi decentralized push gateway
After=network-online.target

[Service]
User=push-gateway
Group=push-gateway
StateDirectory=push-gateway
Environment=PUSH_GATEWAY_BIND=127.0.0.1:3000
Environment=PUSH_GATEWAY_DATABASE_URL=postgres://push_gateway@localhost/push_gateway
Environment=PUSH_GATEWAY_PRODUCTION_MODE=true
Environment=PUSH_GATEWAY_PUBLIC_BASE_URL=https://push.example.com
Environment=PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED=true
Environment=PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG=10000
Environment=PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG=1000
Environment=PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED=false
Environment=PUSH_GATEWAY_PROVIDER=fcm
Environment=FCM_SERVICE_ACCOUNT_FILE=/run/secrets/push-gateway-fcm.json
ExecStart=/usr/local/bin/fedi-decentralized-push-gateway
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=multi-user.target
```

Mount FCM credentials with your secret manager and ensure logs are retained but
not shared with raw reverse-proxy URLs.

### Container

The flake provides a production OCI/Docker image package on Linux systems. Build
it on a Linux Nix builder, or cross-build a Linux gateway binary before changing
the image output to support other host systems:

```sh
nix build .#push-gateway-oci-image
IMAGE="$(nix eval --raw .#push-gateway-oci-image.imageName):$(nix eval --raw .#push-gateway-oci-image.imageTag)"
docker load -i ./result

# Equivalent helper app:
nix run .#push-gateway-container-load
```

The image is intentionally minimal: it contains the gateway binary, CA
certificates for outbound HTTPS to FCM, and minimal NSS files for the configured
non-root runtime user. It runs as uid/gid `65534:65534`, uses the gateway binary
as the entrypoint, sets only non-secret defaults through environment variables,
and bakes no FCM credentials, app ids, operator tokens, database passwords, or
other secrets into the image.

Run exactly one active gateway process, publish it only through a TLS reverse
proxy, and provide persistence and secrets through mounts/environment. SQLite is
acceptable for local/simple deployments with a persistent volume. PostgreSQL is
the production-oriented backend for hosted deployments and should be supplied via
`PUSH_GATEWAY_DATABASE_URL` from the orchestrator's secret/config mechanism.

SQLite example:

```sh
# For host bind mounts, create/chown the directory first:
# install -d -m 0750 -o 65534 -g 65534 /var/lib/push-gateway
docker run --rm \
  --name push-gateway \
  --user 65534:65534 \
  -p 127.0.0.1:3000:3000 \
  -v push-gateway-data:/var/lib/push-gateway \
  -v /secure/fcm.json:/run/secrets/fcm.json:ro \
  -e PUSH_GATEWAY_BIND=0.0.0.0:3000 \
  -e PUSH_GATEWAY_DATABASE_URL='sqlite:///var/lib/push-gateway/push.sqlite?mode=rwc' \
  -e PUSH_GATEWAY_PRODUCTION_MODE=true \
  -e PUSH_GATEWAY_PUBLIC_BASE_URL='https://push.example.com' \
  -e PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED=true \
  -e PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG=10000 \
  -e PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG=1000 \
  -e PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED=false \
  -e PUSH_GATEWAY_PROVIDER=fcm \
  -e FCM_SERVICE_ACCOUNT_FILE=/run/secrets/fcm.json \
  "$IMAGE"
```

PostgreSQL example:

```sh
docker run --rm \
  --name push-gateway \
  --user 65534:65534 \
  -p 127.0.0.1:3000:3000 \
  -p 127.0.0.1:9100:9100 \
  -v /secure/fcm.json:/run/secrets/fcm.json:ro \
  -e PUSH_GATEWAY_BIND=0.0.0.0:3000 \
  -e PUSH_GATEWAY_OPERATOR_BIND=0.0.0.0:9100 \
  -e PUSH_GATEWAY_DATABASE_URL='postgres://push_gateway:PASSWORD@postgres/push_gateway' \
  -e PUSH_GATEWAY_PRODUCTION_MODE=true \
  -e PUSH_GATEWAY_PUBLIC_BASE_URL='https://push.example.com' \
  -e PUSH_GATEWAY_OPEN_SELF_REGISTRATION_ENABLED=true \
  -e PUSH_GATEWAY_MAX_GLOBAL_OUTBOX_BACKLOG=10000 \
  -e PUSH_GATEWAY_MAX_RECIPIENT_OUTBOX_BACKLOG=1000 \
  -e PUSH_GATEWAY_LEGACY_NOTIFICATION_HOOK_ENABLED=false \
  -e PUSH_GATEWAY_OPERATOR_TOKEN='readiness-and-metrics-bearer-token' \
  -e PUSH_GATEWAY_PROVIDER=fcm \
  -e FCM_SERVICE_ACCOUNT_FILE=/run/secrets/fcm.json \
  "$IMAGE"
```

Prefer orchestrator-managed secrets over literal environment values for
`PUSH_GATEWAY_DATABASE_URL` when it contains credentials,
`PUSH_GATEWAY_OPERATOR_TOKEN`, and FCM service-account
material. If FCM is enabled, mount the service-account file read-only under
`/run/secrets` and point `FCM_SERVICE_ACCOUNT_FILE` at it. Host bind-mounted
SQLite directories must be writable by uid/gid `65534`; mounted FCM
service-account files must be readable by that user or group. Avoid
`FCM_SERVICE_ACCOUNT_JSON` in production because process environments are often
easier to expose than mounted secret files.

Multi-replica deployment is unsupported with any backend. PostgreSQL is the
required backend for any hosted production deployment, but this crate still does
not claim horizontally scaled operation until cross-process coordination is
designed and tested. If Kubernetes is used for experiments before that work, set
replicas to `1` and use persistent storage with appropriate backup.

## Backup, restore, and migrations

The configured database is the source of truth for registrations, hooks, durable
notification events, and outbox state.

SQLite backup:

1. Prefer `sqlite3 /var/lib/push-gateway/push.sqlite ".backup '/backup/push.sqlite'"`.
2. Include WAL/SHM consistency by using SQLite backup API or stopping the service
   before filesystem snapshots.
3. Encrypt backups because they include push-token snapshots and notification
   content.

PostgreSQL backup:

1. Use the operator's standard PostgreSQL backup mechanism, such as
   `pg_dump --format=custom`, physical base backups, or managed-service snapshots.
2. Include migration metadata and all gateway tables.
3. Encrypt backups because they include push-token snapshots and notification
   content.

Restore:

1. Stop the gateway.
2. Restore the SQLite file or PostgreSQL database with owner/permissions
   appropriate for the service user.
3. Start one gateway instance and verify `GET /ready` and queue depths.
4. Expect only pending/retrying outbox rows still within their persisted
   resolution deadline to retry; provider sends are at-least-once, so duplicate
   pushes are possible after restore. The first recovered worker dead-letters
   overdue active rows with `resolution_deadline_exceeded`.

Migrations run at startup. Schema changes are additive migrations and should be
tested against a copy of production data before deploying once any persistent
deployment exists. If migration fails, stop the service, preserve the failed
database and logs, restore the last backup if needed, and do not repeatedly
restart against the same partially inspected state until the cause is understood.

## Operational playbooks

### Invalid FCM credentials or auth failures

Symptoms: startup fails in `PUSH_GATEWAY_PROVIDER=fcm`, readiness is unavailable,
or delivery rows retry with auth/provider errors.

1. Verify `PUSH_GATEWAY_PROVIDER=fcm` and exactly one credential source is
   intended.
2. Validate the mounted service-account JSON out of band without printing it to
   logs.
3. Rotate/redeploy credentials from the secret manager.
4. Watch `/ready`, `/metrics`, and retrying/dead-letter queue depth.

### FCM outage, quota, or network failure

Symptoms: `push_gateway_outbox_rows{status="retrying"}`,
`push_gateway_outbox_retrying_oldest_age_seconds`, or
`push_gateway_outbox_oldest_due_age_seconds` grows and delivery failure counters
increase. Use `push_gateway_provider_outcomes_total{reason_class="quota"}` and
`reason_class="network"` to distinguish quota and network pressure; auth
failures usually mean credential/configuration problems.

1. Check FCM status/quota and network egress.
2. Keep the gateway running so retry backoff can drain after recovery.
3. Consider temporarily reducing hook creation/use externally if queue growth
   threatens disk capacity.
4. Do not delete registrations for transient provider errors.

### Outbox queue growth

1. Inspect `/ready` and `/metrics` status counts and oldest due/pending age
   gauges. If `/metrics` returns HTTP 500 with
   `push_gateway_metrics_scrape_db_error 1`, investigate database reachability
   or schema state before trusting queue-depth dashboards.
2. Confirm worker running state is `1` and no single process is overloaded.
3. Increase `PUSH_GATEWAY_OUTBOX_WORKER_CONCURRENCY` cautiously if FCM quota and
   host resources allow.
4. If `dead_letter` grows, watch `push_gateway_outbox_dead_letter_rows` and
   `push_gateway_outbox_dead_letter_total`, then use
   `fedi-decentralized-push-gateway --database-url "$DB_URL" outbox dead-letter-reasons`
   and a bounded `outbox list-dead-letter --limit ...` to inspect sanitized
   metadata without exposing FCM tokens or notification content.
5. After the underlying provider/config/payload issue is resolved, stop the
   gateway, dry-run a bounded replay, then rerun with `--yes` and restart the
   gateway. Expired rows cannot replay; create a new invocation only when a new
   best-effort delivery is appropriate. Watch `/ready` and `/metrics` for queue
   drain.

### DB migration failure

1. Stop the service and keep the failed database plus logs for investigation.
2. Restore backup if service must return quickly.
3. Reproduce against a copied DB before attempting repair.
4. Do not run multiple versions of the service against the same database.

### Disabling bad tokens or hooks

- Bad registrations can be disabled with
  `POST /registrations/{installation_id}/disable?reason=...`.
- Bad hooks can be revoked with
  `DELETE /v1/hooks/{hook_id}`.
- These management endpoints require Nostr auth and are scoped to the signer
  recipient.

### Secret rotation

- FCM credentials: deploy the new secret file/env, restart the single process,
  and verify delivery. Delete old credentials from the secret manager.
- Hook secrets: there is no rotate endpoint yet. Create a replacement hook, move
  external callers, then revoke the old hook.
- Recipient auth keys: derive a new app recipient key from the app root-secret
  model and re-register installations/recreate hooks as needed. Server-side
  in-place recipient-key rotation is not an MVP feature.
