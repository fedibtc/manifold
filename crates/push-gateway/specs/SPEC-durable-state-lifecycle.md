# SPEC-durable-state-lifecycle: Push durable-state lifecycle

## Record justification

The lifecycle crosses recipient-authenticated registration and hook APIs, public hook acceptance, background provider delivery, retention cleanup, stopped-service operator repair, and storage migrations, so no one implementation artifact coherently owns every transition or the backup boundary.

## Durable-state inventory

The storage migration set owns exactly these product-state tables. The
`durable_state_inventory_matches_persisted_tables` storage test fails if a new
table appears without this inventory being revised.

| State | Creation and live transition | Terminal or removal transition | Retention |
| --- | --- | --- | --- |
| `push_registrations` | An authenticated recipient registers or refreshes one installation. A matching recipient may re-enable a disabled installation. | The owner explicitly unregisters it, a token-specific provider rejection disables it, or stale GC removes it after the refresh cutoff. | Active rows remain while refreshed. Disabled and stale rows are removed after their configured horizons. |
| `push_registration_token_owners` | Registration admission binds each globally unique provider token to its live recipient and stable installation. | Signed unregister or rotation releases it. Stale GC removes it after its registration is gone and its own update timestamp passes the registration cutoff. | Owner rows count with registration rows under one physical ceiling. |
| `push_gateway_admission_locks` | Migrations create the fixed `registration` and `hook` mutex rows. Admission transactions update them to serialize count-and-insert decisions. | They have no product lifecycle or removal path. | The two fixed rows remain for the database lifetime. |
| `notification_hooks` | An authenticated recipient creates a finite bearer hook targeting one active owned installation. Use/rate-limit fields evolve with accepted invocation. | The owner may revoke it; expiry also prevents future acceptance. Terminal hooks are deleted after retained events no longer reference them; deletion cascades now-useless idempotency markers. | Active hooks remain until expiry or revocation; terminal event-unreferenced rows are reclaimed in bounded admission and startup cleanup. |
| `notification_events` | The public invocation transaction records an accepted event and optional idempotency key. | An event has no separate status: its target outbox rows determine whether it remains operationally relevant. | The event is removed only after the retention horizon and after every outbox row has gone. A zero-target event follows the same horizon. |
| `delivery_outbox` | The acceptance transaction writes one `pending` row for the hook's active target snapshot. | `succeeded`, `invalid_token`, or `dead_letter` are terminal. `dead_letter` is an actionable operator outcome. | Active `pending`, `retrying`, and `in_progress` rows are never retention-purged. Terminal rows are purged after the configured horizon. |
| `hook_idempotency_tombstones` | The acceptance transaction records a compact hook/key marker and prior target count. | Retention cleanup removes a marker after its finite hook lifetime plus the cleanup margin. Terminal hook GC removes it earlier once retained events are gone because revocation/expiry already prevents replay. | Markers retain no notification or token snapshot and have an independent bounded physical ceiling. |

The database and its backups are the persistent boundary. Process-local rate
limit counters, NIP-98 replay cache entries, worker wakeups, provider OAuth
tokens, and metrics are intentionally ephemeral and reconstructible; they are
not durable product state.

## Accepted delivery resolution

For every target snapshotted by a durably accepted hook invocation, success means
the configured FCM provider accepted the notification request. It does not mean
the mobile operating system displayed it, the app ran a handler, or a person saw
it. The gateway has no application receipt protocol.

Each target has an absolute five-minute resolution deadline measured from the
acceptance transaction's durable `created_at`. The deadline survives process
restart. A responsive worker and storage backend resolve each target by then to
one of:

- `succeeded` — the provider accepted the notification;
- `invalid_token` — the provider permanently rejected the snapshotted token and
  the still-matching registration was disabled atomically; or
- `dead_letter` — the provider permanently rejected the payload, transient
  failures exhausted five attempts, serialized notification data was invalid, or
  `resolution_deadline_exceeded`.

The worker bounds each provider call to 15 seconds and also limits it by the
remaining absolute deadline. A delayed recovery cannot restart the clock: the
first recovered worker marks active rows whose persisted deadline passed as
`dead_letter` with `resolution_deadline_exceeded`. Database or process
unavailability falls under the release recovery objective; it does not turn
accepted work into a successful delivery.

Storage evaluates every deadline decision in the mutation statement, rather
than in a process-local preliminary read. A replay rolls back its full selected
set when its statement-time deadline predicate rejects one row.

The storage tests enforce that deadline expiry terminally resolves each active
outbox state without incrementing provider-attempt count, and that a provider
failure at the deadline cannot schedule another retry. The retention test
enforces that cleanup removes only documented terminal data.
