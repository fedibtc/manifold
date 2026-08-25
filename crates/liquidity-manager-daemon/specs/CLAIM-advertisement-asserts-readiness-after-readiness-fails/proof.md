# Current argument

## Argument

1. **Before persistence, under the writer.** `begin_write` is `BEGIN IMMEDIATE`,
   so no other writer can commit between the recheck's reads and the insert.
   This fences every database-backed readiness input.
2. **Immediately before the commit**, comparing the in-memory snapshot the
   recheck judged against a fresh read. `daemon_state` takes no SQLite lock, so
   the writer does not hold it; comparing the values does.
3. **Before every relay publish**, not once before the loop. By A1 the published
   event is the assertion, so each relay is a separate assertion reached after a
   separate network round trip.

The in-memory inputs — daemon phase, recovery, endpoint identity, signing
readiness, verification inputs — are compared **by value**, not tracked by a
generation counter. A counter is correct only while every writer remembers to
bump it, and `daemon_state` has four writers plus the auth-provider swap.

**L2 (`enum`) — readiness dependencies can change concurrently.**
Regenerating the callers of `reconcile_after_config_change` in
[`admin.rs`](../src/admin.rs) gives six admin verbs that commit
and then trigger publication: `apply_setup_config`, `update_provider_config`,
`install_provider_identity`, `attestation_install`, `attestation_remove`, and
`refresh_holder_authorizations`. `public_readiness` reads mutable setup state
and the enrolled Holder authorizations
([`setup_store.rs`](../src/setup_store.rs),
[`holder_authorization.rs`](../src/holder_authorization.rs),
[`advertisement.rs`](../src/advertisement.rs)).

Only some of those six can move readiness from true to false, which is what L3
needs. `apply_setup_config` and `update_provider_config` can, by disabling ready
advertisement publication or invalidating the endpoint or relay configuration.
`install_provider_identity` is install-only and refuses a disagreeing key, so it
cannot. `refresh_holder_authorizations` only inserts, or replaces a row for one
credential digest with a strictly later-dated authorization, so the enrolled set
never shrinks through it. `attestation_remove` deletes `attestation_payloads`
rows, which readiness no longer reads for the envelope check, so it no longer
empties the envelope set either; it still triggers publication, which now has no
readiness consequence. `restore_backup` replaces the whole data directory and so
can empty the enrolled set, but it tears the runtime down rather than racing this
publisher, and is treated under the live-restore fence rather than here.

The two routes L2 admits are database-backed (`apply_setup_config` and
`update_provider_config`), so L1's first check covers both. The in-memory inputs
are not reachable from L2's admin verbs at all — they move from the daemon's own
lifecycle and from `install_provider_signing_identity` — and L1's second and
third checks cover them.

## Residual windows

- An event published before readiness fails may remain discoverable until expiry;
  this is the separately documented no-op withdrawal limitation in
  `SPEC-flip-advertisement`, not a fresh publication claim.
- Relay delivery failure does not establish readiness and is outside this
  truthfulness claim.

## Weakest links

1. **L3 (`code`)** — publisher/Admin race.
2. **L2 (`enum`)** — the writer set is regenerated per check, not enforced; a
   new admin verb that can falsify readiness would land silently.
3. **L1 (`code`)** — absent readiness revision fence.
4. **A1–A2 (`axiom`)** — public meaning and scheduling.
