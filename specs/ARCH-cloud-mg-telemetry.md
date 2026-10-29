# ARCH-cloud-mg-telemetry: Cloud MG telemetry collection

## Boundary and trust flow

One standalone cloud collector owns the complete path from verified MG
registration to private telemetry output. It accepts the existing exact-body
NIP-98 registration format and enforces freshness plus its own durable replay
protection. It uses the shared
[`SPEC-peer-badge-verifier`](../crates/peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)
contract to verify the complete current Holder authority chain, credential,
holder proof, schema, environment issuer policy, revocation state, and minimum
trust, and requires the verified subject to match the NIP-98 signer. Only then
does it store the submitted Iroh endpoint and MG-wide capability as credential
material. It polls that MG directly over the authenticated
[`SPEC-guardian-telemetry-proxy`](../crates/mg/specs/SPEC-guardian-telemetry-proxy.md)
ALPN. Push-gateway's operator metrics adapter is not in this path.

Admission rejects a capability generation below the target's durable high-water
mark. A same-generation heartbeat is idempotent only when its capability is
unchanged; it may update the signed Iroh endpoint and then advances the
registration revision that fences in-flight work. Neither form resets journal
identity or cursor state.

The authenticated MG identity is the canonical Nostr public key established by
registration. A seat id and formed invite returned over that MG's
capability-authenticated Iroh connection bind the scrape selector to an
operational federation id. This is the authenticated MG's assertion; it does
not independently prove guardian membership or the child's configuration.

The collector collects only:

- metric families and labels admitted by the exact, default-deny
  [privacy inventory](../docs/telemetry/metrics-privacy-inventory.md); and
- exact JSONL records from MG and retained-seat journals populated by typed
  event-local `safe_to_share = true` tracing events.

Ordinary process logs, stderr, span fields, rendered child output, and unlisted
metric families are outside this boundary. MG's journals remain best-effort:
their 5 MiB retention bound, nonblocking producer, and occasional drops are an
accepted source property, not something the collector can repair.

The collector's target database, WAL, bearer capabilities, archive, and backups
are confidential. Capabilities, Holder envelopes, endpoints, invites, journal
selectors, incarnations, cursors, and raw rejected inputs never enter logs,
traces, errors, metric labels, or public operator surfaces. The one exception
is the federation export described below, which returns invites on the private
listener only.

## Metrics ownership and identity

MG applies the compiled checked inventory under bounded resources before
transport. The collector independently applies the same source policy to remain
safe with older raw-response MGs, then adds this exact target identity to
every admitted guardian series:

- `fman_id`: the lowercase hexadecimal canonical MG Nostr public key verified
  at registration;
- `fman_name`: the deterministic lowercase `MgName` derived from that key,
  used only for display and never for series identity or merging;
- `guardian_seat_id`: the canonical seat id returned by the authenticated MG;
  and
- `federation_id`: the canonical lowercase 64-hex id derived from the invite
  asserted for that exact seat by the authenticated MG.

The key is already a public MG identity, the possibly colliding name is its
bounded human-readable fingerprint, and the seat id is bounded collector input
from an authenticated MG. `fman_id` and `guardian_seat_id` are stable across
ordinary restart and capability rotation and operationally identify the source.
The collector stores the federation id with the successful seat snapshot and
rejects producer-owned collisions, missing or malformed current invites, and
persisted metadata/sample mismatches. A later successful poll replaces the
snapshot and its federation attribution together. The seat-scoped observation-time and stale
metrics carry all four labels; MG-global freshness and collector-global
metrics do not carry `federation_id`.

The collector must not substitute opaque-only target ids, caller-provided
display names, full invite data, capabilities, endpoints, journal data, or
other unbounded values. This attribution is suitable for operational grouping,
not authorization, billing, disputes, or independent membership attestation.

`peer_id` and `self_id` remain bounded producer-owned metric dimensions. They
are neither collector-owned source identity nor collector-verified federation
configuration. Consumers must use the authenticated `fman_id` plus asserted
`guardian_seat_id` and `federation_id` for operational source identity and must
not treat any of these labels as independent membership attestation.

Metrics are deliberate observations at a configured cadence of either 15 or 30
minutes. The collector exposes its current admitted samples on a private
Prometheus `/metrics` path with the actual successful collection time as each
sample timestamp. Repeated exposition preserves that timestamp; the collector
never rewrites an old observation with the current scrape time or fabricates a
fresh last value.

A Prometheus server or Agent scrapes this private path with
`honor_timestamps: true` and `track_timestamps_staleness: true` and owns WAL,
TSDB, staleness, and remote-write behavior. Grafana
queries that Prometheus-compatible backend, not the collector exposition path.
Registration heartbeat cadence does not trigger or alter metric observation
cadence. Attempt start durably records the next deadline. After a collection
wave, the worker sleeps only until the earliest durable deadline, so a long
wave does not add another full cadence. The collector commits that cadence
reservation before arming the target deadline. The deadline covers resolution,
authenticated fetch, and cooperative bounded parsing; a reserved final phase
runs the bounded SQLite snapshot commit without cancellation so earlier
successful seats survive a later timeout. Shutdown and a fatal sibling stop
new target scheduling but join reservations and snapshot commits already in
their durability segment. Local contention before a reservation commits never
contacts the MG and backs off for one cadence in-process.
The reviewed pin and module set define the current default-deny inventory, but
they do not define which MGs the collector may contact. The collector does
not compare MG, Fedimint, or `app_start_ts` release metadata with a configured
release: it admits every independently valid family in the reviewed default-deny
policy and discards only unknown or invalid families. Method-labelled API
families require an exact compiled canonical-method allowlist, not a release
match. A missing, duplicate, or invalid
release-marker family cannot suppress another valid family, and journal polling
is independent of metric admission. The fixed telemetry ALPN
`fedi/fman/guardian-telemetry/1` is the protocol compatibility boundary; a
future incompatible wire protocol needs a separately negotiated ALPN/version,
not a release allowlist. Pin or module-set changes require a new inventory only
to expand or alter the allowed metric shapes; future families never enter by
wildcard.

The private listener is a deployment trust boundary, not an Internet-facing
API. Operators must place it on loopback or an access-controlled private
network; it carries health, readiness, `/metrics`, the federation export, and
the seat report exports, while the separate public listener carries only
authenticated registration. The collector fails closed
when configured with a non-loopback private address unless the operator
explicitly asserts that deployment isolation exists. That assertion makes the
boundary visible in runtime configuration; it does not prove the external
control.

## Federation export

Each admitted seat snapshot also keeps, AEAD-encrypted under the store key, the
canonical invite the authenticated MG asserted for that seat. The ciphertext is
bound to the deployment profile, key identifier, target, seat, and federation
id, so it does not authenticate in any other row. The invite shares the row's
lifecycle: an unlisted seat, a quarantined or expired target, and a metrics
policy change remove it together with the snapshot, and the next admitted poll
restores it. The canonical form is bech32m. A seat whose invite parses but is
too long for bech32m keeps its federation id and metrics and stores no invite.

The private listener serves `GET /v1/federations`. It returns, per federation
id of an active and unexpired target's current seats, the invite of the most
recently observed seat that stored one, the count of those seats, and the
latest observation time. Like the rest of the private listener it has no
application-layer authentication, and the public listener never routes it.
Every caller the deployment admits to the private listener, including the
Prometheus scraper, health probes, and any process sharing a loopback network
namespace, can read every exported invite; encryption at rest does not narrow
that set. Restricting invites to a single consumer needs a separately isolated
listener or route-level authorization, which the collector does not provide. The export
lets a deployment-owned consumer, such as a Fedimint Observer, connect to the
federations whose MGs report telemetry; the invite is the same assertion that
already attributes the seat's metrics and carries no stronger guarantee.

`/ready` reports only local store and serving health. One stale or unreachable
MG cannot remove registration and exposition for healthy targets from service.
The private exposition reports each seat's observation timestamp and stale bit.
It also reports `seat_fresh` for every seat of the MG's latest successful seat
listing, including a seat with no observation yet. `seat_fresh=1` needs a
snapshot that passed the same persisted-row revalidation as the forwarded
samples, so a rejected, future-dated or missing observation reports 0.
`snapshot_stale=1`, `seat_fresh=0` and `target_fresh=0` use the same threshold:
strictly more than two configured polling intervals since the relevant
successful observation or complete target poll. Exposition refreshes that derived state
within 30 seconds. Responses for one revision share one immutable allocation.
A revision cannot allocate its replacement until every response retaining the
old backing has gone away; scrapes receive HTTP 429 during that bounded
transition. Cache identity also includes the nearest active lease expiry, so a
target without a snapshot disappears from health exposition at expiry.
Each new cache generation reads its revision, nearest active lease expiry,
eligible snapshots, and eligible target health from one transaction at one
captured time; reuse validates that cache identity transactionally. Quarantine
and lease expiry suppress snapshots from every scrape ordered after that
lifecycle transition. A scrape that overlaps the transition may finish with its
coherent pre-transition body; it never combines lifecycle states from opposite
sides of the transition. Renewal after expiry starts without the previous
snapshot. The collector bounds all persisted metric state to 64 MiB and 200,000
samples. A commit that would exceed the bound stores the snapshots that fit.
Within one commit it applies replacements of already stored seats before new
seats, so new seats do not take capacity ahead of them. A snapshot that does not
fit, new or replacement, is skipped, and its seat keeps its old observation
time. The collector admits one aggregate scrape at a time. It retains latest snapshots only; Prometheus retains metrics history.

## Seat reports from safe journals

Each MG journals a `seat capacity` event on every advertisement cycle and a
`seat readiness check completed` event after every readiness run. The MG owns
both shapes: `SeatCapacityEvent` and `SeatReadinessEvent` in
`service-manifold-guardian` name their messages and fields, the MG emits them
under those names, and MG tests decode the emitted records with them. Releases
before 0.1.4 journal only the free seats, the accepting flag and the operator
limit, and no uptime outcome, scores or minimums; releases before 0.1.9
journal no `version` outcome. Absent fields stay absent.

While validating a batch from the MG journal, the collector keeps the last
record of each kind, in journal order, that has an RFC 3339 timestamp and
whose fields validate: counts as unsigned 32-bit integers, codes and versions
as short bounded ASCII, and percentages from 0 to 100. A capacity record also
needs at least one stated limit, free seats no greater than any of them, and
a policy limit no greater than the most seats a setup-payment policy allows
one MG. Every release states a limit. Releases before 0.1.4 state only the
operator limit, which has no upper bound. These checks
bound what one MG can assert and make its counts
agree with each other; they cannot show that the counts are true. Seat
journals, other records and invalid records are ignored. Decoding never
changes the archived bytes or rejects a batch. An MG whose release journals
neither event has no report and is absent from the exports: absence means no
report, not an offline MG.

A report's observation time is its event timestamp, or the reception time
when that is earlier. The event time is MG-asserted, so it can make a report
older but never fresher than its arrival, and a backlog fetched after an
outage does not look current. The commit that stores a batch's cursor also
stores the target's report of each kind, unless the stored one was observed
later. Reports are not deleted when a target expires or is quarantined; they
leave the exports while the target is not active and unexpired, and return
with their original observation time when it is.

The private listener serves `GET /v1/seat-capacity` and
`GET /v1/seat-readiness`. Each returns `schema_version`, `generated_at` (the
response time, in whole seconds) and one report per active, unexpired target
that has one, ordered by MG public key: `mg_pubkey`, `observed_at`, and the
validated fields, with an absent field as `null`. A report observed after the
response time and a stored row that no longer validates are left out. A store
failure answers 503. The stored row and the response share one serde shape,
which is a persisted format and decodes through the MG's event type, so the
event field names also name the stored and exported fields. The routes use a
`schema_version`, `generated_at` and `reports` envelope, unlike
`/v1/federations`, so that a consumer can reject an unknown schema and judge
report age against the response time. These routes return MG-asserted values, the
target's public key and collector times; they hold no invite, capability,
endpoint or journal position.

## Journal continuity and archive commit

The safe-journal cadence is configured independently of metrics. Its default is
five minutes rather than the 15- or 30-minute metrics cadence.
Within a cadence, each target holds a concurrency permit for at most a
30-second work budget (plus completion of the current bounded durability
operation). Fetches rotate across listed streams before giving any stream
additional backlog capacity. Retention scans run at most once for each advancing
UTC cutoff day and retry after a failed scan.

Each source journal has a persisted UUIDv7 incarnation. The source keeps it
across ordinary process restart and segment rotation and creates a new value
when the journal storage is recreated or replaced. Keeping an incarnation also
promises that segment numbers are durably reserved, monotone, and never reused.
After any reopen or crash repair, the writer reserves and creates a fresh segment
before writing; it never appends to a segment that existed before that reopen.
Incarnation identity uses full-value equality; embedded timestamp ordering is
informational only.

A supported restore either excludes safe-event journals or gives every restored
journal a new incarnation before telemetry resumes. An incarnation stored on the
same volume cannot reveal an arbitrary rollback of that whole volume to a
self-consistent older snapshot. The operational restore procedure, not the token,
must force the new incarnation in that case.

For each stream, the collector persists the source incarnation, source cursor,
archive offset and frame hash, and its own monotone `observed_generation`.
Seeing a different source incarnation increments `observed_generation`, starts
from that incarnation's oldest retained record, and records a continuity gap. A
source `continuity_gap = true` under the same incarnation also increments
`observed_generation` and records the gap before adopting the returned cursor.
Capability or endpoint rotation alone changes none of this state. Gap metadata
lives in SQLite's stream/frame ledger, not as a fabricated safe-event record in
the byte-exact JSONL archive.

The archive path is:

```text
logs/<journal-stream>/<UTC-reception-day>.jsonl.zst
```

Reception time, not an untrusted event timestamp, chooses the day. The collector
preserves the fetched safe JSONL bytes exactly and appends one independent zstd
frame per fetch batch; the frames in a daily file are therefore a valid
concatenated-zstd stream.

Before using a new daily file, the collector durably creates its directory entry
by syncing the parent directory (or uses an equivalent precreation invariant).
For every nonempty batch, it appends the complete frame and `fdatasync`s the file
before atomically compare-and-swapping the SQLite stream state from the
requested incarnation/cursor to the returned incarnation/cursor, recording the
committed offset and hash in the same transaction. It never advances a cursor
for bytes that are not durable. Recovery truncates or ignores uncommitted archive
tails and may refetch an already durable batch; duplicate records are preferable
to loss. Byte-identical records are not deduplicated because distinct legitimate
events can serialize identically.

The configured archive quota is a process-wide hard bound. An append that would
cross it is rejected before writing, leaves that stream's cursor unchanged, and
defers only the affected target's collection for the current cycle. The
collector keeps both listeners and other target work available, but the shared
full archive also rejects their nonempty journal batches until retention or
operator action frees capacity. Archive I/O, recovery, integrity, and
indeterminate-write failures remain collector-fatal.

## Deployment boundary

The runtime is single-active. One process exclusively owns one SQLite
database and its archive on the same encrypted persistent volume, so the cursor
and file commit invariant does not cross storage systems.

Deployment cadence, target lease policy, Prometheus backend and remote-write
topology, infrastructure-as-code location, archive retention/capacity, backup
policy, and encryption-key delivery and rotation remain configurable production
decisions. Multiple active collectors, a remote database, or object storage
would invalidate the local commit boundary and require a new coordination
architecture.

[`cloud-collector-deployment.md`](../docs/telemetry/cloud-collector-deployment.md)
is the single secure-deployment entry point. It enumerates the external
environment controls that the production-readiness assessment assumes, including
an explicitly non-evidential staging-manifest comparison. Repository artifacts
and documentation do not verify that any particular deployment supplies those
controls.

The production-readiness evidence hierarchy starts at
[CLAIM-cloud-mg-telemetry-production-ready](../crates/cloud-mg-telemetry/specs/CLAIM-cloud-mg-telemetry-production-ready.md).
Implementation, package, and test coverage do not by themselves establish its
conclusions or a real production deployment.
