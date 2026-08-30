# Proof: Guardian metric egress confinement

## Scope and model

Scope: `crates/fman/telemetry/src/rpc.rs`,
`crates/guardian-metrics-policy/src/lib.rs`,
`crates/service-fleet-manager/src/telemetry.rs`, and
`crates/fman/specs/SPEC-guardian-telemetry-proxy.md`.

The model starts after capability and seat authorization and quantifies over
every upstream HTTP result observable by `fetch_guardian_metrics`. The exit
channel is a successful `GuardianMetricsResponse` serialized by the telemetry
Iroh service. Fixed diagnostic strings are also treated as exit channels.

## Axioms

The claim's two execution and serialization assumptions are trusted. The
compiled source requirement identifies compatible releases, while the exact
current revision identifies the source from which the closed family inventory
was reviewed.

## Argument

1. **[code] Incompatible transport input has no response body.**
   `fetch_guardian_metrics` accepts only status 200, `text/plain`, no content
   encoding, and at most 4 MiB accumulated across every chunk. Redirects are
   disabled. Every failure returns a fixed service error before response
   construction.
2. **[code] Synchronous parsing cannot block the async reactor.** The complete
   bounded body moves into `spawn_blocking`. The policy checks a two-second
   deadline while parsing and retains fixed line, sample, family, label, input,
   and output bounds; discarded lines consume the same input/work counters.
3. **[code] Classification is exact and staged per family.** `shape` strips only
   exact generated histogram suffixes and selects only source-coded families.
   Known-denied and unknown lines are discarded before labels and values are
   rendered. Each admitted family stages samples, duplicate-series state, and
   histogram parts until final validation; taint or incompleteness discards that
   family alone.
4. **[code] Release identity and global failures fail closed.** The unique
   `app_start_ts` version must satisfy the compiled SemVer requirement, and its
   diagnostic hash must have the bounded source-hash shape. A method-label gate
   additionally requires its separately reviewed exact hash. Invalid UTF-8, an
   unisolatable empty sample name, missing/duplicate/invalid release identity,
   arithmetic overflow, or any global resource/deadline limit returns an error.
   The caller constructs no `GuardianMetricsResponse` on that path.
5. **[code] The only successful body is reconstructed projection output.**
   `fetch_guardian_metrics` drops the raw vector after `project_until`, joins
   only returned canonical samples, and constructs fixed successful metadata.
   No branch constructs a response from raw bytes.
6. **[test] Mixed hostile families preserve only unrelated good output.**
   `emits_only_independently_valid_allowlisted_families` supplies one valid
   family alongside known-denied, unknown, and locally invalid families and
   checks that rejected contents and collector-owned identity labels are absent.
   The shared policy tests additionally pin exact suffixes, duplicates,
   histogram completeness, hostile cardinality, release mismatch, inventory
   parity, and the captured real-seat projection.
7. **[code] Diagnostics have fixed cardinality and content.** Every FMan
   projection failure log and service error is a source-coded constant. The code
   never formats the response, metric name, label, value, seat id, endpoint, or
   dependency error into a projection diagnostic.

## Residuals

The claim does not hide admitted operational metrics, seat discovery, invites,
or safe-event journals from an authorized collector. During mixed rollout, an
old FMan still sends raw metrics; collector-side revalidation protects collector
storage but cannot retroactively establish this FMan egress property.

## Weakest links

Family dispatch, staging, and exit-channel enumeration remain on the `code`
rung. Tests cover adversarial representatives rather than mechanically
enumerating all byte strings. The source revision correspondence depends on the
checked inventory build and release packaging.
