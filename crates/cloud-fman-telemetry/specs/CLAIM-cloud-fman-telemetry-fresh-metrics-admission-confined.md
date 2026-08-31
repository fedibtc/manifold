# CLAIM-cloud-fman-telemetry-fresh-metrics-admission-confined: Direct metric admission is shape-confined and bounded

For arbitrary metrics response bytes and `MetricsIdentity` values passed to one
call of `MetricsPolicy::admit_until`, a successful result cannot contain an
explicitly reviewed-deny or unknown metric family, an input label key outside
an admitted family's exact policy shape, an input label value outside its
source-coded bound, an input-supplied collector identity label, a duplicate
series, an incomplete histogram, a non-finite or sign-negative value, more than
20,000 samples, or more than 2 MiB of rendered samples. Each result adds exactly
the four collector identity keys with the values supplied in `MetricsIdentity`.
Absent one of the global failures below, an unknown, reviewed-deny, or locally
invalid admitted family cannot suppress an unrelated independently valid
admitted family. Complete failure is limited to invalid UTF-8 or an
unisolatable family boundary, global resource/deadline exhaustion, or failure
to render within the global output bound. A missing, duplicate, or invalid
release-marker family is a family-local discard.

“Admitted” means the source-coded `shape` table in this revision. Exact
source-coded reviewed-deny families are discarded in full before their labels
or values are parsed; every unclassified name is also discarded. This
direct admission claim does not assert that either table is complete or faithful to the
Markdown inventory, that a `peer_id` belongs to configured peers, that persisted
snapshots are revalidated before exposition, when the response was observed, or
how admitted samples are subsequently stored or rendered.

## Assumptions

- The process and its dependencies execute the reviewed production Rust without
  memory corruption or code injection.
