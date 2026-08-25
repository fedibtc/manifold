# ARCH-liquidity-manager: FLIP liquidity-manager daemon

## Status

Production deployments preview the Fedimint invite, read the consensus
seat-binding directory, verify the FMan trust material the request carries, and
perform fresh revocation lookups. FLIP does not resolve FMan advertisements
over Nostr. An unavailable required trust dependency fails the request closed
with `provider_unavailable`; it is not substituted or bypassed.
Test deployments may use `--trust-fixtures` only for invite-code preview, and
startup refuses that mode for Bitcoin mainnet. See the current transport and
fixture boundary in [SPEC-flip-federation-trust](./SPEC-flip-federation-trust.md).
Umbrel and StartOS packaging is planned but unimplemented; the current
packaging path is the Docker image described in
[`docs/liquidity-manager/liquidity-manager-docker.md`](../../../docs/liquidity-manager/liquidity-manager-docker.md).

## Role

The `liquidity-manager-daemon` crate is FLIP, the provider-run daemon that
adds liquidity to a Fedimint federation after formation. An operator runs it
beside their own `gatewayd`; FLIP does not participate in formation or DKG,
and it does not package or manage `gatewayd`, Lightning nodes, or
Bitcoin/chain-observer infrastructure. One deployment is locked to one
configured gatewayd: that wallet funds gateway/LN and stability-pool
allocations, top-ups, and operator withdrawals. There is no pricing, payment,
marketplace, automatic replenishment, or multi-gateway support; after an
accepted allocation reaches a terminal state, more liquidity requires a new
app request.

The daemon exposes three surfaces:

- an app-facing **Public Liquidity API** over iroh
  ([SPEC-flip-rpc](./SPEC-flip-rpc.md)), whose endpoint is advertised over
  Nostr ([SPEC-flip-advertisement](./SPEC-flip-advertisement.md));
- a private operator **Admin API** (HTTP/JSON, bearer token) whose Nix release
  embeds the bundled web client through the shared `operator-ui-static` router
  ([SPEC-flip-admin-api](./SPEC-flip-admin-api.md));
- an outbound **Nostr advertiser** that publishes the provider advertisement
  only while setup and dependency validation pass.

Wire DTOs, service traits, and canonical payload/signing rules live in
`crates/service-liquidity-manager`
([SPEC-flip-canonical-payloads](../../service-liquidity-manager/specs/SPEC-flip-canonical-payloads.md));
the daemon must not duplicate them.

## Module responsibilities

- `config`, `daemon`, `main` — boot configuration (CLI + env), data-dir
  layout, process lock, wiring, and the normal/restore-only boot modes. The
  selected canonical environment constructs the shared `PeerBadgeVerifier`,
  which is passed into normal-mode `run_daemon` and retained in
  `DaemonContext`. The same selected profile constructs the shared domain
  `PeerBadgeTrustPolicy` passed into the request-carried FMan verification
  pipeline, so FLIP's direct envelope path cannot drift from verifier-backed
  minimum-level semantics. Isolated restore mode exposes no
  trust-verification surface and starts before verifier construction, so a
  missing production issuer cannot block backup recovery. Daemon startup
  rejects a verifier whose retained environment/revision differs from the
  selected profile before filesystem or network work. It requires an explicit
  `--manifold-environment` or `FLIP_MANIFOLD_ENVIRONMENT` so deployment wiring
  cannot silently select development trust
  ([ARCH-manifold-environment](../../manifold-environment/specs/ARCH-manifold-environment.md),
  [SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)).
- `identity`, `auth` — provider service identity (operator-imported Nostr
  service key) and Schnorr signing over canonical payload hashes; without an
  imported key the daemon fails closed.
- `secret_store` — encrypted SQLite secret records (gatewayd credential,
  bitcoind password, provider key) behind AEAD with a local deployment key;
  admin views and logs expose only redacted indicators.
- `database`, migrations — one local SQLite database owning all durable
  state.
- `setup_store` — setup, validation, hot config reload, and the audit log.
- `advertisement`, `nostr` — advertisement build, publish, withdraw, and
  readiness gating.
- `holder_authorization` — enrollment of Holder-published authorizations from
  the configured relays, and the inline trust envelopes the advertisement
  carries
  ([SPEC-flip-holder-authorization](./SPEC-flip-holder-authorization.md)).
- `public` — the iroh transport and the request
  acceptance/idempotency/planning logic it serves.
- `verification`, `federation_preview`, `revocation`, `attestation_store`,
  `trust_fixtures` — the federation trust pipeline
  ([SPEC-flip-federation-trust](./SPEC-flip-federation-trust.md)).
- `allocation_store`, `allocation_funding`, `gateway_allocation`,
  `stability_allocation`, `stability_deposit` — durable allocation state and
  the two background funding workers (gateway: connect → deposit address →
  withdraw → observe; stability pool: withdraw → peg-in → `deposit_to_provide`
  → observe). Each worker names its own funding constants once, through
  `allocation_funding::FundingKind`, so the persisted wallet-operation id and
  the operation type it is looked up by cannot name different workers.
- `gateway`, `stability_pool`, `target_fedimint`, `chain_observer` — external
  dependency adapters: gatewayd admin API, stability-pool client,
  target-federation Fedimint client, and the read-only Esplora/Bitcoin Core
  chain observer. Each adapter is separate from the worker that drives it —
  `gateway`/`gateway_allocation` as `stability_pool`/`stability_allocation` —
  so setup validation can probe a gatewayd through the adapter without
  reaching the worker that reads setup back. A target client watches the chain
  through the daemon's own
  configured chain observer when that is an Esplora, rather than through
  whatever the target federation advertises; a Bitcoin Core observer cannot
  serve one, because the Fedimint wallet client has no Bitcoin Core path.
  Target clients are a bounded pool, not a cache: requester-supplied federation
  ids decide which ids reach it, so a boot-configured ceiling closes idle
  clients rather than letting that input size a set of RocksDB handles and
  background tasks. Their databases are retained past eviction, because they
  are what operator recovery reads for value FLIP has stopped managing.
- `wallet`, `funds_admin` — the gatewayd-backed provider wallet, its
  accounting, and the durable wallet-operation records
  ([SPEC-flip-funding-safety](./SPEC-flip-funding-safety.md)).
- `recovery` — startup recovery: inventories active allocations, items, and
  wallet operations for workers to resume before fresh work is accepted;
  accepted allocations are the only durable request outcome, so there are no
  request-level records to reconcile or expire.
- `admin` — the operator Admin API: the verb implementations and the
  HTTP/JSON surface serving them, in one module
  ([SPEC-flip-admin-api](./SPEC-flip-admin-api.md)).
- `admin_token` — the persisted, encrypted Admin API bearer token that takes
  over from the boot bootstrap token.
- `manual_ops` — the guarded `retry_funding_step` / `cancel_allocation`
  remediation surface.
- `backup` — data-directory tar.gz backup, staging and validation of an archive
  before it is applied, the data-dir swap with its retained previous state and
  rollback, and the restore-only boot mode.

## Startup and readiness

Startup order is recovery-first: open storage and SQLite, restore wallet
operations and chain-observer cursors, restore allocation state, resume
in-flight work (including `in_doubt` sends), and only then serve fresh
requests and publish or refresh the advertisement. The advertisement is the
only public ready signal; when the deployment is not ready FLIP withholds it
rather than publishing a non-ready state, and withdraws one it has already
published ([SPEC-flip-advertisement](./SPEC-flip-advertisement.md)).

The public Iroh transport identity is derived from the provider signing
identity, so the advertised node id survives a restart and a restart never
costs a reconfiguration. That derivation orders startup: the transport binds
only once an identity exists, and a daemon booted without one waits for an
Admin API install rather than for an operator restart. Dependency
configuration is read from SQLite per pass rather than cached at boot, so
gatewayd, chain-observer, and network changes take effect on the next worker
tick. Periodic workers retry rather than fail fatally, and record each pass's
outcome so a worker failing every pass is visible through health instead of
only in logs.

The daemon process is split into a shell and a replaceable runtime generation.
The shell owns what a restore must not disturb — boot arguments, the
data-directory lock, and the Admin API listener — and holds at most one
generation. Everything derived from the data dir (database, secret store, auth
and verification providers, relay publisher, target-federation clients, and the
periodic workers) belongs to the generation, which runs on a cancellation token
that is a child of the process token. Restoring a backup onto a running daemon
replaces the data dir, so it replaces the generation: the generation is torn
down in full before any file moves, and the next one is built through the same
path a fresh boot takes. This is the architectural invariant that makes a live
restore reason like a restart — no state derived from the old data dir survives
into the new generation — while the continuously held lock is strictly stronger
than a stop/start, which releases it in between.

## State and accounting

All durable state is local: SQLite plus target-federation client storage and
encrypted secret records under the data directory. Secrets at rest
(`secret_store.rs`) are AES-256-GCM records — a fresh random nonce per
record, associated data binding the secret name, record version, algorithm,
and key id under the `fedi-flip-secret-store/v1` prefix, and version and
algorithm checked on decrypt — encrypted under a local 32-byte hex key file
that is generated with mode 0600 when missing. The MVP backup is an
unencrypted tar.gz of that directory and is therefore itself secret material.
Wallet accounting derives `available_balance` from the spendable gatewayd
balance minus pending outgoing operations, in-flight allocation amounts, and
the configured fee reserve; reservation is strict — committed amounts plus fee
reserve stay reserved until terminal item state has been reconciled with
wallet settlement.

## Trust boundaries

- App requests arrive over iroh from anyone; authentication is the signed
  canonical payload — binding the declared requester to the authenticated
  transport actor is a tracked open item (see the status in
  [SPEC-flip-rpc](./SPEC-flip-rpc.md)). Private federation details travel
  only over this RPC, never over Nostr.
- Federation eligibility is verified locally against configured trusted
  issuer authorities with fresh revocation lookups; there is exactly one
  verification profile, with only the invite-code preview fixture-substitutable
  and never on mainnet. FMan trust standing arrives as signed material inside
  the request and is never substitutable
  ([SPEC-flip-federation-trust](./SPEC-flip-federation-trust.md),
  [SPEC-flip-federation-trust](./SPEC-flip-federation-trust.md)).
- The Admin API trusts the bearer token, which identifies the installation;
  it must bind to local/private interfaces unless the operator adds their own
  access control. Operators may see private federation details there, but
  never raw secrets.
- Gatewayd owns wallet signing and broadcast; FLIP holds full gatewayd admin
  credentials but the chain observer is read-only. Gatewayd wallet state and
  signing material never appear in advertisements or app-facing RPC.
