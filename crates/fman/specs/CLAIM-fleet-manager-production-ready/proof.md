# Proof: Fleet Manager production readiness

## Status

Unverified: the expanded umbrella composition and its guardian-metrics premise
have not been verified as one current production-readiness argument.

## Scope and model

This is a compositional conditional argument for
[CLAIM-fleet-manager-production-ready](../CLAIM-fleet-manager-production-ready.md).
It applies to the single-instance, single-tenant production envelope in that
claim: a Fleet Manager hosts guardian seats unattended; an operator may recover
after losing its host; and supported FI and Admin operations run in the release
envelope.

The conclusion has three material dimensions:

1. **confinement** — adversary-reachable interactions cannot exceed their
   authorized effects, including every protected asset named in the claim;
2. **continuity** — paid guardian hosting is unattended, and a lost host can
   recover the Fleet Manager and every published guardian; and
3. **bounded operation** — supported operations and restart/recovery meet the
   release's stated deadlines and recovery objective, when its availability and
   workload preconditions hold.

The model permits the failures that the conclusion names: remote adversaries,
host loss, process restart, dependency unavailability, relay delay, and
workload. It grants every immediate assumption exactly as an axiom. In
particular, this proof does not establish deployment controls, relay retention,
dependency contracts, the interaction-security property, or the Nostr backup
and restore specification.

There is no implementation-grounded step. The claim's broad production-ready
conclusion is defined by the immediately stated deployment, release,
availability, interaction-security, and recovery premises. Inspecting an
individual code path could not establish that this premise set is complete, and
those paths are abstracted by the premises for this local implication.

## Assumption boundary

The imported
[CLAIM-fleet-manager-production-deployment-envelope](../CLAIM-fleet-manager-production-deployment-envelope.md)
premise supplies this conditional property: **given** the secure external
host/operator envelope, FMan runs as one instance for one tenant; the operator
has the admin socket, data root, credentials, and backups; capacity, public-RPC
abuse controls, and health monitoring exist; and the reviewed FMan artifact plus
its bundled, pinned `fedimintd` retain implementation integrity as one TCB. The
production-ready claim's own “within its documented single-instance production
envelope” quantifier supplies that antecedent; the linked checklist remains an
operator obligation, not a live-deployment certification. The imported
[CLAIM-fleet-manager-supported-release-envelope](../CLAIM-fleet-manager-supported-release-envelope.md)
premise supplies this exact property: the release pins `fedimintd`, states
workload limits, dependency-availability preconditions, operation deadlines,
and a recovery objective, and identifies every supported FI or Admin operation
and transition. A formed seat is not upgraded to a different version unless
that transition is explicitly supported. The imported
[CLAIM-fleet-manager-relay-publication-durable](../CLAIM-fleet-manager-relay-publication-durable.md)
premise supplies this exact property: configured Nostr relays eventually accept
and retain every required advertisement, setup-payment, backup-document, and
guardian-archive event, and the operator does not treat best-effort publication
as durable until this has been observed. The imported
[CLAIM-fleet-manager-recovery-dependencies](../CLAIM-fleet-manager-recovery-dependencies.md)
premise supplies this exact property: Fedimint peers retain enough correct
consensus state for guardian recovery, and the pinned Fedimint client, mint
cryptography, iroh, SQLite, RocksDB, filesystem, operating-system process
isolation, and Nostr primitives satisfy their documented contracts or expose
contract-visible failure to FMan, its caller, or its operator. This external
premise does not assert recovery completion, persistence, or a deadline after
failure. The direct quantitative
conformance premise supplies the stated deadline and recovery-objective bounds
for every identified operation and transition when their preconditions hold. The
direct autonomy premise supplies automatic initiation and completion of every
action needed to keep a paid guardian serving, including remediation of each
monitored or detectable in-envelope failure. The direct semantic-correctness
premise supplies every safety and liveness postcondition of the supported
behavior for every identified FI/Admin operation and restart/recovery
transition, and every hosted paid guardian's seat-local lifecycle and Fedimint
protocol participation when their preconditions hold.

The imported [CLAIM-fleet-manager-interaction-security](../CLAIM-fleet-manager-interaction-security.md)
premise supplies this exact property: against its stated remote adversaries, an
interaction remains within the authority and effects granted by the applicable
surface and cannot cause any of the listed secret, authority, payment, policy,
restore, guardian-fee, or post-invite-deletion failures. Its interaction-security
scope is authority, confidentiality, integrity, and value safety; it expressly
does not assert availability, resource or capacity bounds, latency or
operating-cost bounds, or resistance to traffic analysis. The direct
[CLAIM-fleet-manager-guardian-metrics-egress-confined](../CLAIM-fleet-manager-guardian-metrics-egress-confined.md)
premise supplies default-deny confinement of guardian metric bytes before the
authorized collector receives an Iroh response. The direct
[SPEC-nostr-backup-restore](../SPEC-nostr-backup-restore.md) premise supplies
complete ordered publication before a discoverable seat document, plus restore
only from authenticated, internally consistent documents bound to the recovered
identity.

Each premise is conditional. This proof neither checks whether a deployment
monitors health nor whether a relay or dependency actually meets its stated
contract. A failure of one would prevent the operational result in practice; it
does not contradict this implication while the premise is granted.

## Argument

1. **[claim] Confinement.** The interaction-security premise directly covers
   adversary-reachable RPC, Nostr, and guardian-network interaction. Its
   prohibited effects cover secret disclosure; seat and guardian authority;
   exact paid-seat allocation and settlement; unrelated operator value; payment
   policy, trust, and restore state; guardian-fee authority; and deletion after
   invite exposure. These are exactly the protected effects enumerated by the
    production-ready conclusion. Within the claim's documented external envelope,
    the conditional deployment premise supplies the local single-instance/tenant
    boundary, operator-only custody, protected storage/backups, public-RPC abuse
    controls, and trusted FMan/bundled-child artifact boundary. Thus the
    remote-interaction property and external operating boundary jointly cover
    every adversary-reachable interaction in the stated envelope, while the
    guardian-metrics premise confines the contents disclosed through the
    authorized telemetry egress.

2. **[claim] Correct unattended paid hosting.** The confinement result lets
   paid seats be allocated and operated only under their authorized payment,
   seat, and guardian authority. The semantic-correctness premise directly
   supplies every safety and liveness postcondition of the supported behavior
   for every hosted paid guardian's seat-local lifecycle and Fedimint protocol
   participation, so a live but semantically incorrect guardian does not
   satisfy this step. The autonomy premise directly supplies automatic
   initiation and completion, without operator action, of every action needed
   to keep a paid guardian serving, including remediation of monitored or
   detectable in-envelope failures. The deployment-envelope and
   recovery-dependencies premises identify the monitored health signals and
   detectable component failures to which that premise applies. Therefore paid
   guardian seats can be hosted correctly and unattended within the stated
   envelope; this does not promise operation outside it.

3. **[claim] Recovery inputs.** Given the root mnemonic and retained
    authentic documents named by the conclusion, the conditional
    deployment-envelope premise preserves their operator-only custody within its
    external antecedent. The imported relay-publication-durable
   premise makes every required backup event eventually retained and requires
   the operator to observe publication before treating it as durable. The
   backup-restore premise makes the published documents complete and ordered,
   and admits only authentic, internally consistent documents bound to the
   recovered identity. Thus those inputs determine a safe, complete recovered
   fleet rather than a substituted or partial one.

4. **[claim] Guardian recovery.** For every published guardian recovered in the
   preceding step, the imported recovery-dependencies premise grants enough
   correct Fedimint peer consensus state and the documented-or-detectable
   behavior of the pinned client, cryptography, storage, filesystem, process
   isolation, and Nostr primitives. Together with the semantic-correctness
   premise's recovery postconditions, this supplies the final input named by the
   conclusion: each recovered guardian can regain service from federation peers.
   This is per-published-guardian recovery, not a promise to recover unpublished
   state or a federation that lacks enough correct peer state.

5. **[claim] Bounded service and restoration.** The imported
   supported-release-envelope premise identifies every supported FI/Admin
   operation and transition, pins
   `fedimintd`, and states workload limits, dependency-availability
   preconditions, operation deadlines, and a recovery objective. The
   semantic-correctness premise gives every supported FI/Admin operation and
   restart/recovery transition every safety and liveness postcondition of its
   supported behavior, while the quantitative conformance premise gives every
   release-identified supported FI/Admin operation and restart/recovery
   transition its stated deadline or recovery-objective bound when those
   preconditions hold. A formed seat changes version only through a
   release-identified supported transition, so unsupported upgrade behavior is
   not smuggled into this dimension.

6. **[logic] Joint sufficiency.** Step 1 covers all listed protected effects;
   steps 2–4 cover correct unattended hosting and every component of the stated
   host-loss recovery input/output chain; and step 5 covers the stated semantic,
   quantitative operating, and restoration bounds. The deliberate completeness
   challenge was to look for a material production dimension not in one of these
   three groups: local custody/security, correct
   availability/capacity/observability, external publication/peer recovery, and
   release-bounded operations are each explicitly assigned above. No additional
   material dimension remains within the claim's stated envelope. With every
   immediate assumption granted, all three dimensions hold, proving the local
   implication.

## Residuals

The claim excludes multi-instance and multi-tenant operation; operator loss of
the mnemonic or retained authentic backup documents; data that was not yet
observed durable by the operator; unsupported `fedimintd` transitions; a
federation without enough correct peers; dependency unavailability outside the
release's stated preconditions; workloads beyond stated limits; and any
deadline or recovery objective the release did not document. These are outside
the model because the immediate assumptions or property explicitly limit them,
not because they are harmless.

## Weakest links

The broadest premises are semantic correctness, quantitative conformance, and
autonomy: they carry respectively the actual service behavior, deadline/recovery
behavior, and absence of required operator action. The operator observation
condition for backup durability and the deployment monitoring condition are
likewise operational rather than mechanically enforced by this proof. Future
releases can strengthen this argument by publishing a machine-checkable
supported-transition/deadline manifest and by attaching monitored publication
and recovery probes to those declared objectives.

## Open operational counterexamples and obligations

The production envelope remains unverified against these current operational gaps:

- unbounded child output and crash warnings can exhaust a shared unbounded log sink; per-seat budgets and bounded host retention are not established;
- a directory database read failure can terminate the one-shot directory worker and the sibling setup-payment and authorization loops; durable-write health and supervised retry remain required;
- backward wall-clock movement can delay recovery of lost payment tokens until persisted deadlines are re-evaluated;
- descriptor exhaustion can drive a tight admin accept-error loop; paced backoff and a descriptor budget remain absent;
- half-open admin connections lack complete read/write deadlines, bounded input, and pre-fleet serialization;
- a committed mutation may lose its response, so the admin surface does not provide a general exactly-once or result-known guarantee;
- decommission success must not precede observed child death, and spawned-but-unwrapped children need explicit kill/reap ownership;
- a removed payment federation with retained value must remain reopenable, queryable, and drainable;
- the official deployment still relies on stable inputs, durable storage, immutable profile/data-root binding, and fail-closed production publisher and fee-account configuration. Selecting the development profile can mislabel a real deployment, and staging's known publisher secret is not suitable for real trust decisions.
