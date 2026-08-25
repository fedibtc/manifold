# Cloud collector local testing

The focused crate suite exercises the collector at its durable and network
boundaries:

- typed journal-value tests reject malformed, unsafe, spanned, oversized, and
  path-hostile source data before archive code can observe it;
- fake-source orchestration tests drive the production catalog, collector,
  SQLite CAS, archive, cancellation, bounded draining, and restart seams;
- archive tests decode concatenated zstd frames, reopen committed boundaries,
  truncate orphan tails, serialize quota admission under contention, and remove
  expired empty stream directories;
- store tests reopen SQLite and race revision/cursor CAS operations;
- direct-Iroh integration tests use the production ALPN and bounded RPC client
  against an in-process typed server, rather than substituting an HTTP or
  untyped journal protocol;
- metrics policy tests exercise exact family, label, bucket, release, method,
  duplicate-series, completeness, cardinality, and byte gates;
- metrics store tests cover durable attempt deadlines, restart, policy reset,
  revision fencing, post-reservation resolution failure, expiry/renewal,
  partial-seat replacement, timestamps, and target freshness;
- metrics lifecycle fault tests pause before reservation and snapshot COMMIT,
  then prove shutdown and a fatal sibling join the transaction before reopen;
- private-route and Prometheus-parser tests cover local-only readiness, content
  type, grouped family metadata, explicit timestamps, stale metadata, and the
  single live response generation across rapid revisions and slow readers,
  including health-only target expiry.

Run the focused suite with:

```console
cargo test -p fedi-decentralized-cloud-fman-telemetry
```

That command runs the pure focused suite. The real-daemon test needs the
explicit test-only feature and a Defe server. Run it through the bounded
one-shot harness:

```console
./scripts/test-e2e-local.sh -E 'binary(daemon_e2e)'
```

The release packaging contracts are checked through Nix:

```console
nix build .#ci.x86_64-linux.cloudFmanTelemetryCliContract
nix build .#ci.x86_64-linux.cloudFmanTelemetryOciImage
```

Deployment, Prometheus, backup, and archive-recovery procedures live in
[`cloud-collector-deployment.md`](cloud-collector-deployment.md).

The suite deliberately does not claim to prove storage-hardware durability.
`fdatasync`, directory `fsync`, SQLite `synchronous=FULL`, filesystem ordering,
and power-loss behavior retain their operating-system and hardware contracts.
Fault tests prove the collector's ordering and recovery decisions at those
calls: append and sync precede the cursor CAS, a stale CAS rolls its frame back,
and restart truncates bytes beyond the last committed hash boundary. Deployment
qualification should add destructive power-cut testing on the actual storage
stack.
