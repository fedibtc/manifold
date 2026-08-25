# CLAIM-cloud-fman-telemetry-local-data-root-startup-gates: Collector startup gates its configured local data root and identity

For the repository's production collector build without `defe-test-support`,
on a local Unix filesystem and with no hostile same-UID pathname mutation
during startup, successful cooperative startup implies that parsed runtime
configuration passed `Args::validate`; the configured data root is a real
effective-UID-owned mode-`0700` directory; known lock and SQLite paths are
regular files and, after successful open, are effective-UID-owned mode `0600`;
one cross-process exclusive `fs2` lock is held for the supervisor/HTTP lifetime
on that configured root; the key file was not group/other accessible and
yielded exactly 32 bytes; and existing state matched the environment/profile
revision, secret format, key id, and authenticated key sentinel. This property
does not establish exclusion between different roots, remote-filesystem
behavior, immutable mounts, no writes before identity rejection, or an
orchestrator replica count.

## Assumptions

- The local filesystem faithfully reports and enforces Unix ownership and mode
  metadata and `fs2` locks between cooperative processes.
- An untrusted same-UID principal does not race pathname replacement during
  startup.
- SQLite, AES-GCM, configuration parsing, and operating-system process and file
  semantics satisfy their documented contracts or fail detectably.
