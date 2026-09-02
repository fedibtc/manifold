# Security notes

This repository is experimental infrastructure for decentralized federation components and local development tooling.

## Nix source inputs and public binary caches

The default development shell's credential SDK, Fedi, and Fedimint source
inputs are public and pinned in the lockfiles. Neither local builds nor CI need
a cross-repository GitHub token to fetch them.

SelfCI configures the public `fedibtc` Cachix cache as pull-only with
`skipPush: true`; pull-request jobs must never hold cache write access, because
a fork or compromised PR that can push to the cache can poison binaries every
consumer substitutes. The publish workflow — which runs only on `master` pushes
and manual dispatch — pushes the built release image closures with an explicit
`cachix push` of named paths, deliberately avoiding daemon/watch-store modes
that could pick up other jobs' paths on the shared runner.

## Cloud FMan telemetry private listener

`cloud-fman-telemetry` separates its authenticated public registration listener
from its private health, readiness, and Prometheus listener. The private
listener has no application-layer authentication. Bind it only to loopback or
an access-controlled telemetry network and deny ingress from the Internet and
untrusted workloads. A non-loopback bind requires an explicit runtime assertion
that deployment policy provides this isolation; the process cannot verify that
NetworkPolicy or an equivalent control exists or is enforced.
Its `/metrics` output contains vetted operational data and stable FMan,
guardian-seat, and invite-derived federation identifiers; it never belongs on
the public registration route.

## Image publishing credentials

`.github/workflows/publish.yml` is the only workflow that holds write access to
anything outside this repository. It runs solely on pushes to `master` and on
manual `workflow_dispatch`, never on `pull_request`, so a fork pull request
cannot reach the registry. Keep it that way: adding a `pull_request` trigger
would expose registry write access to untrusted contributors.

GHCR access is the workflow's own `GITHUB_TOKEN` with the `packages: write`
permission — repository-scoped and expiring with each job, so it adds no stored
secret and no cross-repository reach. The four `manifold-*` packages are being
recreated from post-scrub source and will be flipped **public** at the repo
cutover, after which consumers (staging Kubernetes, Umbrel devices) pull them
anonymously with no read credential to distribute or leak. The publish workflow
asserts the visibility expected for the current phase — private until the flip,
public after it — so an accidental visibility change is a loud failure, not a
silent policy change.

`packages: write` makes the job's `GITHUB_TOKEN` a registry write credential
for the job's whole lifetime, which is why the checkout runs with
`persist-credentials: false` — the token must not sit in `.git/config` through
the long build on this shared runner.

Published images must not bake in secrets. The fast Push Gateway and FLIP OCI
checks reject secret-like environment variables; the Fleet Manager OCI check
asserts its declared entrypoint, runtime environment, volumes, and working
directory. The FMan and FLIP checks package UI-enabled CI-profile daemons, which
forces Nix to fetch and build the real operator UI while keeping Rust checks on
one Cargo profile. They validate packaging shape, not the optimized binaries.
Every publish architecture consumes the system-qualified
`release-container-images` aggregate, and only the trusted publish workflow
builds and pushes those release-profile images. Supply deployment configuration
at runtime instead.

## Local development services

`defe`, `defe-cli`, and the push gateway resource are local test/development
tools. They bind to loopback or Unix sockets, allocate temporary resources, and
should not be exposed to untrusted networks. Treat paths, process binaries,
environment variables, and resource descriptors as local trusted inputs from
the same developer or CI job.

`FMAN_E2E_PAUSE_AFTER_GUARDIAN_FEE_PAYOUT_START` is a local fault-injection
seam honored only alongside `FMAN_E2E_LOCAL_IROH`: it writes a caller-selected
marker after a native payout commit and then deliberately stops making
progress before the FMan database link. Never set either variable in a
production Fleet Manager process.

`fi-cli` is also locally invoked, trusted development/test tooling, but it may
contact Iroh relays, pinned remote FMan endpoints, and wallet endpoints from
invites. Those endpoints and ordinary `fi-cli` state must remain test-only;
`fi-cli` is not a supported production FI application or wallet. Use only test
federations, credentials, tokens, and funds. Its explicit wallet-secret,
bearer-token, and completion-callback inputs retain the narrower hardened contracts documented in
[`crates/fi-cli/SECURITY.md`](crates/fi-cli/SECURITY.md).
Its development-only guardian-remittance command also trusts the caller's
explicit BtcDepositor account id and pre-sealed metadata and can irreversibly
move wallet value; it performs no production payer-policy validation and must
be used only with test accounts and test funds.

## Sensitive data

### Fleet Manager bundled guardian process

FMan links and directly spawns its pinned `fedimintd` as the guardian process.
The pinned fork is also a compatibility prerequisite: guardians may be selected
across patch and prerelease skew only because the exact pinned commit contains
the reviewed Fedi 0.11 backport of Fedimint PR #9092. Both bundled-daemon run
paths must pass the shared `fedi` vendor identity. Any Fedimint pin change must
re-verify the source ancestry/backport, advertised exact release, runtime vendor
wiring, mixed-release DKG behavior, and release-sync checks. This 0.11 backport
cannot reject an incompatible major/minor/vendor while exchanging setup codes;
it detects the mismatch at the final consensus-config checksum instead.
Treat that code as part of FMan's trusted computing base. The process boundary
contains lifecycle and crash effects; it does not establish a UID, mount,
network, PID, seccomp, or container boundary against a malicious child. A
compromised child can reach fleet authority under the shared single-tenant host
model, as recorded in
[`CLAIM-fleet-manager-compromised-child-contained`](crates/fman/specs/CLAIM-fleet-manager-compromised-child-contained.md).
This accepted lack of sandboxing is defense in depth only: it does not weaken
the required operator custody, data-root, admin-socket, credential, backup, or
network boundaries.

FLIP treats every federation endpoint in an FI-supplied invite as an outbound
network capability. The default `GlobalOnly` policy accepts only canonical
`iroh://<node-id>` guardian endpoints and rejects every `ws`/`wss` endpoint
before DNS or connector work. The pinned WebSocket connector resolves names
internally and follows redirects, so a separate address check cannot constrain
the socket it eventually opens. Iroh instead authenticates an endpoint identity
and is Manifold's production guardian transport. The explicit
`AllowPrivate` setting admits WebSockets only for non-mainnet local/test
deployments at generation startup. An unconfigured daemon started with the flag
can currently retain it after Admin applies a mainnet setup; do not use that
live-apply sequence in production.

Canonical syntax does not constrain the direct or relay destinations learned
through Iroh discovery. A requester that already holds a valid, unrevoked FMan
endorsement can substitute an Iroh node id it owns and publish addressing for
that node. Before end-to-end node authentication, FLIP may send an encrypted
discovery Ping and a QUIC handshake to the published UDP address. A discovered
relay may cause a TCP connection followed initially by a TLS ClientHello, or,
for a cleartext relay, the fixed empty `GET /relay` upgrade request. The
published relay name appears in addressing fields such as SNI and `Host`; a
successful relay handshake may continue with protocol-generated client and
relay frames. The requester cannot select an arbitrary method, path, body,
header set, or subsequent frame contents, use the connection as a general
proxy, read the destination's response, or receive that response through FLIP;
only a generic outcome or timing difference may provide a weak blind
reachability signal. This constrained behavior is an explicitly accepted
low-severity residual as of 2026-08-20, not proof of the broader
no-connection-attempt claim. Deploy FLIP with egress controls when it has
sensitive internal reachability;
otherwise maintaining patched Iroh and Fedimint forks is not required for this
residual. Refusals returned to the FI must not include the endpoint, resolver
result, redirect location, or dial detail.

FLIP's stability-deposit retry contract assumes one active process/runtime
generation per data root, SQLite database, and target-client directory. It
persists a random operation ID and immutable amount/fee tuple before effects,
serializes same-ID attempts, and accepts only an exact global operation-log
receipt whose versioned metadata matches that tuple. Ambiguous errors, crashes,
and cancellation races preserve or terminalize the fence; bounded operation
history is diagnostic only. Malformed, incomplete, or mismatched state requires
operator reconciliation and keeps its reservation. Backup and restore must move
FLIP allocation state and target-client operation logs from one common recovery
point. Revisit this contract before adding replicas, shared client storage,
parallel workers, another submission API, or changing either pinned client.

Do not commit real credentials, Firebase service-account material, Nostr private keys, Fedimint secrets, or production database contents. Tests must use generated or dummy keys and isolated temporary directories. The one deliberate exception is the development/staging placeholder issuer material in `crates/manifold-environment/fixtures/`: those environments' trust roots are declared publicly known and impersonatable (see the SECURITY BLOCKER in that crate), and committing their complete secrets is what pins one canonical issuer authority instead of a last-writer-wins relay event. Nothing production-trusted may ever join that exception.

Push-gateway hook URLs, hook tokens, Firebase/FCM registration tokens,
Firebase service-account material, and FCM OAuth access tokens are bearer or
credential material. They must be
redacted in debug/log output and omitted from client errors except for the
one-time hook token/create response intentionally returned to the authenticated
temporary management caller. That create response must be marked no-store/no-cache.
Generated public hook URLs must use an absolute `https://` origin in production
configuration; local/test insecure origins require an explicit loopback-only
escape hatch. Push-gateway API errors should use the stable sanitized JSON error
envelope rather than forwarding database, provider, parser, or credential details.

FMan accepts a DKG completion hook only inside the FI-signed, versioned
`StartDkgWithCallbackRequest` for a seat that FI owns. The callback-free
`StartDkgRequest` is a distinct RPC/signing label; this prevents an older peer
from silently ignoring the callback and beginning DKG. Signature ownership
alone is not an SSRF defense: the daemon must match the parsed URL's origin to
its explicit deployment configuration, require the exact public hook path,
reject credentials, query, fragment and redirects, and permit HTTP only for an
explicitly enabled development loopback origin. Never log or format the
callback URL or its idempotency key. SQLite/WAL files and backups containing
pending callbacks are bearer-capability material. Delivery or a definitive
terminal outcome atomically clears the live plaintext bearer while retaining a
one-way, non-authorizing commitment and sanitized outcome. The commitment
remains confidential and redacted because it is derived from bearer material.
Startup recomputes the commitment for every resumable callback and fails closed
before invocation when the stored plaintext and commitment differ.
Logical clearing does not erase
old SQLite pages, WAL content, or backups, which remain sensitive through
compaction and retention. Invocation errors must discard reqwest's URL-bearing
error detail and expose only a sanitized class or status. FMan retains and
parses at most 4,096 bytes from a rate-limit response body before reducing it to
the one recognized sanitized gateway code; larger frames and chunked bodies
remain ordinary retryable rate limits. Graceful shutdown cancels and joins each
in-process invocation before returning. Clearing or canceling local state cannot
retract a request the gateway already accepted; the stable idempotency key makes
that race safe to retry.

Push-gateway mobile app-open/deep-link context must be controlled by hook records
created through the management API. Public hook callers are untrusted and must
not be allowed to inject or override workflow/action/deep-link routing, app-open
behavior, notification kind, recipient identifiers, notification identifiers, or
gateway-reserved push `data` keys, including free-form `data.event_id`.

Push-gateway application logs and metrics must use sanitized route templates and
low-cardinality labels only. Do not log full hook URLs, query strings, hook path tokens, FCM tokens, credential JSON, OAuth access tokens, recipient identifiers,
or registration tokens in metrics. Push-gateway request ids are gateway-generated
and must not trust or propagate client-supplied values that could carry bearer
material.

The guardian telemetry capability is one rotatable bearer secret scoped to an
entire FMan. It authorizes authenticated seat discovery, metrics access for
every Running seat, and the FMan and retained-seat safe-event journals. It must
never be logged, exposed through FI/operator listing APIs, or published on
Nostr. FMan derives it from its protected mnemonic using a domain-separated
KDF and stores only its global monotone generation in SQLite. There is no
per-seat secret, generation, rotation, or re-enrollment state; rotation is one
FMan-wide operation.

Registration sends the capability only with current Holder authorization and an
exact-body NIP-98 proof. The receiver must completely verify the Holder
credential and require the NIP-98 signer to equal its subject before encrypted
persistence. Receiver databases and backups contain the capability and require
credential-grade encryption and access control. Periodic idempotent
registration is the recovery mechanism after receiver state loss.

Authorization must precede seat lookup, child probing, journal enumeration, and
filesystem access. The proxy may fetch only a selected Running child's fixed
loopback metrics port, rejects redirects, and bounds frames, concurrency, time,
and response bytes. Raw Prometheus bodies, invite codes, journal identifiers,
cursors, FMan identities, and seat ids must not enter FMan logs or FMan's own
metric labels. See
[`SPEC-guardian-telemetry-proxy`](crates/fman/specs/SPEC-guardian-telemetry-proxy.md).

Safe-event journals are a separate local channel from raw guardian metrics and
ordinary stderr logs. The tracing layer accepts only the event-local typed
boolean `safe_to_share = true`, never span fields or rendered child output.
One-shot `fman-cli` processes initialize only ordinary stderr
tracing; they neither open a safe-event journal nor route unmarked operator
result warnings into one.
Records use `tracing_subscriber`'s standard JSON formatter with current-span
and span-list output disabled.
FMan and each child exclusively own separate directories with 0700 directory
and 0600 file permissions; a lifetime-held nonblocking writer lock prevents
cross-process append and rotation races. After bounded formatting, event
emitters use nonblocking sends into a fixed 128-record queue; a dedicated OS
thread owns the appender and all filesystem I/O. A full or disconnected queue,
oversized event, or retention/write failure drops diagnostics rather than
blocking guardian operation or growing without bound. Each journal is capped
at two 2.5 MiB segments (5 MiB of record data); startup removes legacy segments
over the current limit before a bounded crash-tail repair. Seat journals live
outside the restart-wiped fedimintd data directory and remain retained across `RestartDKG`
and terminal decommission. The marker remains an audited assertion, not a
sanitizer: changes to marked events or formatted values require the
safe-to-share tracing audit.

Each journal directory also contains a canonical UUIDv7 incarnation file. A
synced pending file is published without replacement by a same-directory hard
link and the directory is synced before pending cleanup. Existing directories
receive one identity without rewriting records. A valid unpublished pending
identity is completed; a malformed unpublished pending file is discarded, while
a malformed published identity fails closed and is never silently regenerated.
List and fetch initialize legacy retained journals under a separate metadata
lock. Fetch compares both request and cursor incarnations before segment
discovery and returns a sanitized typed discontinuity with no journal bytes on
mismatch. Incarnations and coordinates remain forbidden in diagnostics.

The supported storage envelope is a local filesystem where file sync orders
data and inode state before linking, hard links publish atomically without
replacement, and directory sync commits same-directory create, link, rename,
and unlink operations. Advisory locks must cover every official journal
process. One no-follow directory descriptor anchors all metadata and segment
operations; regular entries with unexpected types or link counts fail closed.
Deployment must provide these local-filesystem semantics. Syscall errors fail
journal initialization, but the process cannot detect a network/FUSE filesystem
that returns success without honoring them; behavior outside this envelope is
not detectably safe. Existing segments become read-only at reopen, and a durably
reserved fresh segment receives all new records.

Supported FMan restore excludes these journals, or creates new incarnations
before telemetry resumes. A complete persistent-volume snapshot rollback can
restore both records and in-volume identities consistently and is therefore
undetectable by the identity itself. Before starting FMan from a stopped
whole-data-root restore, the operational restore procedure must remove the
global `safe-events/` directory and every `seats/*/safe-events/` directory.
Startup then creates fresh journal incarnations. This deliberately discards
restored telemetry journals; do not start the restored daemon before the reset.

Secure FMan exposure is not cloud collection. The standalone collector described
by [`ARCH-cloud-fman-telemetry`](specs/ARCH-cloud-fman-telemetry.md) now owns
exact-body NIP-98 and live Holder verification, encrypted target admission,
leases, and revision fences. It directly polls the typed safe-journal list/fetch
API and the authenticated guardian metrics proxy, archives validated exact
JSONL, retains bounded latest metric snapshots, and exposes them only on its
private listener. Push-gateway's operator adapter is not a trust hop.

The public listener is plaintext HTTP behind deployment-owned TLS termination.
Operators must configure every immediate proxy CIDR the collector may trust and
must make those proxies overwrite `Forwarded`/`X-Forwarded-For`; untrusted peers'
forwarding headers are ignored. Admission applies a hard connection lifetime and
connection cap before body or cryptographic work, then bounded source-prefix,
request, replay, and PeerBadge-verification admission. Every accepted connection
retains the exclusive data-root lock, and shutdown aborts and joins tracked
connections before the lock can be released.

The database, WAL, archives, keys, and backups are confidential. Startup
authenticates an AEAD key sentinel and the Manifold trust-profile revision and
refuses missing, mismatched, or unsupported-format metadata on populated state.
Each target ciphertext also authenticates its format, deployment profile, key
identifier, target identity, generation, and authorization time. Candidate
databases from an earlier unreleased format must be reset rather than migrated
silently. The data root must be an owned real 0700 directory and its database,
WAL, shared-memory, and lock files must remain 0600 regular files.

The commit-coupled SQLite database, WAL, and archive share one encrypted
persistent volume in the single-active deployment. That volume must provide
local block-filesystem semantics for SQLite WAL shared memory, advisory locks,
`fsync`/`fdatasync`, and durable directory-entry updates. Unsupported
NFS/network/FUSE semantics are outside the deployment contract; the process
checks types, ownership, and modes but cannot prove backend durability.
Expired or quarantined targets are excluded at collection start and again by
the archive/cursor commit transaction. Key delivery and backup placement remain
separate configurable security decisions.

The collector may attach only the private inventory's bounded, authenticated
`fman_id`, display-only `fman_name`, `guardian_seat_id`, and `federation_id`
labels to collected guardian series. The collector derives `federation_id` from
the invite asserted for that exact seat by the authenticated FMan. This is
operational attribution, not independent guardian-membership or child-config
attestation, and is insufficient for authorization, billing, or disputes.
`fman_id`, not the collision-prone name, preserves FMan identity.
Capabilities, Holder envelopes, endpoints, full invite codes or data, journal selectors,
incarnations, cursors, raw unverified identifiers, and caller-controlled or
unbounded values remain forbidden in labels, logs, traces, and errors.
The collector's stderr formatter accepts only its exact crate-target namespace
before applying `RUST_LOG`; operator directives cannot enable events or spans
under nonmatching dependency targets that may format held endpoint or capability
material. Add bounded, categorized collector diagnostics in that namespace
instead of enabling dependency targets in this process.

Collected metric timestamps are the actual 15- or 30-minute observation times.
Repeated exposition must preserve them; a collector must not make stale values
look fresh. Prometheus/Agent must set both `honor_timestamps: true` and
`track_timestamps_staleness: true` and own TSDB and remote-write semantics.
Remote target failure is exported as stale snapshot metadata and does not make
the process readiness route fail. Readiness covers the local database and
ability to serve. Quarantined/expired targets are suppressed immediately and
their latest metric snapshots are deleted rather than reappearing after renewal.

Metrics persistence is capped globally at 32 MiB and 100,000 samples. One
immutable exposition generation may be live at a time; concurrent scrapes
share it, while a changed revision gets HTTP 429 until slow readers release the
old backing. Per-seat admitted text is capped at 2 MiB so JSON serialization stays
within the 4 MiB durable row bound. A policy fingerprint covers the inventory
revision and unconditional static method allowlist. FMan, Fedimint, and build
release metadata do not authorize targets or enable metric families; a
non-allowlisted method taints only its own family. A fingerprint change atomically
discards snapshots and poll deadlines; Prometheus owns history.
Shutdown and worker-failure paths stop new metrics work but join any cadence
reservation or snapshot commit already in its durability segment. As with
safe-journal commits, storage-device completion retains the operating system
and hardware contract described by the collector testing guide.

For safe journals, a changed persisted source incarnation is a continuity gap,
not an order comparison or cursor continuation. A source-reported retention or
cursor gap under the same incarnation is also durable continuity metadata. The
source may preserve an incarnation only while it durably reserves monotone,
non-reused segment numbers and starts a fresh segment after every reopen or crash
repair rather than appending to an existing segment. Every journal operation must
stay relative to one safely anchored directory descriptor, reject symbolic and
multiply linked entries, and validate entry types. A supported restore excludes
journals or forces new incarnations. An in-volume
incarnation cannot detect rollback of the entire volume to a self-consistent
snapshot; the operational restore procedure must force new incarnations before
telemetry resumes.

Each exact JSONL fetch batch is one independent zstd frame in its UTC
reception-day stream. The collector must durably create a new daily file's
directory entry, then `fdatasync` each frame before atomically advancing the
SQLite incarnation/cursor and archive offset/hash. Recovery may duplicate a fetch
but must never advance past bytes that were not made durable.
Collection defaults to a separate five-minute log cadence, four concurrent FMan
polls, 30 reception days, and 10 GiB of compressed archive data; deployments may
tighten these bounds. A local archive or SQLite durability failure stops the
service rather than retrying across an indeterminate append/commit boundary.

The standalone collector durably admits at most 4,096 FMan identities. Expired
and quarantined registration history continues to consume this bound; a new
identity is refused at saturation rather than starving an already admitted target.
Each target durably owns at most 32 typed journal selectors, matching the source
list bound and the archive traversal calculation. Rotating selectors cannot grow
SQLite or filesystem state beyond that limit and is refused without advancing a
cursor. Revisit these coupled target, selector, traversal, quota, and retention
bounds before increasing any one of them or introducing registration deletion.

The push gateway's database-backed hook and durable delivery-outbox state is
currently scoped to exactly one active gateway process, including exactly one
active outbox worker, per configured database. PostgreSQL is the
production-oriented storage backend, but choosing PostgreSQL does not provide a
multi-replica production deployment policy or HA design. Do not run multiple
active gateway processes/workers without an explicit database coordination
design and tests. Outbox rows include push-token snapshots and serialized
notification content and must be protected as sensitive data. Backups and
restored copies of the configured database have the same sensitivity as the live
database and should be encrypted and access-controlled.

Push-gateway public hook invocation currently enforces per-hook fixed-window
rate limits in the configured database together with TTL, revocation, max-use,
body-size, and payload-field checks. New hooks default to 2 accepted invocations
per hour; any higher configured limit should be an explicit test/dev setting or
reviewed production exception. The gateway also has in-memory single-process
abuse and backlog guardrails (source-prefix, per-hook, hook creation,
registration writes, active resource caps, and outbox backlog caps). These
counters reset on restart and are not distributed. Source-prefix limits use the
direct socket peer unless that peer is in configured trusted-proxy CIDRs; trusted
proxies must strip/overwrite inbound `X-Forwarded-For`/`Forwarded` headers.
These controls are not a substitute for a distributed rate-limit/replay backend,
deployment hardening, or multi-replica worker coordination before public
production exposure. Management and registration endpoints use Nostr/NIP-98
signed requests: the authenticated recipient is the signer’s canonical lowercase
hex public key, the signed event must bind the exact method and
`PUSH_GATEWAY_PUBLIC_BASE_URL` plus request path/query, body methods must bind
the raw-body SHA-256 `payload` tag, timestamps must be fresh, and exact event-id
replay is rejected by the single-process replay cache. Legacy
`PUSH_GATEWAY_APP_ID` and
`PUSH_GATEWAY_UNSAFE_ALLOW_ANY_APP_ID_FOR_TESTS=true` knobs are compatibility
no-ops for endpoint auth and must not be treated as production authorization.

## FI formation inputs

FI formation timing options cross a consumer/CLI input boundary. Keep invalid
timing unconstructible and reject it before durable-state, wallet, lease, or
network work. [`crates/fi-client/SECURITY.md`](crates/fi-client/SECURITY.md)
owns the exact shared native/WASM timer domain and runtime deadline contract.


## FMan/FLIP/guardian-telemetry trust material

On Linux, one process-lifetime OS thread creates every supervised `fedimintd`,
so `PR_SET_PDEATHSIG` follows the FMan daemon lifetime rather than an ordinary
Tokio worker's lifetime. Seat tasks still own wait, stop, and respawn. An
unexpected exit of the spawner thread is fleet-wide and requires daemon
restart: existing children receive their parent-death signal and the closed
static request channel makes later spawns fail closed. Re-check the
worker-retirement regression and daemon hard-exit contract before changing the
spawner, Tokio runtime, subprocess boundary, or parent-death behavior.

FLIP applies the invite-endpoint boundary above before preview, target-client
join, and gateway attach. Keep all three paths aligned when changing connector
policy. Review the concrete Iroh destination boundary whenever the
Fedimint/Iroh pin, discovery configuration, or connector overrides change.

LNv2 gateway registration transports only the shared canonical
`GatewayApiUrl`: public HTTPS or an identity-shaped Iroh endpoint, with no
credentials, query, fragment, private/loopback host, or unsupported scheme.
FMan stores the FI-signed URL through its local admin-authenticated LNv2 API but
does not fetch it; making guardians probe requester-carried URLs would create an
SSRF boundary and turn transient gateway availability into authorization
policy. FLIP sources the URL from gatewayd's own public registrations, never
its credentialed admin URL. FI requires threshold guardian acknowledgements
and fresh aggregate readback before durably binding verification to that exact
URL. Recovery consumes already-signed durable completion evidence before FLIP
rediscovery, so provider disappearance cannot strand federation attachment.
The aggregate reader ignores unrelated entries outside `GatewayApiUrl` policy
individually; such an entry cannot hide a valid exact target.

Production-capable FI, FMan, FLIP, push-gateway telemetry, and cloud FMan
telemetry collector processes must
select their Manifold environment explicitly. The shared environment profile
provides typed public issuer identities, the minimum accepted PeerBadge trust
level, the setup-payment publisher key, and Nostr relay routing;
FMan resolves its relay and publisher from that profile with no CLI override.
Development-only environment-variable overrides are resolved inside the
`manifold-environment` crate and refused outside the Development profile, so
staging and production wiring cannot be silently redirected. FMan also binds
each data-root database to its first selected environment before onboarding and
refuses every later cross-environment open before exposing admin or network
services. This prevents policy authenticated under Development from surviving
a restart as Staging or Production. The binding and durable DKG completion
callback storage rewrite the pre-deployment initial migration; preceding
experimental data roots intentionally fail migration checksum validation and
must be recreated rather than being silently assigned an environment or partly
upgraded for callbacks. Identical relay
URLs do not give every consumer identical relay semantics: the PeerBadge
verifier uses them for complete fresh authority lookup of unpinned
(production) issuers and for revocation reads — development and staging
issuer authorities are pinned from committed documents and never looked up —
while FMan uses them
for active publication and operator-triggered Holder-authorization enrollment.
Enrolled complete authorization events are retained in the FMan database and
reverified at startup; failed or empty relay answers never empty the cached set. This cache
is carriage, not credential validity: relying FIs still apply current issuer
policy and revocation checks.
Development and staging issuer identities are known-secret placeholders and
must never secure real trust decisions. Production carries only non-placeholder
issuer identity roots, each a personal key held individually by its custodian.
Each root is independently sufficient to authenticate an issuer authority,
including its choice of issuance key and revocation locations. Compromise of
any one root can therefore authorize arbitrary PeerBadge issuance until a
profile revision removing it reaches every FI, FLIP, push-gateway telemetry,
and cloud FMan telemetry collector consumer.

Adding a production root requires out-of-band authorization and possession
verification plus an internal custodian record; names and private operational
records do not belong in this public repository. Loss, suspected compromise,
or custodian offboarding requires removing the public root, bumping the profile
revision, and tracking a coordinated rollout to every relying consumer. An
empty root set fails verifier construction. The canonical mapping and rollout
contract are defined by
[`SPEC-manifold-environment`](crates/manifold-environment/specs/SPEC-manifold-environment.md).
Production pins its setup-payment publisher and Guardian Verification Fee
account in that profile.

Each relying path applies the same profile-owned minimum PeerBadge trust level
after complete authentication and schema parsing. A profile-policy change must
roll out across all of them as one revisioned deployment so mixed versions
cannot disagree about admission.

Treat FI-, app-, Nostr-, metadata-, and FMan API-provided trust material as
untrusted input until the component that relies on it verifies it locally.
FI metadata maintenance rejects keys over 128 bytes and every unknown key
before probing fedimintd; refusal logs contain only the bounded reason and key
length, never attacker-controlled key bytes — the sanitization is structural,
in that the logging path is handed only the key length. Seat-binding validation
likewise reports a fixed reason for a non-canonical peer id rather than retaining
the rejected id for later formatting. Name, icon URL, and
welcome message each have a 65,536-byte raw resource cap, followed by trimmed
semantic maxima of 30, 2,048, and 500 bytes respectively; guardians submit the
accepted original string. Name and welcome message also reject bidirectional
control and zero-width characters; homoglyph spoofing within accepted scripts
remains possible. The icon URL must name a public host — loopback, link-local,
and RFC-1918 addresses, `localhost` names, and bare undotted hostnames are
refused — but DNS is not resolved, and consumers fetching icon URLs still need
their own fetch-time policy. The fixed ToS value must match exactly. The
seat-binding directory has a 65,536-byte raw and canonical cap, and each
guardian votes only for a directory whose entry for its own seat carries its
own attestation key, refusing when it cannot derive its own peer id: an
attacker-assembled directory verifies on every signature, so the self-seat
check is what keeps an authorized FI from having honest guardians vouch seats
to foreign keys. Tests pin rejection before child access, padded semantic
maxima, exact raw submission, and the self-seat refusal.

FI-signed envelope verification discards serde parse detail when typed payload
decoding fails. Its loggable authentication error retains only the fixed verb
label, so rejected field names and values cannot forge daemon log records.
Revisit these caps and the no-rate-limit boundary together before
adding metadata keys or increasing the transport frame. Both ordinary metadata
and guardian-fee proposals commit to the exact consensus base occurrence and
share one guarded whole-map merge path. The meta module has no atomic CAS, so
the FI must serialize these operations across verbs, reread threshold consensus
after each wave, and rebase/replay on staleness. Within one exact base and one
live FI invocation, it records
acknowledged rows and retries only unresolved guardians with capped exponential
backoff; a new base resets that set. Cancellation or FI restart performs safe
exact replay and may resubmit rows because acknowledgements are deliberately
not durable. The combination of bounded inputs and selective live-invocation
retry keeps an unavailable minority from amplifying near-transport-limit
payloads.
The complete raw consensus object has a separate inclusive 1,048,576-byte cap,
enforced by FI immediately after live reads before hash/parse/fan-out during
both seat-binding publication and later maintenance, and by FMan before parsing
the current object or submitting the canonical target.
Metadata bases are bound to the meta module's monotone consensus revision, so
a base names one occurrence of the board state: a board that returns
byte-exactly to an old state does so under a fresh revision and a fresh base,
and nothing bound to the earlier occurrence — a signed request, a delayed
handler, an admission pin — can re-match it.
Each live seat loop also keeps one pin — the single whole-object target
admitted for the live occurrence: identical retries are allowed, but a
differently targeted Iroh
handler for the same live occurrence is refused with the distinct
`MetaTargetConflict`, not the retryable
stale-base answer, so the FI stops same-base retries against a pinned guardian
instead of treating the pin as transient. One pin is the complete state:
staleness is checked against a fresh consensus read before the pin, and a
superseded occurrence can never become current again, so its pin fences
nothing and is simply replaced — no history, cap, or eviction policy.
The target is pinned before entering the fallible child submit RPC, because an
error response cannot prove that the guardian did not accept the vote; an
ambiguous response therefore permits only exact replay for that occurrence.
A process restart clears the pin together with every
handler it fences. The residual wedge is same-revision equivocation, the
intended refusal: sub-threshold waves of two *different* targets on one live
occurrence can leave both unreachable until enough FMan processes restart or
the base moves, and the fee verb shares the same pin, so fee and metadata
proposals cross-block on a shared occurrence. Ordinary do/undo/redo-differently
cannot compose into this, because the undo mints a fresh occurrence. A
durable signed mutation sequence can replace the restart-scoped pin if the
product later needs immediate supersession within one occurrence.
`fedi:fman_api_urls` consensus metadata is a public directory only; it is not
trust evidence. FLIP/external verifiers start from the invite code, fetch final
config plus consensus metadata, parse the canonical URL directory, query public
FMan APIs, verify signed `GetFederationTrustMaterial` responses, and only then
evaluate returned peer attestations and credentials.

`FmanPeerAttestation` binds an FMan service/ad key to a concrete federation peer
and that seat's public guardian-fee account
using the canonical payload `{ type: "fedi.fman.peer-attestation", version: 1,
attestation }` and signature domain `fedi-fman-peer-attestation/v1\0`. FLIP must
verify each attestation, match it to the invite-code-previewed federation config,
check holder authorizations and backing credentials, and perform required issuer
revocation lookups before accepting liquidity. Multiple peers operated by the
same FMan are valid, but that FMan counts once as a trusted identity for policy
thresholds. Identity equality and threshold counting must use canonical parsed
Nostr public keys, not raw strings or alternate encodings such as `npub`/NIP-21
URIs. Fee accounts in a verified directory must be unique single-signature
`BtcDepositor` accounts. FI formation checks each against the earlier signed
seat acceptance before publication, and every guardian-fee vote validates the
complete consensus directory so replacing a minority recipient cannot reach
threshold.

The current v1 FMan enables the public trust-material API. The generic iroh RPC
adapter limits raw request frames to 1 MiB, 128 concurrent stream handlers, and
a 10-second initial-frame read; the trust-material handler then validates the
canonical request's 4096-byte limit, identifiers, and peer filter before reading
fleet state. It has no endpoint-specific rate limit or 4096-byte
pre-deserialization limit, so deployment-level abuse controls remain necessary.
Relying verifiers must cap canonical response size, public API URLs, peer
attestations, holder authorizations, and credentials; reject stale, overlong,
future-issued, or expired responses; and fail closed when required issuer
revocation lookups are unavailable. Nested peer attestations and holder
authorizations must bind to the response
`material.fman_pubkey`; relayed material from another FMan is rejected unless a
future protocol explicitly defines delegation.

Each seat's fedimintd binds its p2p and api ports on all interfaces because
its iroh UDP sockets live at those addresses; ui and metrics stay loopback. In
iroh mode the p2p port has no TCP listener, but the api port also serves
fedimint's plaintext WebSocket JSON-RPC API: every public client endpoint is
reachable there without authentication, and only admin endpoints require the
seat's `api_auth`. This is the same logical API already reachable worldwide
over the seat's iroh API endpoint and is accepted; nothing FMan-side requires
inbound reachability (relays remain the fallback). Deployments that cannot
firewall the seat grid expose plaintext HTTP/WS and QUIC pre-auth parsing
directly to their network. Containerized packages must publish the seat grid
as UDP only and leave the TCP listener unpublished. Re-check this boundary
whenever the fedimint pin changes what listens at `--bind-api`/`--bind-p2p`
or the API-secret/transport policy changes.


## FLIP backup archive integrity

A FLIP backup archive carries a SHA-256 digest per archived file and restore
verifies every one before the archive is applied. This detects corruption: a bad
sector, a truncated copy, an interrupted transfer.

It authenticates nothing. The digests are stored inside the archive they
describe, so anyone who can modify the archive can recompute them to match. A
restore that succeeds proves the archive is internally consistent, and does not
prove it is the archive FLIP wrote. Treat archive custody as the control that
matters: an archive holds identity secrets and possibly the local secret-store
key, so it is secret material, and its integrity currently rests on where it is
kept rather than on anything the format proves.

Authenticating an archive needs a signature or a MAC over a key the archive does
not carry. That work is open and is tracked in
[`docs/liquidity-manager/liquidity-manager-open-items.md`](./docs/liquidity-manager/liquidity-manager-open-items.md).


## FLIP Admin API credential rotation

Rotating the FLIP Admin API token supersedes the boot bootstrap token for every
request the running daemon serves. A daemon that cannot read its rotated token
locks the authenticated surface rather than falling back, so breaking the secret
store is not a way to re-enable a credential the operator retired.

That lock would also shut the operator out of the API they need in order to
diagnose the storage failure. The `allow_bootstrap_token_fallback` break-glass
argument reopens the fallback and accepts the bootstrap token again. It is
deliberate, and it defines the boundary rotation actually provides.

Rotation protects the token against an adversary who reaches the admin port. It
does not protect it against an adversary who can set process arguments and
restart the daemon: that adversary re-enables the bootstrap token by
configuration. Requiring a restart is the boundary, not an obstacle to overcome
— recovery is meant to need control of the deployment rather than reach to the
port.

Two consequences for operators. Keep the bootstrap token as a live secret for
the life of the deployment, with the same handling as the rotated one; a
rotation retires it from the request path and not from the deployment. And treat
the ability to restart a FLIP process with chosen arguments as equivalent to
holding the current admin token, when sizing who may operate the host.

The fallback is off by default, applies only while the rotated token cannot be
read at all, and logs a warning on every request it admits. Restore the storage
and restart without it.


## FLIP live-restore allocation authority

An accepted FLIP allocation is durable authority for later provider funding.
Live restore compares the running generation's accepted allocation identities
with the staged archive while new allocation commits are fenced. It refuses an
archive that omits or replaces an identity before tearing down the running
generation. One generation admits only one pending restore, so a concurrently
staged archive cannot be installed later after authority has advanced. Do not
bypass these refusals by deleting SQLite rows or using fresh-host restore mode
against the same active deployment: either action discards the only available
cross-generation authority and can repeat externally irreversible work.

Fresh-host disaster recovery cannot perform this comparison because no newer
generation is present. Before returning such a restore to service, the operator
must select the intended archive and reconcile its allocation and funding history
with the provider wallet, gateway, and target federation. The daemon does not
silently assert that an archive is the latest financial history.


## FLIP provider-wallet manual recovery

An externally accepted provider-wallet send can outlive loss of its response,
txid, and all promptly available chain or target-side evidence.
`in_doubt` and `manual_review_required` fence automatic resubmission, but an
authenticated `SafeToRetry` resolution is an operator assertion: it resets the
operation to `pending` and permits the allocation worker to submit it again.
Admin authentication proves authority to make that assertion, not that the
first send did not happen. Operators must reconcile the provider wallet and
external systems out of band before selecting `SafeToRetry`; otherwise the
accepted first send plus lost evidence plus a mistaken authenticated resolution
can duplicate the submission and payment.

[`gateway::tests::manual_safe_to_retry_resubmits_an_accepted_unknown_gateway_send`](./crates/liquidity-manager-daemon/src/gateway.rs)
reproduces this boundary deterministically. Re-check it whenever manual
resolution, wallet/chain/target evidence, or provider-wallet idempotency changes.


## FMan key-locked ecash payments

The FI-facing RPC service currently has no rate limiting. Unsigned availability
and quote calls allocate no durable state, but they and invalid paid
presentations still consume bounded compute; deployment-level abuse controls
remain necessary for untrusted exposure.

Seat payments are Fedimint notes locked to quote-specific keys derived by the
FMan. Spend keys and reconstructed bearer notes must never be logged, formatted
in `Debug` output, included in errors or metrics, or written to Nostr backups.
The signed quote binds the FI, payment federation, denominations, and blinded
nonces; `CreateSeat` persists payment evidence with the accepted seat before
handing it to fedimint's durable receive state machine. Aggregate blinded
signatures and operation ids are public; spend keys and reconstructed notes are
not.

Payment-federation invites reach the FMan only through the authenticated
common setup-payment publication, whose admission rejects invites containing a
Fedimint API bearer secret before invoking Fedimint client code (the pinned
client logs complete invites at debug verbosity). The retained event and its
derived membership live in SQLite. Treat the FMan SQLite database and WAL,
per-federation wallet databases, data-root copies, and all backups as sensitive
credential-bearing state. They must never appear in advertisements,
availability responses, logs, errors, or non-operator output. Encrypt and
access-control them accordingly; deleting an exported backup requires a
separate retention policy and does not follow from removing the live settings
row. Common-set removal never deletes the wallet: a removed member's wallet
database and balance stay in place. After restart the wallet lazily reopens the
retained client from its durable scope prefix when queried or swept. One process
attempts that reopen only once; cancellation or failure requires restart so a
dependency task left behind cannot race a second database opener. Do not destroy
the retained scope merely because its client is dormant. Treat an API secret used
with an older FMan while debug or trace logging was enabled as exposed: rotate
or revoke it, restrict retained logs as credential material, and scrub or
destroy them under the deployment's log-retention policy.

FMan's native payout boundary returns a v1/v2 operation id only after the pinned
client's local durable operation commit. Serialized starts reject a reused v1
invoice that already names a completed operation, rather than returning its old
success as a new payout. Exact-id status and await validate the
FMan payout metadata in that wallet scope and do not request another invoice or
start another payment. A terminal Lightning rail result remains separate from
active mint change, input, or refund state machines, so destruction decisions
must continue to require both facts.

Operator payout requests carry a caller-generated request id. FMan first commits
that id with its exact wallet scope and destination snapshot in SQLite, then
serializes lookup/start within the wallet scope. Native v1/v2 metadata carries
the same id and destination snapshot. A retry in another scope fails. A retry in
the original scope
returns the stored destination snapshot, so changing the configured destination
cannot retarget an existing job. Process death or a lost response
after the native commit therefore leaves an operation that `PayoutStatus`,
`AwaitPayout`, or the repeated sweep can discover without requesting another
invoice or starting another outgoing payment. Protect and back up the FMan
SQLite database together with wallet databases at one quiesced/common restore
point: losing or restoring either side independently removes the cross-database
recovery argument. The destination binding in native metadata makes detectable
skew fail closed instead of correlating an old operation to a retargeted job.

Native await uses the terminal observed directly from the subscription; it does
not require the dependency's best-effort outcome-cache write to succeed. It
rereads the independent active-state-machine fact after terminal observation
and consults cached v1 state only for the refund distinction hidden by v1's
aggregate failure result.

## FMan guardian-fee collection

Guardian-fee revenue lives in the same Fedimint clients used for payments, under the FMan's one mnemonic-derived root
([REQ-guardian-fee-remittance](specs/REQ-guardian-fee-remittance.md)). Their
databases hold spendable ecash and the recipient account key, so they are
credential-bearing state under the same handling rules as the payment wallet's
databases above.

Fee value movement is deliberately a client capability, not a guardian one.
The vault receives the seat's public invite code, the committed recipient
account key, and ordinary client-scoping inputs; its transactions never carry
the child's admin password or invoke a guardian/admin mutation. Getting that
public invite may first probe the local child, including an authenticated
read-only phase check, and status presentation reads public config/consensus
state; report-triggered backup may also read existing guardian files. Those
discovery, presentation, and backup reads do not authorize the later client
transaction. Preserve the value-moving boundary — a guardian able to pay
itself through its own consensus authority would be a different and much worse
trust story.

Where the money is owed and how it is moved are separated by crate. The
recipient account and the fee policy a seat votes for are derived and validated
in `fman-core`, which hands the seat's account key to the collection
implementation on every call; an implementation therefore cannot choose an
account of its own. The key material crosses that boundary as a keypair with no
formatting or serialization, and its only constructor takes mnemonic-derived
bytes — review any new caller of `GuardianFeeAccountKey::keypair`.

The FI-facing fee proposal accepts at most the payer-compatible 210,000 ppm.
Every recipient crosses the protocol boundary as a complete single-signature
`BtcDepositor` account plus a repeated matching account id. FMan validates the
version-1 weighted-recipient wire, positive non-overflowing weights, strict unique AccountId
order, its own mnemonic-derived guardian account, and the deployment-pinned
Guardian Verification Fee account before touching fedimintd. The fixed policy
gives FI four shares and every guardian one share; one additional share is the
Guardian Verification Fee. FI and FMan identities are disjoint by construction,
so there is no combined FI-guardian entry; a colliding account is refused as a
duplicate. Development and staging use known-test Guardian Verification Fee
accounts from the environment profile. Never accept a caller-provided fallback
for the Guardian Verification Fee account.

Each remittance carries an accounting breakdown sealed to the recipient account
key. Decrypted breakdowns and `WithdrawGuardianFees` tokens are operator-only
output: bearer notes and their plaintext must never be logged, put in `Debug`
output, or persisted outside the client database. A remittance whose breakdown
fails to decrypt is reported with its amount and an error, never dropped —
suppressing it would hide received money.

An accepted FI-triggered claim currently uses the pinned client's generic
receive/reissue finalizer. Its built-in consolidation and denomination balancing
may select pre-existing ordinary wallet notes and charge their input/output fees
to the combined transaction. This is a known economic exposure, not a principal
isolation guarantee.

Refusals are separate: the FMan builds and signs the exact payment-funded refund
transaction from that quote's locked notes and the FI's outputs. Submitting that
transaction does not invoke ordinary-wallet funding or consolidation, and its
fees never enter accepted-seat revenue accounting.

The seat row is the sole durable admission outcome. `CreateSeat` reads the
quote's seat row before the offer epoch in one snapshot and repeats both checks
in that order after acquiring SQLite's writer reservation. An existing seat
therefore dominates an epoch change: concurrent duplicates return the same
acceptance rather than one acceptance and one refund-bearing refusal. Every
quote-invalidating epoch change commits before a refusal can depend on it, and
the deterministic refund is instantiated only after admission returns that
refusal. The
`duplicate_snapshots_resolve_to_one_acceptance_without_a_refusal` regression
forces both duplicates to observe the formerly unsafe absent-seat/current-epoch
snapshot before they contend at the writer boundary. Re-check it whenever seat
replay authority, admission ordering, offer-epoch mutation, or refund emission
changes.

The FMan's only payment-federation policy source is the authenticated
kind-37707 common set
([SPEC-locked-payment](crates/fman/specs/SPEC-locked-payment.md)):
there is no operator-curated list or admin acceptance verb. Quote acceptance,
paid availability, and wallet join reconciliation all read the retained
admitted membership, and any removal from the set draws a fresh offer epoch in
the same database transaction, refusing (and refunding) outstanding quotes
against removed members. The FMan does not publish the membership in kind
37701, `GetAvailability`, or relay tags. FI paid selection independently
consumes the same authenticated common set. Fedi does not yet publish kind
37707, so production paid setup remains unavailable until the first
publication.

FI common-set policy is authorized only by a complete kind-37707 event from the
deployment-pinned Fedi publisher. Admission verifies the event ID and signature,
author, kind, exact `d` tag, 24-hour future timestamp bound, strict
bounded content, public Fedimint invite parsing, absence of API bearer secrets,
and unique derived federation IDs. Checking content without the signed event or
accepting a self-selected author would let an attacker choose payment policy.
Transport code must bound the complete event frame before this content-bounded
admission helper performs cryptography. FI relay and CLI inputs cap a complete
normalized event at 256 KiB; one relay query observes at most 16 candidates and
retains at most 4 MiB.

FI must atomically retain the complete highest admitted event, statically
revalidate that trusted record after restart, and use its opaque admitted value
for NIP-01 rollback checks. Events do not expire: last-known-good policy remains
usable through publisher and relay outages, so an isolated FI may continue
using a federation Fedi has since removed. An accepted event up to 24 hours in
the future can block normally timestamped updates until time catches up.

The pinned Fedi publisher can select every payment federation, publish a stop
set, or direct consumers to attacker-controlled public endpoints. Key compromise
or damaging misconfiguration requires out-of-band key/configuration recovery;
the event format does not authorize its own replacement key. See
[SPEC-setup-payment-federations](specs/SPEC-setup-payment-federations.md) and the
[setup-payment publisher security notes](crates/setup-payment-publisher/SECURITY.md).
That production CLI must durably create a no-overwrite receipt, enforce current
addressable replacement order, publish the same signed event to every canonical
Production relay, and verify canonical readback before reporting success.

## Reporting

Do not open a public issue for a security-sensitive problem. Use GitHub's
private vulnerability reporting on this repository ("Report a vulnerability"
under the Security tab); the maintainer team triages those reports. If that
route is unavailable, contact the repository maintainers privately instead of
posting exploit details publicly.
