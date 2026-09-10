# Proof for CLAIM-liquidity-manager-production-ready

## Scope

This is a compositional proof of
[CLAIM-liquidity-manager-production-ready](../CLAIM-liquidity-manager-production-ready.md).
It covers only the documented single-process, single-gateway release envelope
and establishes only the local implication from the immediate assumptions. It
does not establish those assumptions or recursively import their verification
status.

The completeness challenge covers admission and trust, managed-funds custody,
settlement attribution, retry/crash/restore behavior, allocation progress and
failure visibility, confidentiality, canonical interoperability, resource
bounds, and public-RPC fairness. A failure in any one is material to unattended
production use.

## Model and quantifiers

A supported execution satisfies the deployment, operator, dependency, FI, and
release-envelope premises and contains any finite workload admitted by that
envelope. `Ready` means every conclusion in the claim statement holds for every
such execution, including the explicitly described resource and pinned-Iroh
residuals. Workloads, topologies, dependencies, and operator actions outside the
envelope are not quantified.

The managed-funds dimension covers FLIP-triggered custody and recovery authority
without treating stability investment performance as a daemon custody property.
The completed-allocation attribution claim separately supplies the exact
source-specific settlement binding named by the production-ready property.

## Assumptions

The immediate premises are exactly those in
[the claim record](../CLAIM-liquidity-manager-production-ready.md). Linked claims
are axioms at this level. The first five direct premises define the supported
deployment and release. The remaining premises cover authenticated admission,
revocation, constrained outbound transport, allocation privacy, managed funds,
stability progress and fairness, public-resource limits, canonical wire and
signing behavior, failure visibility, secret confinement, success capability,
deadlines, durable allowance consumption, target-client bounds, RPC fairness,
restore, and semantic authorization.

## Argument

1. **`assumption + claim` — supported and authorized envelope.** The deployment,
   operator, dependency, FI, and release premises define one accountable
   supported installation. The federation-capability, revocation, request
   authorization, and allocation-privacy premises restrict admitted work to the
   authenticated actor, exact federation, unrevoked authority, and semantic
   intent. The endpoint premise includes only the documented constrained pinned-
   Iroh pre-authentication traffic, so the residual described by the claim is not
   accidentally asserted away.
2. **`claim + assumption` — managed funds, attribution, and idempotency.**
   [CLAIM-liquidity-manager-managed-funds-not-lost](../CLAIM-liquidity-manager-managed-funds-not-lost.md)
   supplies operational custody, exact effect/target authority, and money-effect
   retry safety without asserting stability investment performance.
   [CLAIM-allocation-completion-has-item-specific-settlement](../CLAIM-allocation-completion-has-item-specific-settlement.md)
   supplies the source-specific binding between reported completion and the
   exact contribution delivered to the intended target. It deliberately does
   not require that contribution to debit FLIP's provider wallet. The
   durable-transition premise extends idempotency to allocation-state,
   target-client, and recovery transitions.
3. **`claim + assumption` — supported allocation progress.** The two terminal-
   observation premises and the one-target fairness premise prevent one
   stability operation or target from hiding terminal progress or blocking the
   rest. The success-capability and deadline premises cover every conforming
   gateway, wallet, target, and recovery operation under the stated dependency
   and scheduling preconditions. Failure visibility makes each unsuccessful or
   recoverable outcome actionable rather than silent.
4. **`claim + assumption` — bounded unattended service.** The durable-request,
   verification-run, target-client, and RPC premises jointly bound the resources
   which the claim says are bounded and provide the stated stream progress. The
   claim separately and truthfully names unbounded qualifying federations,
   retained databases, renewing verification work, and the pending-open budget;
   they are not hidden by this lemma.
5. **`assumption` — confidentiality and interoperability.** Secret confinement
   protects the named local and protocol secrets. The canonical adapter premise
   fixes accepted wire forms and signing domains, preventing a locally correct
   service from being unusable or unauthenticated at its supported boundary.
6. **`assumption` — restart and restore.** The restore premise carries protected
   state, credentials, capacity, and dependencies through the documented normal
   restart or restore path within the recovery objective. Combined with progress
   and failure visibility, it restores unattended operation rather than merely
   starting a process.

These lemmas cover every material dimension named by `Ready`; the immediate
premises therefore imply the claim's conclusion.

## Residuals

- This local proof does not establish any linked claim or direct premise.
- The resource and constrained-Iroh limitations stated in the claim remain
  inside the supported conclusion exactly as written.
- The allocation deadlines and recovery objective are release commitments, not
  measured deployment evidence.

## Weakest links

1. Managed-funds preservation depends on broad direct authority and
   automatic-transition premises which are not yet focused linked claims.
2. Deadline, success-capability, finite-resource, and RPC-fairness premises are
   direct assumptions rather than fully decomposed linked claims.
3. The exact supported workload and dependency availability are release-envelope
   declarations; operational measurement would strengthen them.
