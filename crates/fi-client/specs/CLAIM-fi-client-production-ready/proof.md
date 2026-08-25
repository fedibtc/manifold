# Proof of CLAIM-fi-client-production-ready

This is a compositional proof. It establishes only the local implication from
the claim's immediate assumptions to its property; it does not establish any
assumption or inspect an implementation to do so.

## Scope

The property covers the documented feature envelope: pinned-locator formation,
optional authenticated setup-payment funding, DKG, seat-binding publication and
readback, invite recovery, and read-only verified FMan discovery. It excludes
trust-based selection, registry-backed creation, post-formation maintenance,
liquidity attachment, and guardian-fee arrangement. The claim is broader than
this proof, so its status remains `Unverified`.

The quantified executions use a supported formation size and workload. They
include successful formation, every failure before formation, interruption and
restart at every durable or external-effect boundary, stale quotes, concurrent
drivers, retry, and recovery after availability returns. The conclusion's
deadline and recovery-objective predicates use the values and
dependency-availability preconditions stated by the release.

## Assumptions

The following are axioms, copied from
[the claim](../CLAIM-fi-client-production-ready.md):

1. The consumer supplies one stable FI signing identity, protects and
   namespaces the database, preserves it across supported restarts, and backs
   it up with the identity and wallet recovery material needed to resume.
2. The payment adapter makes each funding operation recoverable before
   committing value, recovers before creating a replacement, replays exact
   quote-bound payments and refund context, reports terminal rejection
   accurately, and settles signed refunds idempotently.
3. The consensus-read adapter performs a genuine threshold Fedimint consensus
   read and does not substitute cached or fabricated configuration or metadata.
4. The transport and FMan implementations honor the authenticated RPC, signed
   quote, commitment, DKG, status, and invite contracts; the pinned
   cryptographic and persistence dependencies satisfy their documented
   contracts or fail detectably.
5. The deployment profile pins authentic setup-payment publishers, issuer
   identity roots, relays, and a schema-valid minimum PeerBadge trust level,
   and the required trust services and clocks satisfy the documented freshness
   and availability bounds.
6. The release states supported formation sizes and workload limits,
   dependency-availability preconditions, formation deadlines, and a recovery
   objective.
7. Within that envelope and those limits, `fi-client`'s durable formation state
   machine resolves concurrent drivers, restarts, and retries for the same
   consumer formation intent to one stable logical formation attempt, with no
   second active attempt for that intent; that attempt has exactly one terminal
   outcome. It gives each logical external value effect one stable identity
   that it authorizes, durably keys, and executes at most once; it accepts
   lifecycle artifacts only after phase-valid authentication and validation,
   never regresses admitted payment policy, and reaches `Formed` only from
   durable matching agreement by every selected seat on the federation and
   seat-binding directory.
8. For every nonterminal logical formation attempt, `fi-client` enables its
   unique next success, failure, or timeout transition, and each enabled
   transition completes within its allocated budget. When the selected eligible
   FMan set, consumer adapters, and required trust services return successful
   valid responses within the configured timeouts and guardians complete DKG,
   valid timely inputs take the specified success transition; regardless of
   availability, timeout, invalid, and terminal inputs take a typed actionable
   terminal state that cannot publish `Formed`. The cumulative transition
   budgets from start or supported resume to terminal outcome fit the configured
   formation deadline or recovery objective.
9. The consumer uses only the documented feature envelope: pinned-locator
   formation, optional authenticated setup-payment funding, DKG, seat-binding
   publication and readback, invite recovery, and read-only verified FMan
   discovery; it excludes trust-based selection, registry-backed creation,
   post-formation maintenance, liquidity attachment, and guardian-fee
   arrangement.

## Argument

The assumptions partition the material dimensions of the property rather than
prove their practical truth.

| Material dimension | Assumptions that bound it | Local consequence |
| --- | --- | --- |
| Supported execution universe and time bounds | 6, 8, 9 | The argument ranges only over declared sizes, loads, dependencies, deadlines, recovery objective, and features; every nonterminal attempt has a unique next transition that completes within its allocation, and cumulative start/resume-to-terminal bounds fit the stated deadline or objective. |
| Stable authority and resumable state | 1 | A supported restart retains the one FI authority and the namespaced durable state, identity, and wallet-recovery inputs required to continue. |
| Value commitment and refund effects | 2, 4, 7 | A value effect is recoverable before replacement, bound to its signed quote and exact replay context, and given one durable authorized key and at-most-once execution; signed refunds converge idempotently, and terminal rejection becomes an accurate typed input instead of a new commitment. |
| Authentic lifecycle inputs | 3, 4, 5, 7 | Threshold configuration reads, authenticated RPCs, signed lifecycle artifacts, pinned trust roots and minimum-level policy, fresh clocks, authentic relays, and phase-valid client acceptance exclude fabricated consensus configuration, unauthenticated lifecycle facts, and substituted trust material. This proof does not establish the claim's selection behavior. |
| Formation agreement, policy, and publication | 4, 7, 8 | The authenticated commitment, DKG, status, and invite contracts supply the agreement artifacts; the state machine preserves admitted payment policy and can publish `Formed` only after durable matching agreement by every selected seat. |
| Interruption, retry, concurrency, and recovery | 1, 2, 6, 7, 8 | Stable durable consumer inputs, recover-before-replace payment behavior, one attempt per consumer formation intent, stable logical effect identities, and total bounded transition paths cover each listed disruption class within the supported universe. |

For a successful-valid response sequence within the configured timeouts and
with guardian DKG completion, the authenticated formation inputs in rows three
through five supply the necessary authority, agreed federation, and
seat-binding directory. Assumption 8 supplies a non-deadlocking unique next
transition and takes valid timely inputs through the specified success path;
each transition completes within its allocation, and its cumulative bound meets
the formation deadline. Assumption 7 gives the consumer formation intent one
attempt with one terminal outcome and permits `Formed` only after that durable
matching agreement. The stable consumer state in row two makes the result
recoverable. For any other sequence, including an unavailable dependency that
reaches its timeout, assumption 8 maps timeout, invalid, and terminal inputs
to a typed actionable terminal state that cannot publish `Formed`.

The same partition covers the safety clauses. Durable pre-effect
authorization/keying, stable logical effect identities, at-most-once execution,
quote-bound recover-before-replace payments, one attempt per consumer formation
intent, and idempotent refunds prevent duplicate or unauthorized value
commitment.
Threshold reads, signatures, authenticated RPC, pinned trust inputs, and
phase-valid acceptance prevent unauthenticated lifecycle facts. The state
machine does not regress admitted policy, and its `Formed` guard requires the
agreement specified in the conclusion. Persistent namespaced state and recovery
inputs combine with total transition paths whose cumulative bounds fit the
recovery objective, so supported interruptions resume to formation or the typed
actionable terminal outcome within that objective when availability
preconditions return.

## Residuals

The claim does not cover unsupported formation sizes or workloads, unavailable
dependencies after the stated availability preconditions are required again,
features outside the enumerated envelope, consumer loss of the required
identity/database/wallet-recovery material, or failures of the assumed adapters,
FMan, transport, trust services, clocks, cryptography, or persistence
dependencies.

## Weakest links

Assumptions 7 and 8 are the broadest local premises. They deliberately state
the state-machine's single-intent identity, terminality, safety, totality, and
progress mechanisms rather than infer them from external contracts or release
documentation. Future work can refine each into focused component claims
without changing this claim's conditional property.
