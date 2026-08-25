# Proof: Cloud FMan telemetry repository deployment evidence

## Scope and model

The scope is
[CLAIM-cloud-fman-telemetry-production-deployment-envelope](../CLAIM-cloud-fman-telemetry-production-deployment-envelope.md),
this proof, and the three immediate linked claim records below. Child proof
scopes and assumptions are not imported.

The model quantifies only over repository artifacts at the reviewed revision.
It partitions their deployment evidence into checked archive
declarations/defaults, assumption-bounded local startup gates, and documented
operator/external premises. It does not quantify over workflow execution,
registry state, deployment selection, or operator compliance.

## Exact imports

The imports are the complete property paragraphs, including every
qualification and assumption, from exactly these three immediate children:

1. [checked OCI archive declarations and defaults](../CLAIM-cloud-fman-telemetry-collector-oci-archive-declarations.md);
2. [assumption-bounded local startup gates](../CLAIM-cloud-fman-telemetry-local-data-root-startup-gates.md); and
3. [documented operator deployment premises](../CLAIM-cloud-fman-telemetry-operator-deployment-premises-documented.md).

No paraphrased subset replaces an imported property.

## Axioms

Exactly the three linked claims' complete property paragraphs are granted as
axioms. Their proofs, scopes, and separate assumptions are not imported. The
composition adds no local axiom, including no publication, registry,
deployment-compliance, or operator-behavior premise.

## Argument

1. **[claim] Checked archive.** Import 1 supplies the exact archive selected by
   the configured release aggregate, the declarations/defaults established by
   its explicit Nix check, and the source-level publication wiring.
2. **[claim] Local startup gates.** Import 2 supplies the successful-startup
   configuration, data-root, process-lock, key, and persisted-identity gates,
   with its local-filesystem and cooperative-process qualifications intact.
3. **[claim] External-premise documentation.** Import 3 supplies the statement
   that the shipped deployment material identifies the remaining operator and
   external-system controls without establishing compliance.
4. **[enum] Three-way closure.** The claim says only the conjunction of those
   three imported conclusions. Archive construction and configured release
   wiring belong to import 1, daemon startup behavior belongs to import 2, and
   statements assigning non-local controls belong to import 3. Actual
   publication, registry contents and digest selection, runtime overrides,
   deployment compliance, network isolation, encryption, Prometheus, and
   backups fall outside the repository-local predicate.

## Evidence boundary

The image defaults include a private bind on `0.0.0.0:8176` but omit the
explicit isolation assertion and required site-specific production values.
They are checked defaults, not a standalone successful deployment. A runtime
can also override image declarations. Neither fact contradicts the imported
archive or startup conclusions.

## Residuals

A deployment may run two valid collectors on different roots, use unencrypted
storage, expose the private listener after asserting isolation, select a
different image, configure Prometheus incorrectly, or take an uncoupled backup.
The repository evidence can remain valid in every execution because the claim
does not quantify over actual deployment behavior.

## Weakest links

The three-way partition is an `enum` obligation. Each substantive mechanism
and its assumptions remain owned by the corresponding imported claim.
