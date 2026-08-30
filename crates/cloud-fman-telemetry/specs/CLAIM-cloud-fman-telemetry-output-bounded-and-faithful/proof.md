# Proof: Cloud FMan telemetry output bounds and fidelity

> **Falsified proof:** Persisted-row revalidation, bounded producer-owned peer
> dimensions, and the collector-target formatting boundary constrain those
> disclosure paths. The wall-clock discontinuity below still retimes an old
> observation as fresh.

## Scope and model

Scope: `crates/cloud-fman-telemetry/src/*.rs`,
`crates/cloud-fman-telemetry/migrations/*.sql`,
`crates/cloud-fman-telemetry/tests/*.rs`,
`docs/telemetry/metrics-privacy-inventory.md`,
`crates/fman/specs/SPEC-guardian-telemetry-proxy.md`,
`specs/ARCH-cloud-fman-telemetry.md`, `SECURITY.md`.

This leaf has no claim imports. It quantifies over authenticated FMan responses,
with their bounded release identity granted by the claim's trust assumption,
persisted snapshots and stream state, public registration responses,
authorized private HTTP responses, process logs/traces/errors, archive
paths/bytes, and emitted metric names and labels. It excludes encrypted
persistence and capability-bearing authenticated outbound Iroh requests, which
are authorized storage and transport rather than disclosure exit surfaces.
Source-approved safe-event payload fields are granted input; the archive claim
is that the collector validates and preserves them exactly without injecting its
held registration or cursor material.

## Axioms

The authenticated FMan proxy and upstream safe-event mechanism satisfy the
assumptions in
[the claim](../CLAIM-cloud-fman-telemetry-output-bounded-and-faithful.md). The
configured source-version requirement and canonical-method-label switch
identify the reviewed compatibility envelope; only that separate method gate
uses an exact reviewed source hash. The badged FMan's bounded version and hash
are trusted release diagnostics. Source approval, rather than this proof,
classifies the payload fields inside each safe event.

## Argument

1. **[test] Forwarded metrics default deny.** The metrics-policy tests force
   exclusion of unknown families, labels, buckets, duplicate/incomplete families,
   identity overrides, malformed input, hostile cardinality, and unreviewed
   suffix variants. The real-seat replay verifies that exact reviewed-deny
   families, including raw-method families, are discarded without entering the
   result while unknown and locally invalid families cannot suppress unrelated
   valid families. The tests verify
   insertion of canonical FMan, asserted seat, and invite-derived federation
   identity for current allowed
   families.
2. **[test] Collector metrics inventory.** Snapshot parser tests accept exposition
   containing the five named `cloud_fman_telemetry_*` families. The renderer
   constructs only those generated families in addition to already-filtered
   forwarded samples. Admission diagnostics use only six fixed event labels;
   the rate-limited warning has one fixed reason and no source-derived field.
   Complete call-site enumeration remains `enum`.
3. **[test] Journal default deny.** Journal-type tests accept bounded complete
   safe JSONL and reject malformed, oversized, spanned, or unmarked records.
   Archive tests establish byte-exact concatenated-frame decoding after
   validation. The collector neither interprets nor augments approved payload
   fields before that byte-exact append.
4. **[test] Snapshot fidelity.** Snapshot tests cover colliding display names,
   preserved observation timestamps across repeated scrapes, stale state as
   metadata rather than retiming, exact-boundary freshness, bounded persisted-row
   rejection diagnostics, parser-valid exposition, and remote-failure isolation.
   They do not cover a forward wall-clock step and correction spanning
   collection, durable scheduling, target health, and later exposition.
5. **[test] Persisted metrics revalidation.** Metrics-policy and store tests
   require the exact FMan/seat/federation identity labels, remove only those labels,
   then re-admit under the current release, family, label, series, histogram, and
   resource policy before accepting byte-identical canonical output. Same-policy
   restart tests omit hostile identity, label, duplicate-series, cardinality, and
   aggregate-overflow rows while retaining a valid neighbor and its original
   observation time.
6. **[code] End-to-end origin and bounds.** Pollers construct identity and
   successful observation time from the authenticated target and completed
   fetch, the store persists bounded snapshots under revision/seat CAS, and the
   renderer reads only rows that the current policy revalidated.
   `Store::metric_exposition` returns cache identity, eligible snapshots, and
   eligible health from one transaction at one captured time, so the renderer
   cannot combine identity or samples from opposite sides of a lifecycle
   transition. The focused
   `metrics_exposition_and_quarantine_have_one_lifecycle_order` test pins that
   transaction joint. These joints are not established by the renderer tests
   alone.
7. **[test] Nonmatching trace targets are excluded.** The stderr formatting
   layer intersects the operator-controlled `EnvFilter` with the exact collector
   crate-target namespace. The
   `permissive_filter_cannot_render_dependency_endpoint_or_capability` test uses
   the production filter composition with `trace`, supplies endpoint- and
   capability-like fields on selected Iroh span and event targets, proves they
   do not render, and proves an owned warning still does. This mechanism filters
   `Metadata::target`; it does not establish cryptographic crate provenance for
   arbitrary target strings.
8. **[enum] Exit closure.** The 24 production Rust files contain four HTTP routes
   and 13 response shapes; six collector tracing call
   sites; the single validated archive content writer and its bounded path
   constructors; 51 admitted source metric families plus exactly five generated
   families; two Iroh connection sites; and four typed capability-bearing RPC
   sends. They contain no additional concrete held-material output path. Startup
   `Result` termination and the panic hook remain weaker, unexercised exits, so
   this enumeration cannot establish the broad claim.

## Counterexample

Use the supported 900-second metric cadence, its 1,800-second freshness
interval, and the default 3,600-second registration lease.

1. Register a target at wall time `T`. While it remains eligible for a due
   poll, step wall time forward by 3,500 seconds.
2. `Store::reserve_metric_attempt` accepts the target and durably sets
   `next_due_at` to the stepped time plus 900 seconds. The complete poll then
   gives its snapshots future-era observation timestamps, and
   `Store::commit_metrics` stores the stepped time as `last_complete_at`.
3. Correct wall time to `T`. Ordinary idempotent registration renewal extends
   the lease without clearing metric state because the lease has not expired.
   The future `next_due_at` prevents a correcting poll until wall time reaches
   `T+4400`.
4. More than 1,800 real seconds after the poll,
   `Store::metric_exposition` still marks the target fresh because it checks
   only `last_complete_at >= now - stale_after`, not
   `last_complete_at <= now`. It initially rejects the future-dated snapshot.
5. When corrected wall time reaches the stored observation time, the snapshot
   becomes eligible without a correcting poll. `render_metrics` computes age
   from the now-equal wall-clock timestamps and emits the approximately
   3,500-second-old observation with age zero and `snapshot_stale=0`.

This violates the claim that an old observation cannot be retimed as fresh.
The claim has no bounded or monotone wall-clock axiom that excludes the
execution.

## Evidence anchors

The named metrics-policy, journal-type, archive, snapshot, store snapshot/CAS,
poller, authentication-sanitization, registration-handler, and logging-filter
tests in the scope are focused unit/component evidence. The daemon E2E observes
one accepted raw family and one exact archive record for one healthy target; it
does not exercise hostile output, collision, bounds, stale transition, or
credential exit-channel cases end to end.

## Residuals

FMan seat and invite-derived federation identity are the authenticated FMan's
assertion, not independent federation attestation. Stable canonical FMan, seat,
and federation identities deliberately
appear to an authorized private scraper. Prometheus retention, query access,
remote write, and Grafana disclosure belong to the production deployment.
Authorized outbound Iroh requests and encrypted credential persistence carry
capabilities by design. A source release, metric inventory, or method-family
change requires a new exact review.

## Weakest links

The upstream safe-event/source properties are axioms. End-to-end timestamp and
identity provenance remain on the `code` rung. The exit inventory remains on
the `enum` rung; startup `Result` termination and the panic hook were not
exercised, and target strings do not prove crate provenance.
