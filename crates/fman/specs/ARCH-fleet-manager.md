# ARCH-fleet-manager: Fleet Manager daemon

The FMan daemon is one process that hosts up to its durable operator-configured
guardian-seat capacity,
spawning a `fedimintd` child for every non-decommissioned seat. The daemon exposes
five surfaces: an FI-facing RPC service over iroh, an operator-facing admin
socket (local unix socket), an optional authenticated browser operator surface,
an outbound Nostr integration, and an outbound DKG completion-callback
integration. It persists to one SQLite database in the data root.

## Crate layering

The component is split into crates under `crates/fman/`. Current Linked Specs records and their proofs live under `specs/` and cover properties that cross these crate boundaries.

```
crates/fman/
  specs/              current records and companion evidence
  core/               package fman-core
  cli/                package fman-cli (operator admin client)
  fedimint/           package fman-fedimint
  nostr/              package fman-nostr
  bin/                package fman (ships the `fleet-manager` binary)
```

`core` is the daemon: seats, storage, allocation, identity, guardian-fee
policy, and the RPC and admin surfaces. What an FMan *talks to* outside its
bundled child is a set of traits it defines and does not implement —
`EcashWallet` and `GuardianFeeVault` (`wallet`, `guardian_fee`) and
`BackupSink` and `BackupArchive` (`backup`) — so its dependency tree does not
grow when one of those capabilities is added. The bundled-child boundary is
different: `core` depends directly on `fedimint-server` for the driven-DKG
parent client and wire vocabulary. `fedimint-server` is already shipped through
the binary's bundled `fedimintd`, and one shared protocol definition prevents
the two ends from drifting.

`fedimint` and `nostr` depend on `core` and fill those holes. `cli`
owns the operator admin command tree and its standalone local-socket client
binary. `bin` is a binary and nothing else: daemon argument parsing, the
bundled-`fedimintd` argv[0] entry, optional release-time embedding of the
operator dashboard, and the wiring that hands `core` its implementations.
Nothing depends on it, which is
what makes it the only place a new capability's dependency lands. DKG callback
durability and origin policy therefore live in core behind
`CompletionCallbackInvoker`; `bin::push_callback` implements that hole and alone
imports the push-gateway DTO and outbound HTTP/TLS features.

The FI (payer) side of the key-locked payment protocol is not here at
all: `fi-cli` carries it with its own wallet, and nothing outside the FMan
depends on `fedimint`. What both sides must compute identically — the
per-generation cryptography, denomination selection, and the refund
preparation the payee re-runs as its validation oracle — lives in the
shared `locked-payments` crate (`crates/locked-payments`)
so agreement rests on one definition rather than two copies.

**A trait here is exactly a hole**: it exists when the implementor lives in a
crate `core` cannot name, and for no other reason. The traffic that runs the
other way — what a runtime reads *out* of the daemon, like the advertisement
snapshot and the retained setup-payment policy — needs no trait, because the
runtime depends on `core` and calls `FleetNostrHost` and
`FleetSetupPaymentPolicyStore` directly. A trait whose only implementor is
`core` itself is indirection wearing the costume of inversion, and the tell is
that its other implementors are all test doubles.

A hole is also specifically something the daemon needs *done*. What it needs
to *know* from a runtime is a value, and travels as one: the operator socket
reads `directory::DirectoryPresence` off a `watch` channel the runtime
publishes, which is why it cannot block on a relay even in principle. The
operator's explicit Holder-enrollment refresh travels in the other direction
through a runtime-supplied callback: the admin operation schedules work and
returns rather than holding its local connection across relay I/O.

## Module responsibilities

Modules below are `core`'s unless named otherwise. Dependency
direction is strictly bottom-up; each layer only knows the ones below it.

- `identity` — derives every key from the install's BIP-39 root mnemonic
  ([ARCH-fleet-manager-identity](./ARCH-fleet-manager-identity.md)).
- `facts` — the durable seat vocabulary shared by storage and everything
  above it: seat ids, creation facts, formed-seat facts, and port-block
  addressing.
- `db` — SQLite pool, migrations, identity row, operator settings and offer
  generation, and the durable per-seat lifecycle facts, typed accepted-payment
  evidence, terminal claim observations, callback delivery work, and immutable formed seats
  ([ARCH-fleet-manager-storage](./ARCH-fleet-manager-storage.md)).
- `seat_process` — the process boundary for spawning, containing, stopping, and
  waiting on a `fedimintd` child. The bundled, pinned `fedimintd` is part of
  FMan's trusted computing base: its process boundary isolates lifetime and
  crashes, not a malicious child. The hosting model (purposeful
  subprocess-per-seat, daemon-coupled lifetimes)
  is [ARCH-fleet-manager-seat-processes](./ARCH-fleet-manager-seat-processes.md); its module doc
  states the local design and the guarantees the rest of the daemon
  relies on (exit kills children, stop reaps before returning, child respawn
  continues unless a failed stop leaves exit unproved, wipe-safe disk layout).
  On Linux, one process-lifetime
  OS thread creates every child so the kernel parent-death signal is bound
  to the daemon lifetime rather than to an ordinary async-runtime worker.
  The seat loop directly owns the returned process and
  `fedimint-server` parent client and consumes each child's initial state,
  lifecycle events, and exit alongside commands and timers. Tests select an
  in-process child at that process boundary; it owns both the task speaking the
  same framed protocol and a real loopback WebSocket JSON-RPC server, and tears
  both down with the scripted child lifetime.
- `backup` — the recovery data and its sink boundary
  ([SPEC-nostr-backup-restore](./SPEC-nostr-backup-restore.md)). It owns the
  *only* assembler of a seat's publication — its document and, until
  confirmed, the guardian archive the document names — over `db` and the
  seat's data directory: backup events are addressable, so a publication
  replaces the last one, and a document built from what a call site happened
  to hold would erase what an earlier publication made durable.
  `backup_worker` decides *when* a seat is published — by comparing the
  assembled document's hash against the seat's last confirmed publication —
  and can express neither a partial document nor a different order. The relay
  storage format — event families, sealing, archive slicing, padding,
  blinded coordinates, the schema version — and the relay mechanics both belong to
  `nostr`, behind two traits this module defines: `BackupSink` for
  publication, `BackupArchive` for the restore read-back. They are separate
  because they are used at different times by different callers — the archive
  is read before any fleet exists, under the phrase the operator just typed.
- `seat` — one seat's command-and-snapshot handle, its single-owner
  lifecycle loop, and every verb that acts on that seat, over the
  vocabulary `facts` defines.
- `fedimint_api` — the seat-facing adapter over Fedimint's native
  `DynGlobalApi`. One server-default `ConnectorRegistry` is bound when the
  fleet opens and shared by every seat; consensus status and `meta` calls use
  Fedimint's typed API traits, and remaining consensus calls use its native
  request routing. The adapter owns only request timeouts and error categories;
  pre-consensus state arrives directly through the seat loop's driven client.
- `meta_fields` — the compiled, typed validator set for consensus-metadata
  keys the daemon will relay. Unknown keys fail closed, and each signed
  mutation is bound to its exact whole-object consensus base
  ([SPEC-fi-metadata-maintenance](./SPEC-fi-metadata-maintenance.md)).
- `fleet` — seat allocation and the in-memory registry. `Fleet::authorize`
  is the only crate-visible seat-selection path: it takes a
  `VerifiedFiRequest` of a `SeatScopedFiRequest` type and returns the seat
  only when the request's verified signer matches the seat's immutable
  durable owner (`seat_by_id` stays module-private). Together
  with `seat` it implements the seat lifecycle
  ([SPEC-seat-lifecycle](./SPEC-seat-lifecycle.md)).
- `wallet` — the `EcashWallet` boundary: the trait keeping fedimint-client
  types and the mint generation out of fleet logic, the consuming
  verified-payment value (typed evidence on acceptance or a lazy claw-back on
  refusal), terminal outcomes, and `NoWallet`/test fakes. The vocabulary sits here, above any
  implementation, because storage and pricing must not import their shapes
  from the capability that happens to fill the hole. The pending paid-seat
  economic contract is
- `wallet::EcashPayoutWorker` — the intent-level operator payout boundary:
  start a scoped sweep, observe or await a request, and read drain status. The
  `payout_wire` owns the stable operator response shapes. The Fedimint
  implementation owns the durable ledger and native-operation vocabulary, SQLite
  queries, native submission, and reconciliation. SQLite records the request
  before wallet work and links the native operation after its commit; a process
  death between those commits is reconciled from the same request identity in
  native operation metadata.
- `guardian_fee` — where fee revenue is *owed*: the remittance account
  derived from the mnemonic and the seat id, and the policy value a seat
  votes for ([SPEC-guardian-fee-policy](./SPEC-guardian-fee-policy.md)).
   This half is in core because getting it wrong pays a stranger silently.
   Moving the money afterwards is `GuardianFeeVault`, a hole the wallet
   implementation fills. Collection can span two durable operations, so it
   distinguishes complete results from incomplete results that preserve exact
   terminally confirmed progress; only failures before any durable operation or
   earlier progress remain ordinary errors.
   `remittance_metadata` owns the sealed accounting format both ends share.
- `fedimint` — the wallet implementation:
  `payee` is the FMan side (`EcashWallet`), `claim_worker` reconciles typed
  accepted evidence against Fedimint's durable operation log with periodic
  retries, `guardian_fee` is the fee vault, and
  `setup_payment_policy` the join reconciler consuming the Nostr boundary's
  admitted-set watch. Its clients share one RocksDB, partitioned by a
  monotonically allocated prefix per client scope, with the allocator and its
   prefix-to-scope map under prefix zero. The prefix is reserved before
  Fedimint is handed the database and never reused, so a join that fails
   partway cannot leave state a later client inherits. Identity onboarding
   records fresh-versus-restored wallet provenance: a fresh identity uses
   Fedimint join for a scope absent from its never-removed map, while a restored
   identity recovers mnemonic-derived keys. The wallet never leaves or forgets
   a scope; policy removal only makes it dormant. Selective replacement of its
   RocksDB is outside the supported data-root lifecycle. Pre-origin identities
   migrate to restored provenance. The crate name is
   `fedimint` rather than `wallet` because a Fedimint client is not only a
   wallet. Payout starts are serialized per client scope. Under that fence the
   implementation first searches durable operation metadata for the caller's
   request id and starts a new payment only when none exists.
- `fedimint` scopes — one payment client per federation, and a
   separate guardian-fee client for every guarded seat/federation scope. The
   guardian client uses a distinct derived root, a seat-specific child of that
   root, and the seat's committed `BtcDepositor` key, so payment ecash and the
   ecash of any two guardian seats never share a note pool.
  Collection remains a public client operation: it takes only the seat's invite
  code, never guardian authority.
- `service` — thin FI RPC mapping: every signed verb verifies its envelope
  into a `VerifiedFiRequest`, and every existing-seat verb resolves its
  seat through `Fleet::authorize` as its first fleet call; no FI verb
  calls an operator verb. Policy gates, verb dispatch, and error
  translation follow ([SPEC-fi-rpc](./SPEC-fi-rpc.md)); envelope
  mechanics live in `crates/service-fleet-manager`
  ([SPEC-signed-envelopes](../../service-fleet-manager/specs/SPEC-signed-envelopes.md)).
- `fman-telemetry` — the fixed-ALPN seat-discovery, projected metrics, and safe-event
  pull service plus periodic registration worker. All surfaces use one stable
  FMan-wide capability, with resource identifiers confined to encrypted RPC
  payloads. It selects no arbitrary path or URL and applies the compiled bounded
  source policy before transport
  ([SPEC-guardian-telemetry-proxy](./SPEC-guardian-telemetry-proxy.md)).
- `safe-tracing` — a Manifold-owned tracing layer used independently by the
  FMan process and every bundled fedimintd child. It persists only event-local
  typed `safe_to_share = true` events and never inherits span fields.
- `bounded-rolling-file` — the tracing-independent, single-writer segmented
  JSONL storage mechanism beneath `safe-tracing`; it owns disk bounds,
  retention, permissions, locking, and crash-tail repair
  ([ARCH-fleet-manager-seat-processes](./ARCH-fleet-manager-seat-processes.md)).
- `restore` — rebuilding a fleet from its backup documents, over `db`,
  `backup`, and the seat data directories
  ([SPEC-nostr-backup-restore](./SPEC-nostr-backup-restore.md)).
- `onboarding` — the daemon's first phase: it durably walks identity creation
  or recovery, verified Holder authorization, and initial price/capacity. It is
  the only writer of the onboarding state and identity row, and sits above
  `restore` and below `fleet`, which cannot open until every stage has finished
  ([SPEC-admin-socket](./SPEC-admin-socket.md)).
- `admin` — operator admin socket ([SPEC-admin-socket](./SPEC-admin-socket.md)).
- `admin_http` — optional browser adapter over the same operations; the binary
  may compose its dashboard assets through the shared `operator-ui-static`
  router onto this API router
  ([SPEC-operator-http](./SPEC-operator-http.md)).
- `directory` — the daemon's half of its outward presence: the advertisement
  snapshot a runtime pulls, the setup-payment policy store it pushes into, and
  the onboarding state the admin socket samples. Stated in the daemon's
  vocabulary, not in Nostr's, which is what keeps the dependency one-way.
- the `nostr` runtime — implements `directory`'s traits and owns
  relay connections, periodic signed
  advertisements, Holder-authorization discovery and verification, and
  authenticated setup-payment publication refresh. The binary supplies an
  advertisement-snapshot source and a policy-store callback that persists each
  admitted event atomically with the fleet's derived membership and epoch
  policy; consumers subscribe directly to the boundary's admitted-set watch
  (the wallet's join reconciler is one such subscriber). The relay set and the
  setup-payment publisher come from the resolved `ManifoldEnvironmentProfile`
  — there is no relay or publisher CLI argument, and development-only
  environment overrides are resolved inside `manifold-environment` itself.
  The daemon does not evaluate its own badge; advertisement consumers own
  complete PeerBadge verification.
  Startup requires an explicit `--manifold-environment` or
  `FLEET_MANAGER_MANIFOLD_ENVIRONMENT` so deployment wiring cannot silently
  select development trust. Before onboarding, the daemon binds the data-root
  database to that environment; every later open checks the immutable binding
  and refuses a different environment before using its identity, policy,
  wallet, seats, admin socket, or network service. Retained setup-payment policy
  is revalidated under the selected profile's publisher before the RPC router
  is spawned, so publisher rotation has no old-policy serving window. The
  binding was added by an intentional pre-deployment rewrite of the initial
  migration: preceding experimental databases fail checksum validation and
  must be recreated rather than being assigned an unknowable environment
  ([SPEC-advertisement](./SPEC-advertisement.md),
  [ARCH-manifold-environment](../../manifold-environment/specs/ARCH-manifold-environment.md),
  [SPEC-manifold-environment](../../manifold-environment/specs/SPEC-manifold-environment.md)).
- `cli` — the operator admin command tree and local-socket client.
- `bin`'s `main` — daemon CLI args, capability wiring,
  shutdown ordering, and the argv[0] under which this binary is the bundled
  `fedimintd`.

The resolved `ManifoldEnvironmentProfile` is the sole source of a seat's
Bitcoin network. The binary selects one chain-data backend: complete
operator-supplied Bitcoin Core credentials replace the profile's public
default Esplora route, but cannot replace its network. Staging therefore forms
Mutinynet (`signet`) seats against its profile-owned Esplora default without
Bitcoin Core configuration; Development and Production have no public default
and require Bitcoin Core. The process spawner clears the child environment and
passes only the selected backend's variables.

## Concurrency model

The fleet serializes everything the daemon does. Each active `seat::Seat` is
a cheap command-and-snapshot handle for one **seat loop** task; a terminal
decommissioned handle has no loop or child. The active task owns the process,
  child runtime enum (process, driven client, and ceremony acknowledgement),
  respawn policy, and lifecycle publication and executes every operation that can
observe or mutate fedimintd one at a time, so no two live child operations
interleave. The handle and loop retain clones of one watch sender: the handle
borrows and subscribes, while the loop publishes. `GetStatus` combines that
snapshot with an inline final-directory check; a periodic seat-loop watchdog
is the only publisher of formed-seat consensus health. Requests never initiate
a probe. Dropping the loop's command receiver is the completion barrier for
stop and decommission. After an unexpected command failure, the durable
decommission marker distinguishes terminal decommission from an internal loop
failure.
`GetDkgCode` enters the lifecycle queue even when it can re-serve an exactly
matching recorded code without probing. Admission resolves durable replay and
already-stale requests from one SQLite read snapshot. Potential acceptance then
rechecks both facts in an immediate write transaction shared with settings
writers, and checks the smaller of the remaining live-seat slots and the
never-reused port grid at that writer boundary. `GetQuote` instead reads
capacity, epoch, plans, and
the accepted setup-payment membership in one SQLite snapshot. Settings changes
compare, write, and replace the epoch in one immediate transaction; the Nostr
boundary's common-set replacement writes the retained event, the derived
membership, and (on any removal) a fresh epoch in one transaction the same way,
so membership and epoch can never be observed inconsistently. SQLite owns these
serialized decisions
([ARCH-fleet-manager-storage](./ARCH-fleet-manager-storage.md)); seat facts are loaded
at startup and operator settings remain database-owned.

One fleet-wide completion-hook worker reconciles `completion_callbacks` joined
to immutable `formed_seats` and excluding `decommissioned_seats`; it never
consults a ceremony attempt or probes `fedimintd`. Seat transitions only commit
SQLite facts and wake the worker. Periodic scans are the liveness floor, so a
missed wake or process restart loses no work. The worker invokes different seats
concurrently but admits at most one invocation per seat. The first `StartDkg`
callback choice is retained for the formation; later attempts do not replace it.
Before network I/O, one SQL update checks that the callback is still resumable on
a formed, non-decommissioned seat, then increments the attempt count and persists
the next retry time. Every guardian and retry uses the FI's formation-wide
gateway idempotency key, which the gateway deduplicates per hook id. Retryable
network/408/5xx/rate-limit outcomes use bounded exponential backoff with
deterministic per-seat jitter. Missing/mismatched origin or an unavailable HTTP
client is operator-blocked without network I/O. Not-found, expired/revoked,
max-use and policy rejection are terminal. Delivery, terminal rejection and
decommission atomically clear the plaintext bearer while retaining sanitized
status/reason/timestamps for operations. A
request lost at process death is safe because startup reconstructs pending or
operator-blocked work and the gateway deduplicates the stable idempotency key
across retries and guardians addressing the same hook.
Fleet shutdown cancels and joins every invocation; bare drop signals detached
cleanup whose `Db` handle retains the data-root lock until all bearer-holding
futures are gone.
The focused verification split is recorded in
[`testing.md`](../testing.md).

Operators configure the sole accepted gateway origin with
`--push-gateway-origin` (`FLEET_MANAGER_PUSH_GATEWAY_ORIGIN`). Omitting it keeps
ordinary direct-daemon service available but rejects callback-bearing
callback-bearing `StartDkg` requests before mutation. Production accepts HTTPS only. Development may opt
into a loopback HTTP origin with `--allow-insecure-push-gateway-origin`
(`FLEET_MANAGER_ALLOW_INSECURE_PUSH_GATEWAY_ORIGIN`); that escape hatch is
rejected for every other Manifold environment.
The Umbrel and StartOS production packages require the real
`FLEET_MANAGER_PUSH_GATEWAY_ORIGIN` at startup and deliberately provide no fake
default.

## State ownership

Six kinds of state, six owners:

- **Derived** (keys, identities): recomputed from the mnemonic, never
  stored except the mnemonic itself
  ([ARCH-fleet-manager-identity](./ARCH-fleet-manager-identity.md)).
- **Owned live facts** (durable accepted seats, the current ceremony's
  acknowledgement, the set-once formed federation invite, decommission):
  in-memory synchronization while the daemon runs; accepted-seat identity,
  formation, and decommission survive in set-once SQLite records rebuilt at
  startup, while ceremony acknowledgement is ephemeral. The signed acceptance
  is reconstructed from durable seat facts and signed afresh on replay; its
  signature is not stored.
- **Database-owned offer state** (epoch, plans, the retained setup-payment
  publication and its derived accepted membership): read as a coherent
  snapshot and changed transactionally. Membership is written only by
  common-set replacement, never by an operator verb; the wallet joins
  members from the admitted-set watch and never leaves. Keeping every scope
  mapping is also what makes absence under a freshly generated identity proof
  that its federation-derived root has never been used.
- **Wallet-implementation-owned payout jobs**: the `fman-fedimint` payout
  worker operates the shared SQLite ledger containing the caller request ID, immutable
  wallet scope (including the public guardian invite needed after seat
  decommission), destination snapshot, and the set-once link to a native
  operation. The schema remains in core's single migration set; no lifecycle
  transition deletes or retargets a job.
- **Fedimint-wallet-owned payout operations**: each wallet database owns the
  native v1/v2 operation, its state machines, and FMan request metadata. FMan
  reconciles a missing SQLite link by enumerating the immutable stored wallet
  scope. The two databases are separate commits and must be backed up and
  restored as one consistent data root. A mismatched restore is unsupported;
  native destination binding makes detectable skew fail closed rather than
  misassociate an old operation with a retargeted job.
- **fedimintd-owned runtime state** (setup conversation, formed health):
  never persisted; setup observations arrive through the driven-child stream,
  while formed health is re-derived by probing the consensus API
  ([ARCH-fleet-manager](./ARCH-fleet-manager.md)).

## Trust boundaries

- FI requests arrive over iroh from anyone. Signed seat-scoped requests are
  authenticated by their envelope and the seat's creation-time `fi_id` binding.
  `GetAvailability`, `GetQuote`, and `GetFmanTrustMaterial` are
  intentionally unauthenticated reads; the latter returns signed public trust
  material. Quotes bind their `fi_id` and payment binds the quote, so
  unsolicited payment is unrepresentable
  ([SPEC-locked-payment](./SPEC-locked-payment.md)).
- Telemetry requests arrive on a distinct Iroh ALPN and use one stable,
  root-derived FMan bearer registered to Fedi. The bearer authorizes seat
  discovery, every Running seat's metrics, and all explicitly shareable
  journals. The child is not contacted until the secret passes constant-time
  comparison
  ([SPEC-guardian-telemetry-proxy](./SPEC-guardian-telemetry-proxy.md)).
- Safe-event authorization precedes journal enumeration and path selection;
  callers can select only the global journal or a seat id already mapped by the
  fleet.
- The admin socket trusts filesystem permissions: whoever can read the
  data root already owns the fleet (it contains the mnemonic database).
- DKG completion callbacks are auxiliary and non-load-bearing: federation
  formation succeeds independently of delivery. The FI supplies a bearer hook
  under one deployment-pinned push-gateway origin; core validates and stores
  that exact capability, while the binary's no-proxy, no-redirect HTTPS adapter
  is the only component allowed to invoke it. Development HTTP is restricted
  to numeric loopback.
- Each `fedimintd` child binds its iroh-carrying ports (p2p, api) on all
  interfaces (the local e2e harness, whose seat keys are publicly derivable,
  keeps them on loopback) — fedimintd places its iroh UDP sockets at those
  addresses, and
  a loopback bind would force relay-only peering with no hole-punched direct
  paths — while the ui and metrics ports stay localhost-only. In iroh mode
  fedimintd binds no TCP listener at the p2p address; the api address also
  carries fedimintd's plaintext WebSocket client API, the same public API
  already served to every iroh dialer, with admin verbs gated by the seat's
  `api_auth`. The public transport is iroh. The daemon owns the child's
  environment and startup contract:
  nothing from the operator's shell can alter fedimintd behavior, and no
  secrets travel via env or argv (test-enforced).
- The host is assumed single-tenant: local processes are inside the trust
  boundary (pre-DKG fedimintd exposes no network API; its inherited socket is private to FMan
  until the daemon sets local params).

Constrained by [REQ-no-public-ip](./REQ-no-public-ip.md) and the
cross-program schema records
([SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md)).
The current shipping boundary and
intentional omissions are [ARCH-fleet-manager-product-boundary](./ARCH-fleet-manager-product-boundary.md).
