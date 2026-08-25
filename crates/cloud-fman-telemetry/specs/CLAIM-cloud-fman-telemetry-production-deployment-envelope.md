# CLAIM-cloud-fman-telemetry-production-deployment-envelope: Repository artifacts bound the collector deployment evidence

At this revision, the repository's configured cloud-collector release path
selects one Nix OCI archive whose declarations and defaults are inspected by an
explicitly invocable Nix check; a collector that successfully starts applies
the stated local configuration, data-root, lock, key, and persisted-identity
gates under explicit filesystem, SQLite, and cipher premises; and the
deployment material identifies the remaining storage, replica, listener,
key-custody, source, Prometheus, backup, capacity, and shutdown controls as
operator or external-system premises. This property does not assert that a
publish workflow ran, a registry serves the archive, a deployment selected it,
or any external premise is satisfied.

## Assumptions

- [CLAIM-cloud-fman-telemetry-collector-oci-archive-declarations](CLAIM-cloud-fman-telemetry-collector-oci-archive-declarations.md)
- [CLAIM-cloud-fman-telemetry-local-data-root-startup-gates](CLAIM-cloud-fman-telemetry-local-data-root-startup-gates.md)
- [CLAIM-cloud-fman-telemetry-operator-deployment-premises-documented](CLAIM-cloud-fman-telemetry-operator-deployment-premises-documented.md)
