# push gateway notes

- Read [`./SECURITY.md`](./SECURITY.md) before changing HTTP handlers, registration persistence, credentials, or test-resource behavior.
- Keep [`./README.md`](./README.md) synchronized with endpoint, configuration, persistence, and `defe` resource behavior.
- Product direction is webhook-to-mobile-push: mobile app users create shareable HTTPS hook URLs that external callers invoke to trigger mobile push notifications.
- Treat hook URLs as bearer capabilities. Possession of a valid, unexpired, unrevoked URL authorizes hook invocation.
- Never log raw hook URLs or hook tokens. Store hook tokens only as hashes.
- Generated production hook URLs must use a validated absolute `https://` origin;
  only explicit local/test paths may use empty or loopback HTTP origins.
- `POST /v1/hooks` intentionally returns one-time bearer material and must keep
  no-store/no-cache response headers.
- Prefer stateful server-side hook records because hooks need identification, rate limiting, TTL, revocation/deletion, and workflow/deep-link context.
- Keep [`./README.md`](./README.md) and [`./SECURITY.md`](./SECURITY.md) synchronized when changing hook behavior, registration persistence, notification delivery, credentials, or public HTTP endpoints.
- Read [`./testing.md`](./testing.md) before changing tests, push-provider
  behavior, hook invocation, registration persistence, or `defe` resource
  behavior, and [`SPEC-hook-invocation`](./specs/SPEC-hook-invocation.md)
  before changing hook consistency semantics.
- Read [`./OPERATIONS.md`](./OPERATIONS.md) before changing readiness, metrics, runtime configuration, deployment assumptions, backup/restore behavior, or operational playbooks.
- Production multi-process / multi-replica operation is not supported yet. Exactly one active gateway process, including exactly one active outbox worker, may run against a configured database.
- Management/registration auth is Nostr/NIP-98 signed requests. Recipient identity
  is the canonical lowercase hex pubkey from the signed event; do not reintroduce
  request `app_id` equality or caller-supplied `recipient_id` as auth.
- Future changes touching hook invocation, idempotency, rate limiting, outbox claim/reset, or worker startup must not imply HA or horizontally scaled production support unless they also add an explicit database coordination design and tests.

- Read [`SPEC-hook-invocation`](./specs/SPEC-hook-invocation.md) and
  [`ARCH-push-gateway`](./specs/ARCH-push-gateway.md) before changing durable
  hook invocation, outbox persistence, idempotency, or worker lifecycle
  behavior.
- Preserve crate boundaries documented in [`ARCH-push-gateway`](./specs/ARCH-push-gateway.md):
  keep DTO/value types in `push-gateway-types`, SQL/migrations in
  `push-gateway-storage`, provider traits in `push-gateway-provider`, Google/FCM
  HTTP/OAuth code in `push-gateway-provider-fcm`, and Axum/config/orchestration in
  this server crate.
