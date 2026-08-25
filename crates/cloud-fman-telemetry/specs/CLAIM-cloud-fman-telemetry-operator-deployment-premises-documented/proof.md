# Proof: Cloud FMan telemetry operator deployment premises are documented

## Scope and model

Scope: `docs/telemetry/{cloud-collector-deployment,cloud-collector-testing}.md`,
`packages/cloud-fman-telemetry/config.example.env`, `SECURITY.md`,
`specs/ARCH-cloud-fman-telemetry.md`, and this claim and proof.

The predicate is documentary: the shipped material identifies which controls
the repository does not enforce locally. It does not quantify over production
deployment behavior or infer compliance from instructions.

## Axioms

The distinction in
[the claim](../CLAIM-cloud-fman-telemetry-operator-deployment-premises-documented.md)
between a documented premise and its fulfillment is trusted. Documentation
alone cannot establish an external control.

## Argument

1. **[code] Storage and process topology.** The deployment guide requires one
   active container on one commonly mounted encrypted persistent volume,
   specifies ownership/mode provisioning, and rejects remote or split storage,
   shared filesystems, rolling overlap, and multi-active replicas.
2. **[code] Key custody and listener isolation.** The guide assigns read-only
   key delivery and runtime/provisioning/backup custody, volume and backup
   encryption, and key rotation to deployment systems. It requires actual
   network policy/firewall isolation and a negative reachability test for the
   unauthenticated private listener; the daemon boolean is only an assertion.
3. **[code] Source and external telemetry.** The guide requires deployed source
   pins and canonicalizer correspondence beyond local string validation. It
   specifies Prometheus timestamp/staleness settings and assigns TSDB, WAL,
   remote write, and Grafana query ownership to the external backend.
4. **[code] Backup, capacity, and lifecycle.** The guide requires a stopped,
   coupled SQLite/WAL/archive/key recovery point; private empty-volume restore;
   storage headroom and poison/capacity response; readiness before traffic; and
   75-second termination grace.
5. **[code] Qualification boundary.** The testing guide leaves destructive
   storage-stack qualification to the deployment, and the architecture keeps
   orchestrator, storage, secret-manager, network, Prometheus, and backup
   choices outside the collector.
6. **[enum] Premise closure.** The claim enumerates storage/topology,
   provisioning/custody, listener, source, telemetry backend, backup, capacity,
   qualification, readiness, and shutdown. Each is stated as required operator
   or external-system behavior rather than repository enforcement.

## Evidence boundary

The material includes example configuration and a read-only comparison of
external staging manifests. It does not establish that an orchestrator, secret
manager, storage system, network policy, Prometheus backend, backup system, or
operator follows it.

## Residuals

An actual deployment may violate every documented premise without falsifying
the documentary predicate. Missing publication and deployment activation are
also outside it.

## Weakest links

The documentary inventory is an `enum` over prose and remains weaker than
machine-checked deployment evidence. Reader interpretation is axiomatic.
