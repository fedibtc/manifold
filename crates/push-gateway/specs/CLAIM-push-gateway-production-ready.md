# CLAIM-push-gateway-production-ready: Push Gateway is ready for production

In the documented production mode, an authenticated recipient can create,
manage, and revoke only its own bounded bearer hooks and registrations. Every
accepted invocation is durably enqueued at most once per idempotency key,
survives restart, and resolves each snapshotted active target to provider
acceptance (not application receipt), permanent invalid-token handling, or an
observable actionable dead-letter state. Public callers, logs, errors, metrics, backups, and unauthorized
management callers do not disclose credentials, tokens, hook secrets, recipient
identity, or notification contents outside the authenticated recipient, the
configured FCM provider, and the protected persistence and operator boundary.
Under the release's dependency-availability preconditions and workload limits,
each snapshotted active target of an accepted invocation reaches provider
acceptance, permanent invalid-token handling, or an observable actionable
dead-letter state within the documented delivery deadline, and restart or
restore recovers service within the documented recovery objective.

## Assumptions

- In the documented production mode, the Push Gateway's management,
  invocation, durable-state, provider-outcome, and recovery contracts ensure
  that an authenticated recipient can create, manage, and revoke only its own
  bounded hooks and registrations; that every accepted invocation has one atomic
  durable admission that snapshots the hook's active installation target, and
  all accepted attempts with one idempotency key in its documented namespace
  share that one admission; that the snapshotted target reaches provider acceptance, permanent invalid-token
  handling, or an observable actionable dead letter within the documented
  delivery deadline under the release's stated workload and
  dependency-availability preconditions; that restart or documented restore
  recovers service and this durable state within the recovery objective; and
  that its public, persistence, operator, and observability interfaces disclose
  credentials, bearer hook URLs and secrets, registration tokens, recipient
  identity, and notification contents only to the authenticated recipient,
  configured FCM provider, and protected persistence and operator boundary.
- The deployment enables production mode, real FCM delivery, an HTTPS public
  origin, exactly one of open authenticated self-registration or a nonempty
  emergency recipient allowlist, backlog caps, protected operator endpoints,
  and exactly one active gateway process per database. The release
  states workload limits, dependency-availability preconditions, operation and
  delivery deadlines, and a recovery objective.
- The production database and its backups provide the documented transaction,
  durability, and confidentiality properties. Operators follow the documented
  migration, backup, restore, and stopped-service outbox-administration
  procedures.
- The TLS terminator and reverse proxy preserve exact public URL and source
  semantics, replace untrusted forwarding headers, and do not disclose
  credentials, bearer hook URLs and secrets, registration tokens, recipient
  identity, or notification contents through their callers, logs, errors,
  metrics, or operators outside the protected boundary; clocks satisfy the
  NIP-98 freshness and delivery-retry assumptions.
- The configured FCM provider includes the OAuth credential service used to
  authorize FCM. The operator accepts that provider's processing, retention,
  and confidentiality terms for registration tokens, OAuth credentials, and
  notification payloads. That provider implements its documented response and
  credential semantics.
- Recipient signing keys, Firebase credentials, database credentials, and
  backups remain confidential. Possession of a bearer hook URL authorizes its
  invocation.
