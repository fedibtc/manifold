# Proof for CLAIM-liquidity-manager-production-ready

## Scope

This is a compositional proof of
[CLAIM-liquidity-manager-production-ready](../CLAIM-liquidity-manager-production-ready.md).
It covers FLIP only within that claim's documented single-process,
single-gateway production envelope. It establishes the claim's local
implication: if every immediate assumption below holds, the stated production
readiness property holds. It neither establishes the assumptions in a real
deployment nor evaluates linked claims or claims.

The proof considers admission, value and budget accounting, settlement
attribution, irreversible-effect idempotency, failure and recovery handling,
confidentiality, service-adapter interoperability, resource bounds, stream
fairness, allocation completion, and restore. These dimensions are material
because failure of any one contradicts the claim's definition of readiness.

## Model and quantifiers

Let a supported deployment be one satisfying the claim's first five
assumptions, and let an execution contain any finite release-conforming
workload of supported gateway and stability-pool allocation requests. A request,
allocation, item, withdrawal, wallet operation, RPC stream, target client, and
restore is in scope only when the documented interface and release envelope
admit it.

`Ready` means every conclusion named in the claim statement holds for every
such execution: authorized admission and operator-free successful completion
capability; provider-wallet value protection; item-specific settlement
attribution; crash and retry idempotency; recoverability; actionable failure
visibility; secret confinement; canonical wire and signing behavior; the stated
finite resource and fairness bounds; deadline-bounded completion or actionable
terminal state; and recovery within the stated objective. Inputs, dependencies,
workloads, deployment topologies, and operator actions outside the stated
envelope are not quantified.

The supported envelope includes the accepted pinned-Iroh behavior. An endorsed
requester that owns an Iroh node id may publish direct or relay addressing which
the transport contacts before node authentication. Direct traffic consists of
an encrypted discovery Ping and QUIC handshake. Relay traffic starts with TCP
plus a TLS ClientHello or fixed empty `GET /relay`; the published relay name
appears in SNI and `Host`, and successful setup may continue with
protocol-generated client and relay frames. The requester cannot select an
arbitrary method, path, body, header set, or subsequent frame contents, and FLIP
does not disclose the destination response to it.

## Immediate assumptions

The following are the immediate premises, copied from the claim record. The
linked claims are axioms at this level; this proof does not open or
assess them.

1. Exactly one FLIP process and runtime generation owns one data directory and
   configured gateway. The Admin API binds to a local or private interface or is
   protected by operator-controlled access enforcement.
2. The operator protects the data directory, local encryption key, Admin token,
   gateway credential, provider identity, and unencrypted backups, provisions
   durable host capacity and fee reserves, and follows the documented backup,
   restore, monitoring, and manual-recovery procedures.
3. The configured gateway, chain observer, target Fedimint federations, Nostr
   relays, issuers, SQLite, filesystem, clock, and cryptographic primitives
   return authentic results with the documented durability or fail detectably.
4. A supported FI supplies the required signed endorsement and trust material.
   FLIP's public RPC boundary verifies that material and binds the requester's
   signing key to the authenticated transport actor before admitting the
   request.
5. Production disables `--trust-fixtures` and uses only release-pinned supported
   protocol and dependency versions. The release states workload limits,
   dependency-availability preconditions, allocation deadlines, and a recovery
   objective.
6. Every public allocation is admitted through a valid, unrevoked endorsement for
   its exact federation, and each federation has at most one allocation with at
   most one item per source
   ([claim](../CLAIM-federation-capability-bounded-allocation.md)).
7. FLIP fails closed when it cannot perform a required fresh credential-revocation
   check
   ([claim](../CLAIM-missing-nostr-revocation-fails-open.md)).
8. FI-supplied invite data cannot select an arbitrary method, path, body, header
   set, or subsequent frame contents before target-federation authentication, or
   expose the selected destination's response to the FI. Pinned Iroh's
   protocol-generated discovery/QUIC and relay handshake/framing traffic to
   node-owner-published destinations is included in the production envelope
   ([claim](../CLAIM-endorsed-invite-endpoint-ssrf.md)).
9. A caller without an allocation's requester key and details commitment cannot
   learn whether the allocation exists
   ([claim](../CLAIM-allocation-existence-probe.md)).
10. [CLAIM-fresh-request-id-repeated-funding](../CLAIM-fresh-request-id-repeated-funding.md).
11. An allocation item completes only when its fulfilled amount is attributable
    to FLIP-caused provider-wallet outflow for that item's persisted target
    ([claim](../CLAIM-allocation-completion-has-attributable-provider-outflow.md)).
12. One operator withdrawal intent causes at most one wallet send and one settled
    payment
    ([claim](../CLAIM-duplicate-operator-withdrawal.md)).
13. [CLAIM-duplicate-stability-deposit](../CLAIM-duplicate-stability-deposit.md).
14. FLIP does not admit a new request against capacity from a possibly debited
    wallet send until a durable observation is known to include that debit
    ([claim](../CLAIM-fi-stale-capacity-reuse.md)).
15. At every commit that adds or reactivates a wallet liability, active allocation
    reservations, operator withdrawals, possibly spent uncovered sends, and the
    fee reserve do not exceed known spendable balance, and active allocation
    reservations do not exceed the configured allocation cap
    ([claim](../CLAIM-wallet-budget-overcommit.md)).
16. After an item or wallet operation becomes terminal, stale work cannot perform
    another irreversible effect or overwrite its terminal evidence
    ([claim](../CLAIM-post-cancellation-effect.md)).
17. Before accepting a nonzero stability-pool allocation, FLIP verifies that the
    authenticated target configuration has a usable stability-pool module
    ([claim](../CLAIM-accepted-stability-allocation-requires-target-module.md)).
18. A stability-pool allocation accepted for one configuration cannot cause
    provider-wallet outflow through a different configuration
    ([claim](../CLAIM-stability-worker-config-revision-fence.md)).
19. No automatic path can strand a stability-pool item's claimed ecash in the
    target client. Only a deliberate, authenticated operator action can, and it
    records the abandoned amount and the operator's reason durably
    ([claim](../CLAIM-failed-stability-allocation-strands-ecash.md)).
20. If a claimed stability-pool deposit is rejected, an Admin operation fails the
    item, releases the provider capacity its reservation held, and durably records
    the amount left behind in the target client. Recovering that value is outside
    FLIP
    ([claim](../CLAIM-stability-deposit-rejection-releases-capacity.md)).
21. Fair worker execution eventually observes an upstream terminal
    `deposit_to_provide` result for an active stability-pool item
    ([claim](../CLAIM-stability-deposit-terminal-state-not-observed.md)).
22. Fair worker execution eventually observes an upstream terminal safe peg-in
    result for an active stability-pool item
    ([claim](../CLAIM-stability-peg-in-terminal-state-not-observed.md)).
23. One unresolved stability-pool target cannot prevent processing of later active
    allocations
    ([claim](../CLAIM-stability-worker-single-target-starvation.md)).
24. FI input cannot make FLIP retain an unbounded number of target clients or
    target-client databases
    ([claim](../CLAIM-stability-target-client-retention-is-unbounded.md)).
25. FI input cannot grow durable public-request state without consuming a
    configured finite per-requester or global admission allowance
    ([claim](../CLAIM-fi-request-resource-exhaustion.md)).
26. Within each runtime generation and configured verification window, one
    endorsed federation cannot exceed its outbound trust-verification run
    allowance, regardless of requester-authored fields
    ([claim](../CLAIM-fi-rejected-request-verification-amplification.md)).
27. Incomplete unauthenticated RPC streams cannot indefinitely prevent all new
    streams from reaching frame decoding
    ([claim](../CLAIM-public-rpc-slow-stream-exhaustion.md)).
28. The supported service-adapter release accepts and emits only the documented
    canonical wire forms, and verifies the documented signing domain and bindings
    before treating a request or response as authenticated.
29. Every supported allocation path records a non-secret, actionable failure or
    terminal outcome through the documented operator-visible monitoring and
    recovery interface.
30. FLIP confines the protected data-directory contents, local encryption key,
    Admin token, gateway credential, provider identity, wallet material, and
    allocation secrets to the authorized local storage, operator, and protocol
    recipients defined by the supported deployment.
31. Each supported provider-wallet effect, including gateway funding, has one
    durably reconciled semantic operation: retries cannot duplicate its
    irreversible debit; its persisted authorized target, amount, and fees remain
    within its durable reservation; every authorized but unreconciled operation
    keeps that full reservation active and charged against known spendable balance
    and the configured allocation cap until durable evidence proves no effect or a
    known balance observation includes the debit; no operation can perform an
    effect after releasing that reservation; for each allocation item or operator
    withdrawal intent, aggregate irreversible debits and fees across all semantic
    operations and effects do not exceed its durable authorized reservation, and
    no distinct operation can reuse consumed reservation authority; and its
    evidence permits recovery or reconciliation at every crash point.
32. Within the stated workload limits and dependency-availability preconditions,
    every conforming valid supported gateway and stability-pool allocation can be
    admitted and has an operator-free execution that completes successfully.
33. Within the stated workload limits and dependency-availability preconditions,
    fair scheduling and each required supported gateway, wallet, target, and
    recovery operation produce an observed success, failure, or recoverable
    outcome early enough to record allocation completion or an actionable
    terminal state by the documented allocation deadline.
34. Each retained durable public-request row holds one non-reusable unit of a
    configured finite global allowance for its full retention lifetime and,
    where configured, its finite per-requester allowance.
35. Within one runtime generation and configured verification window, every
    delivery and wire variant for one federation consumes the same authenticated
    finite allowance.
36. Configured finite limits bound concurrently retained target clients and
    target-client databases.
37. Within the stated workload limits and fair-scheduling conditions, every new
    RPC stream reaches admission and frame decoding within its documented finite
    bound despite incomplete unauthenticated streams.
38. Following a supported normal restart from protected durable state or the
    documented restore procedure with the protected durable backup, capacity,
    credentials, and dependencies available restores the supported service within
    the documented recovery objective.
39. Every admitted valid, unrevoked endorsement authorizes the exact requester
    signing key bound to the authenticated transport actor, its federation, and
    its semantic liquidity intent.
40. Every provider-wallet operation, effect, and durable settlement evidence is
    bound to the exact allocation item or operator intent whose durable reservation
    authorized it and can satisfy only that owner. One effect and its evidence
    cannot count its value or fees more than once.

## Argument

1. **Envelope and authorized admission.** Assumptions 1 through 5 define the
   accountable owner, protected administration boundary, supported release, and
   authenticated dependency model. Assumptions 4, 6, 7, and 40 jointly make
   admission authorization specific to the authenticated FI, the exact
   authenticated requester key, a valid unrevoked endorsement, its semantic
   intent, and its exact federation. Assumption 8 prevents the admitted FI from
   selecting an arbitrary method, path, body, header set, or subsequent frame
   contents before authentication, or receiving the selected destination's
   response. It deliberately includes pinned Iroh's protocol-generated
   discovery/QUIC and relay handshake/framing traffic to node-owner-published
   destinations.
   Thus unsupported fixture behavior, unauthenticated callers, revoked
   credentials, and attacker-selected application requests are outside an
   admitted supported allocation; the documented constrained Iroh traffic is
   inside the envelope.

2. **Provider-wallet value and settlement attribution.** Assumptions 11, 14, and
   15 protect the budget before and during allocation: fulfilled value has an
   item-persisted target and attributable FLIP-caused provider-wallet outflow;
   uncertain debits cannot free capacity; and all stated liabilities plus the
   fee reserve remain within known spendable balance and the allocation cap.
   Assumptions 12 and 13 make the two additional irreversible value paths
   single-effect and amount-bounded. Assumptions 17 and 18 ensure a
   stability-pool value path uses an authenticated usable module for the
   accepted configuration, rather than another configuration. Assumption 31
   extends the same one-effect and bounded-authority protection to gateway
   funding and every other supported provider-wallet effect, including a crash
   before an effect is reconciled. Its reservation remains charged until
   reconciliation, preventing cross-operation reuse while an authorized effect
   remains possible, and no later effect can follow a release. Its consumable
   authority bounds the aggregate debits and fees of every operation for the
   item or operator intent. Assumption 40 binds each operation, effect, and
   settlement evidence to that same owner and makes its value non-reusable.
   These premises
   jointly establish the claim's provider-wallet-value and item-specific
   settlement-attribution dimensions.

3. **Idempotency, terminal integrity, and recoverability.** Assumption 10
   prevents duplicate allocations for the same authenticated semantic intent.
   Assumptions 12, 13, and 16 extend that protection across withdrawal,
   deposit, stale-worker, crash, and retry paths: a terminal result neither
   repeats its irreversible effect nor loses its terminal evidence. Assumption
   31 covers the preceding un-reconciled crash point, including gateway
   funding, and retains evidence sufficient to resume, reconcile, or recover
   without creating another operation that can consume the same authority.
   Assumption 16 also closes the allocation hierarchy per item: a terminal item
   has no later irreversible effect and no overwritten terminal evidence, which
   is what "a terminal allocation has no active value-affecting child" amounted
   to once no roll-up allocation status existed to quantify over. Assumptions 19
   and 20 supply automatic or operator recovery for claimed ecash on failed or
   rejected stability work.
   Together with durable, authentic-or-detectable dependencies in assumption 3
   and the operator procedure and capacity conditions in assumption 2, these
   establish the claimed crash/retry idempotency and recoverability within the
   envelope.

4. **Confinement, failure visibility, and interoperability.** Assumption 9
   confines allocation existence to the requester key and details commitment.
   Assumptions 1 and 2 protect administrative and local secret access;
   assumption 30 adds the necessary service-side non-disclosure boundary.
   Assumption 29 makes each supported failure or terminal state actionable
   without exposing those secrets, while assumptions 2 and 3 supply monitored
   handling and detectable dependency failure. Assumption 28 independently
   supplies canonical service-adapter encoding and exact authenticated signing
   treatment. Hence the confidentiality, failure-visibility, and canonical
   wire/signing dimensions all hold without inferring them from a linked
   analysis.

5. **Finite public and outbound resources; fair stream and worker progress.**
   Assumptions 25 and 34 bound durable public-request state: every retained row
   consumes one non-reusable unit throughout its lifetime, and the global
   allowance is finite. Assumptions 26 and 35 bound outbound trust-verification
   rate within one runtime generation and window, with every delivery and wire
   variant charged to the same authenticated federation allowance. Lifetime
   cumulative work is not established by these assumptions; the root record
   retains it as an unaccepted open defect. Assumptions 24 and 36 bound
   concurrently retained target clients and their databases by configured finite
   limits. Assumptions 27 and 37 give both
   collective and per-new-stream admission and frame-decoding fairness despite
   incomplete unauthenticated streams. Assumption 23 prevents one unresolved
   target from excluding later active allocations, while assumptions 21 and 22
   make fair stability workers observe both relevant upstream terminal results. This
   jointly supplies each explicitly named resource, stream, and slow-target
   fairness conclusion.

6. **Completion or actionable terminal state by deadline.** Assumption 32
   supplies an operator-free successful-completion execution for every
   conforming valid supported allocation. Assumption 33 is the
   time-bounded progress premise for all supported allocation paths under the
   release's stated workload and availability conditions. Assumptions 21, 22,
   and 24 give its stability-pool-specific fair-worker cases; assumptions 16,
   19, 20, and 23 ensure observed terminal and recovery outcomes remain
   coherent and actionable. Assumption 29 makes every resulting failure or
   terminal state operator-actionable. Assumption 5 supplies the documented
   deadline that assumption 33 meets. Therefore every accepted supported
   allocation reaches completion or an actionable terminal state within that
   deadline.

7. **Restart and restore objective.** Assumptions 1 through 3 provide a
   single-owner, protected, durable-or-detectable foundation. Assumption 2
   supplies the protected backup, capacity, credentials, and operator procedure.
   Assumption 38 makes both normal restart and documented restore meet the
   objective which assumption 5 states. The terminal-integrity and recovery
   premises keep
   recovered allocation state attributable and actionable. Thus restart or
   restore recovers the supported service within the documented recovery
   objective.

Each material conclusion dimension appears in at least one argument step, and
no step treats a linked analysis as established fact rather than its stated
immediate axiom. The assumptions therefore jointly imply `Ready` in the stated
model.

## Residuals

- Multi-process or multi-gateway operation, public unprotected Admin exposure,
  fixture trust, unsupported protocol versions, and workloads beyond the release
  limits are outside the claim's envelope.
- Dependency unavailability, unauthentic behavior, non-durable storage, absent
  backup material, insufficient capacity or fee reserve, and operators who do
  not follow the documented procedures violate immediate assumptions rather than
  falsifying this conditional implication.
- This proof does not establish the practical truth of the linked claims, release deadlines, availability conditions, or recovery objective.

## Weakest links

- Assumptions 28 through 40 are direct release and operational premises rather
  than independently linked lower claims. They are the least mechanically
  anchored parts of this compositional argument.
- In particular, assumption 32 carries the unattended-success capability,
  assumption 33 carries the global deadline guarantee, and assumption 38 carries
  the recovery-time guarantee. Future focused claims can
  replace those premises only when they preserve their exact semantics.
- The claim premises are granted exactly as written. Their practical
  validity remains a separate verification concern and does not alter this
  claim's local conditional proof.
