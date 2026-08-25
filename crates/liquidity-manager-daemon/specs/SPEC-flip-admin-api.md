# SPEC-flip-admin-api: Operator Admin API contract

## Record justification

The Admin API contract binds the daemon's HTTP adapter and admin service to
the external operator console maintained outside this repository, so no
single implementation artifact can own it coherently.

The Admin API is a private HTTP/JSON surface on a dedicated local/private
port, never advertised over Nostr and separate from the Public Liquidity API.
It is the canonical operator automation surface: the bundled web client and
any future CLI must use it rather than reading local storage or calling
daemon internals. The canonical request/response shapes are the Rust DTOs in
`crates/service-liquidity-manager/src/admin.rs`, served by the
`OperatorAdminApi` trait; this record does not restate the route list.

Nix release builds embed the FLIP dashboard in the daemon binary and serve it
from the Admin API listener; other builds serve only the API. Static and SPA
routing comes from the same `operator-ui-static` crate FMan uses, while the
FLIP daemon selects its own dashboard assets and reserves `/admin` and
`/health` from SPA fallback. The static router is merged outside bearer-token
middleware: `/`, `/index.html`, and `/assets/*` are public on this private
listener so the dashboard can load before the operator enters a token. This
does not make any `/admin/*` route public.

`GET` and `HEAD` serve exact embedded files; extensionless browser-navigation
paths fall back to `index.html`. Missing `/admin/*`, `/health/*`, `/assets/*`,
and mock-control paths return 404 rather than HTML. The shell and non-hashed
files use `Cache-Control: no-store`; Vite's content-hashed assets use
`Cache-Control: public, max-age=31536000, immutable`. Static responses carry
their media type and `X-Content-Type-Options: nosniff`, and are gzipped when the
client accepts that encoding. No proxy sits between the dashboard and this
listener, so the daemon owns compression for its own assets.

## Transport, auth, and errors

Routes are `POST /admin/v1/<method>` with JSON bodies. `GET /health` is the
unauthenticated process-shell liveness route: it remains available before a
normal-mode runtime generation finishes starting and while a live restore is
between generations. `GET /admin/health` / `POST /admin/v1/get_health` are the
protected health routes. In normal mode they are runtime-backed, so they have
the same response shape but return `unavailable` until a generation is
installed and during a live-restore swap. In restore-only mode they instead
serve the authenticated restore-shell health response without a generation.

`GET /health` is unauthenticated, so it discloses statuses and no free text.
The response carries `overall_status`, `mode`, and each component's name,
status, and observation time; every `detail` string is withheld. The protected
verbs carry the same document with details intact. The details are formatted
from the daemon's own configuration and observations — database path, admin and
public bind addresses, Iroh node id, auth and verification modes, and the
latest spendable wallet balance — so the boundary is drawn at the whole `detail`
channel rather than at a list of fields that would need re-auditing whenever a
caller appends one more.

`mode` is the operational fact that has to survive that redaction: it reports
`normal`, `restore`, `reloading`, or `no_runtime`. A client needs it to choose
a console before it has authenticated, and during a live restore swap no
authenticated route answers to supply it.
Protected routes require `Authorization: Bearer <admin-token>`; the token
identifies the whole installation. The daemon must bind to a local/private
interface unless the operator adds a separate access-control layer. Service
failures return the `ServiceError` envelope (`{"code","message"}`) with the
fixed mapping:
`invalid_argument` → 400, `permission_denied` → 403 (401 for missing/invalid
bearer), `not_found` → 404, `failed_precondition` → 412, `unavailable` → 503,
`internal`/`unknown` → 500.

List routes use `PageRequest`/`ListResponse` (opaque cursor, limit clamped to
1..=100, inclusive-from/exclusive-to time ranges, newest-first with id
tie-breakers). Admin responses may include private federation details for
operator debugging, but never service secrets: secret-bearing views return
indicators such as `has_admin_credential`, and raw gatewayd credentials,
bitcoind passwords, and provider identity secrets must not appear in
responses or logs.

Secrets are also not carried *into* the daemon inside the configuration they
belong to. They are written by name, one at a time, with an explicit set or
clear, and a configuration write can neither store nor remove one. A config
write states the whole configuration, so a secret inside one has an absent case
that has to mean something — and a client cannot restate a secret it was never
allowed to read. Every interpretation of that absence is wrong: reading it as
"remove" destroys a working secret when an operator edits an unrelated field,
and reading it as "required" forces them to retype a secret their edit does not
concern. An empty value is refused rather than treated as a removal, and a
secret the daemon cannot operate without cannot be cleared at all.

## Boot modes

Normal mode serves the full surface, including `restore_backup` on the running
daemon. Restore mode remains for the disaster case — a fresh host with an empty
data dir — and exposes only health, `inspect_backup`, and `restore_backup`,
with normal services, SQLite handles, and public transport down while the
archive is extracted. Every other `/admin/v1/*` route answers `unavailable`,
naming the mode: the verb exists and returns when the restore finishes, and a
client cannot distinguish a bodiless 404 from an unreachable daemon. A
restored deployment completes validation, confirms the original instance is
offline, and restarts into normal mode before resuming work or advertising.
Deferred-but-specified methods must be route-complete with stable
machine-readable errors, never silent 404s.

## Operational semantics

Hot-reloadable settings (relays, advertisement refresh, display/contact
metadata, attester policies, replenishment thresholds, capacity, funding
policy, public-ready enablement, the configured gatewayd wallet, chain
observer, and wallet network) are validated first, persisted atomically,
and republish/withdraw/leave the advertisement per the resulting readiness;
reload never interrupts in-flight accepted allocations, and funding-policy
changes never rewrite settlement requirements for already-submitted
operations. Display/contact patches distinguish no-change, replacement, and
explicit clearing.

Provider identity is installed through the API on a running daemon and is
install-only: a key disagreeing with the installed one is refused, because
provider-key rotation is out of scope. Installing it is what makes an
authorization request showable, so the enrollment routes report the provider
pubkey and the current enrollment state, and reconcile Holder authorizations
against the configured relays on operator request rather than continuously
([SPEC-flip-holder-authorization](./SPEC-flip-holder-authorization.md)).
Reconciling with no relays configured is a precondition failure; a partial
answer, where some relays failed, is a success that names them.

Attestation install carries operator-configured trust policy only, meaning an
issuer authority. Trust material about this provider is never uploaded, so a
Holder authorization or a credential is refused rather than stored where
nothing would read it. Installing policy needs no provider identity, because
the document describes an issuer rather than this provider. The Admin API bearer token is
rotatable, and a rotated token supersedes the boot bootstrap token outright.
A target federation's Fedimint client can be closed so the next use reopens
it, releasing its database lock in between.

A rotated token supersedes the bootstrap token outright, including when it
cannot be read: unreadable secret storage locks the authenticated surface
rather than falling back, so a storage failure cannot resurrect a retired
credential. Recovering from that is a boot-time decision, not a request-time
one — a break-glass argument re-enables the bootstrap token, and using it
therefore requires control of the deployment rather than only reach to the
port. Storage that is merely unavailable for a moment, as during a live
restore, reports `unavailable` instead and needs no such intervention.

Only deployment shape — the
Manifold environment, trust-fixtures mode, data-directory and database
location, and listener bindings — requires a restart.

Restoring a backup onto a running daemon replaces every durable thing it owns,
so it is specified as a runtime replacement rather than a settings reload: the
archive is extracted and validated while the current runtime keeps serving, and
only an archive that passes is applied. The runtime is then torn down in full —
workers stopped, federation clients closed, database closed — before the data
dir is replaced, and rebuilt afterwards through the same startup path a restart
takes, including startup recovery. The process, its data-directory lock, and
the Admin API listener persist across the swap; runtime-backed admin routes
report `unavailable` during it while the shell `GET /health` route reports the
restore. State the restore displaces is retained, not deleted, and a restored
state that fails to start is rolled back to it. Because the public node id is
derived from the provider identity, an archive carrying a different identity
than the running daemon is refused; installing another provider's state is
what restore mode is for.

Live restore must also preserve every allocation identity already accepted by
the running generation. Archive validation compares federation, requester,
provider, network, and details commitment under the same acceptance fence that
guards new allocation commits. An archive that omits or replaces one of those
identities is refused before teardown, and the current generation keeps serving.
Each runtime generation admits at most one restore: a concurrently staged
request whose captured generation is already closing is refused rather than
replacing the pending archive.
This check uses the live generation as the cross-generation authority. Fresh-host
restore mode has no newer generation to consult, so it does not claim rollback
detection and remains an operator-controlled disaster-recovery boundary.

Restore validation separates two questions. Whether the world is ready —
dependencies reachable, config valid — stays advisory, because a
disaster-recovery host has its dependencies down and must still be restorable.
Whether the daemon can open the payload at all is a precondition: an archive
whose secret records do not decrypt under the key this daemon will use is
refused before anything is replaced, since it would otherwise come up unable to
read the admin token and lock the operator out. Restoring is a state
replacement, so the admin token in the archive supersedes the running one.

The advertised endpoint address is derived daemon state, not operator input:
the public transport key comes from the provider identity, so the node id is
stable across restarts, and the daemon records its own node id as the
advertised address rather than failing readiness until an operator supplies
it. The public transport therefore binds only once a provider identity
exists.

Three verbs are deliberately reachable from the CLI only, and no browser
surface is planned for them: `install_provider_identity`, `rotate_admin_token`,
and `reopen_federation_client`. The first two carry secret material in the
request body — a provider signing key and an admin bearer token — and a browser
is the wrong place to paste either: it lands in form state, in autofill, and in
whatever the page's history retains. The third is runtime surgery on a
federation client rather than configuration, and it is used while diagnosing,
which is when a typed command with an explicit federation id is safer than a
button. **The API accepts all three from any authenticated caller**; what this
records is that the operator UI does not offer them, so their absence there is a
decision rather than an omission.

Withdrawal idempotency (`withdrawal_intent_id`) and the
`retry_funding_step` / `cancel_allocation` guards are specified in
[SPEC-flip-funding-safety](./SPEC-flip-funding-safety.md); manual operations
report `accepted`, `rejected`, `not_found`, or `already_applied` in the
response `status`, not as transport errors. Backups are unencrypted tar.gz
archives of the data directory (paths relative to its root, backup outputs
excluded), created and inspected through the API with a derived
`BackupManifest` that restore validates against extracted contents rather
than trusting; because archives contain identity secrets and possibly the
local secret-store key, they are themselves secret material.

An archive also carries `backup-checksums.json`, a SHA-256 digest per archived
file, written last so each digest covers exactly the bytes the archive holds.
Restore recomputes every digest from the extracted files and refuses the archive
on a mismatch, on a recorded file that did not arrive, or on an extracted file
that was never recorded. This detects corruption only. The digests travel inside
the archive they describe, so they establish nothing about who wrote it, and a
self-consistently wrong archive — one whose payload was captured torn, since the
walk reads live SQLite and Fedimint client files — digests exactly as torn.


## Alternatives

Copying missing allocation rows into an older archive would preserve the
identity but combine funding progress and external-client state from different
recovery points. A separate append-only authority store would also fence
rollback, but it would introduce another durability and disaster-recovery
contract. Refusing a regressing live restore is smaller: the running generation
already supplies authoritative accepted identities, and refusal leaves its
coherent state untouched for operator recovery.
