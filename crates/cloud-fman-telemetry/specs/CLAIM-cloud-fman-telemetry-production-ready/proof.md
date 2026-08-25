# Proof: Cloud FMan telemetry production readiness

## Scope and model

This composition supports
[CLAIM-cloud-fman-telemetry-production-ready](../CLAIM-cloud-fman-telemetry-production-ready.md).
Its local scope is that claim, this proof, and
`specs/ARCH-cloud-fman-telemetry.md`. It imports only the five immediate claim
properties linked by the claim. Child proof scopes and axioms are not imported.

The model quantifies over one supported release, one actual single-active
deployment, every admitted target within its declared bounds, every public and
private collector exit surface, ordinary process restart, and one coupled
backup/restore recovery point. It admits arbitrary remote registration and
telemetry bytes, slow or unavailable targets, concurrent registrations and
scrapes, cancellation, and crashes. It does not quantify over state accepted
after a chosen backup recovery point.

## Exact imports

The imports are the complete property paragraphs, with every adversary,
quantifier, bound, exception, and time qualification preserved, from:

1. [admission confinement](../CLAIM-cloud-fman-telemetry-admission-confined.md);
2. [bounded faithful output](../CLAIM-cloud-fman-telemetry-output-bounded-and-faithful.md);
3. [archive/cursor consistency](../CLAIM-cloud-fman-telemetry-archive-cursor-consistent.md);
4. [target-failure containment](../CLAIM-cloud-fman-telemetry-target-failures-contained.md);
   and
5. [the release artifact envelope](../CLAIM-cloud-fman-telemetry-production-deployment-envelope.md).

No paraphrased subset replaces an imported property.

## Additional axioms

The deployed-artifact correspondence, actual-deployment,
external-Prometheus, and coupled-backup assumptions in the root claim are
trusted here. They establish artifact selection, operator behavior, and
external infrastructure that release artifacts cannot enforce.

## Argument

1. **[claim] Authorized inputs.** Import 1 supplies every registration-owned
   authority mutation clause in the conclusion.
2. **[claim] Confined outputs.** Import 2 supplies the inventory,
   confidentiality, source-identity, cardinality, timestamp, and stale-value
   clauses for the claim's exact exit surfaces.
3. **[claim] Durable journal progress.** Import 3 supplies the collector-owned
   archive/cursor ordering and explicit source-discontinuity clauses, including
   its retention boundary.
4. **[claim] Bounded service.** Import 4 supplies cross-target isolation, local
   readiness, resource/freshness bounds, and joined durability on shutdown.
   Together with import 2 it supplies lifecycle-consistent exposition: an
   overlapping scrape may finish with its coherent older body, while a scrape
   transaction ordered after quarantine or expiry cannot expose the target.
5. **[claim] Repository deployment evidence.** Import 5 supplies checked image
   declarations/defaults, assumption-bounded local startup gates, and
   documentation of the remaining operating premises. It supplies no actual
   publication or deployment compliance.
6. **[assumption] Executed-artifact correspondence.** The direct
   deployed-artifact axiom binds the manifest and referenced blobs selected by
   immutable registry digest to the exact checked Nix OCI archive, binds that
   archive to this reviewed source revision, and requires the collector process
   to execute its entrypoint and runtime code without code-affecting
   substitution.
7. **[assumption] Actual runtime controls.** The other local deployment axioms
   supply single-active common-volume use, runtime/provisioning/backup
   live-source custody, trusted backup-copy custody, private network isolation,
   and external metrics ownership.
8. **[claim] Ordinary restart.** Imports 3 and 4 respectively cover archive
   recovery and joining durability work; import 5 covers environment/key startup
   gates. Together they preserve the four operational properties across ordinary
   restart.
9. **[assumption] Coupled restore.** The direct backup assumption supplies exact
   recovery-point coupling and pre-traffic startup/readiness. The conclusion
   deliberately permits loss of telemetry accepted after that recovery point.
10. **[enum] Joint sufficiency.** Admission, output, durable progress, bounded
   service, repository-local release evidence, immutable
   artifact/source/deployment correspondence, actual runtime controls, restart,
   and recovery-point restore assign every root clause. An independent checker
   must attack this partition for omitted production dimensions.

## Evidence boundary

`real_daemon_registers_pulls_persists_and_restarts` covers one healthy target,
initial registration, one metrics and journal pull, clean SIGTERM, a manually
appended orphan tail, and ordinary restart. It does not integrate crash windows,
stale CAS, archive poison, hostile/unreachable multi-target isolation, fairness,
stale transitions, bounds rejection, generation replacement, source incarnation
or gap handling, expiry/quarantine, cross-worker fatal cleanup, real backup, or
external infrastructure. Those mechanisms have focused unit/component evidence
where named in the leaves; package and SelfCI success establish neither every
conclusion nor a real production deployment.

## Residuals

The property excludes multi-active collectors, remote or split database/archive
storage, best-effort source-journal loss before collection, arbitrary
whole-volume rollback, telemetry accepted after a backup recovery point,
inventory or upstream method-family changes without a new reviewed pin,
dependencies and load outside declared bounds, and Prometheus/Grafana behavior
outside the direct deployment axioms. These boundaries narrow the quantified
property; they do not classify an in-scope failure as harmless.

## Weakest links

The deployed-artifact correspondence, actual deployment, and backup behavior
are axioms. Completeness of the production-dimension partition remains an
enumeration obligation. The repaired restart/shutdown step composes
supervisor-level worker joins with separate tests of each real durability
segment rather than one integrated crash matrix.
