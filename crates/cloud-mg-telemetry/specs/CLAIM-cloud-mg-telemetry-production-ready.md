# CLAIM-cloud-mg-telemetry-production-ready: Cloud MG telemetry is ready for production

Within its documented single-active deployment and supported release envelope,
the cloud MG telemetry collector admits only authorized MG targets, confines
its private output to faithful approved telemetry, preserves every fetched
safe-journal batch before advancing its durable source position, and contains a
failing or hostile target within the documented resource and freshness bounds.
Ordinary restart preserves those properties. A supported coupled backup and
restore recovers the state and archive at its chosen recovery point or keeps the
collector out of traffic; telemetry accepted after that recovery point is
outside its recovery guarantee.

## Status

Unverified.

## Assumptions

- [CLAIM-cloud-mg-telemetry-admission-confined](CLAIM-cloud-mg-telemetry-admission-confined.md)
- [CLAIM-cloud-mg-telemetry-output-bounded-and-faithful](CLAIM-cloud-mg-telemetry-output-bounded-and-faithful.md)
- [CLAIM-cloud-mg-telemetry-archive-cursor-consistent](CLAIM-cloud-mg-telemetry-archive-cursor-consistent.md)
- [CLAIM-cloud-mg-telemetry-target-failures-contained](CLAIM-cloud-mg-telemetry-target-failures-contained.md)
- [CLAIM-cloud-mg-telemetry-production-deployment-envelope](CLAIM-cloud-mg-telemetry-production-deployment-envelope.md)
- The collector process in the actual deployment executes the entrypoint and
  runtime code carried by an image whose manifest and referenced configuration
  and layer blobs are byte-for-byte those in the exact Nix OCI archive selected
  and checked at this reviewed repository revision. The deployment selects that
  image by immutable registry digest, the archive was built from the same source
  revision, and no entrypoint override, mount, injected executable or library,
  or other code-affecting runtime substitution replaces or modifies that code.
- The actual deployment otherwise follows the release envelope: one active
  process owns the SQLite database, WAL, and archive on one encrypted persistent
  volume; the runtime service and explicitly trusted provisioning and backup
  identities alone can read live data or key material; only explicitly trusted
  backup identities can read backup copies; and only authorized probes and the
  scraper can reach the private listener.
- The actual Prometheus-compatible backend preserves collector timestamps with
  `honor_timestamps: true` and `track_timestamps_staleness: true` and owns WAL,
  TSDB, staleness, and remote write. Grafana queries that backend.
- Backup and restore stop the collector, copy SQLite, WAL, and archive from one
  recovery point with the matching key, restore them together to a private empty
  volume, and admit traffic only after startup recovery and readiness succeed.
