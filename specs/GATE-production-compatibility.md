# GATE-production-compatibility: Production compatibility

## Gate

Manifold network interfaces and persisted formats intended for production use
must remain backward compatible. Persistence-schema changes must preserve
existing state through migrations.

Rust source-level public APIs are outside this gate. A Rust API change that also
changes a network interface or persisted format remains subject to the
compatibility requirements for that interface or format.

Any incompatible network-interface or persisted-format change requires explicit
human approval as an exception. The commit and pull request descriptions must
both document the approved exception.

## Justification

Manifold is approaching production-like deployments. Operators' state and
integrations must survive future upgrades; the earlier assumption that they
could be discarded or changed incompatibly is no longer acceptable.
