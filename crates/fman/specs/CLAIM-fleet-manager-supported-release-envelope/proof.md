# Proof: Fleet Manager supported release envelope

## Scope and model

This is a compositional conditional argument for
[CLAIM-fleet-manager-supported-release-envelope](../CLAIM-fleet-manager-supported-release-envelope.md).
It covers the release declaration and formed-seat version transitions named in
that claim. It does not inspect the declared daemon, operations, transitions,
workload, dependencies, deadlines, or recovery behavior.

The model quantifies over one release, every supported FI or Admin operation
and transition it identifies, and every formed seat. It grants each immediate
assumption exactly as an axiom. The conclusion concerns the presence and scope
of the release envelope; it does not establish that the declared bounds are
met, that dependencies are available, or that a supported operation is
semantically correct.

## Assumption boundary

The immediate assumptions respectively supply the pinned guardian daemon; the
workload, dependency-availability, deadline, and recovery-objective
declarations; the exhaustive catalog of supported FI or Admin operations and
transitions; and the formed-seat version-transition restriction. No assumption
uses the conclusion or another assumption as its justification.

## Argument

1. **[assumption] Pinned guardian daemon.** The first assumption supplies that
   the release pins `fedimintd`.
2. **[assumption] Declared operating bounds.** The second assumption supplies
   the workload limits, dependency-availability preconditions, operation
   deadlines, and recovery objective stated by the conclusion.
3. **[assumption] Complete supported catalog.** The third assumption supplies
   every supported FI or Admin operation and transition identified by the
   release.
4. **[assumption] Formed-seat upgrade restriction.** The fourth assumption
   supplies that a formed seat does not upgrade to a different version unless
   that transition is explicitly supported.
5. **[logic] Joint sufficiency.** Steps 1 through 4 establish each conjunct of
   the claim verbatim. Therefore the release has the stated supported envelope,
   including its formed-seat upgrade restriction.

## Residuals

This claim does not assert that a particular `fedimintd` version, workload
limit, dependency-availability precondition, deadline, recovery objective, or
supported transition is sufficient in practice. It also does not assert
semantic correctness or timely completion of any cataloged operation. Those
properties are outside this claim's declaration and transition restriction.

## Weakest links

Each direct assumption is an unchecked release or operational premise. The
complete supported catalog and formed-seat upgrade restriction are the most
important boundaries: omitting either would permit an operation or version
transition outside the stated envelope.
