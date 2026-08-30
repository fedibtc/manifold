# CLAIM-cloud-fman-telemetry-output-bounded-and-faithful: Output is bounded and faithful

For an admitted target, an authorized private scraper sees only forwarded metric
families and labels admitted by the current default-deny inventory plus the
five bounded `cloud_fman_telemetry_{target_fresh,snapshot_observation_timestamp_seconds,snapshot_stale,snapshot_rejected_rows,metrics_admission_total}`
families. The archive contains only exact validated safe-journal JSONL. Public
HTTP responses, private HTTP output, logs, traces, errors, archive paths and
metric names or labels expose no collector-held capability, Holder envelope,
endpoint, invite, cursor, incarnation, or raw rejected input. The collector does
not inject that held material into archive bytes; those bytes are the exact
upstream-approved safe-event payload and may contain fields approved at the
source. Apart from the bounded release version and hash supplied by a trusted,
badged FMan, these surfaces neither merge distinct FMan identities, accept
unbounded source cardinality, nor retime an old metric observation as fresh. Authorized
outbound Iroh requests and encrypted persistence are outside these observed exit
surfaces.

## Status

Falsified. Persisted metric snapshots are revalidated under the current policy,
peer and self IDs are bounded producer-owned dimensions, and the stderr
formatter rejects targets outside the collector namespace before applying
`RUST_LOG`. A supported forward wall-clock step during one complete poll can
nevertheless persist future-era completion and observation times. After clock
correction and ordinary lease renewal, the future `next_due_at` delays a
correcting poll, target freshness remains true beyond the real freshness
interval, and the old snapshot is eventually emitted with zero age and
`snapshot_stale=0`. No bounded or monotone wall-clock assumption excludes this
execution.

## Assumptions

- The authenticated FMan telemetry proxy returns its asserted stable seat
  selector and bounds its response as specified by
  [`SPEC-guardian-telemetry-proxy`](../../fman/specs/SPEC-guardian-telemetry-proxy.md).
- Upstream safe journals contain only event-local typed
  `safe_to_share = true` events; their source mechanism never includes ordinary
  stderr, span fields, or rendered child output. The source owns the payload
  fields of each approved event; this claim does not independently classify
  those fields.
- The configured source-version requirement covers releases compatible with
  the reviewed metric inventory. Method families are enabled only for the
  separately reviewed combined source hash with both required canonicalizers.
- Authenticated, badged FMans are trusted to report their bounded release
  version and hash; the collector preserves both as Prometheus diagnostics.
