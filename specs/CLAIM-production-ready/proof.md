# Proof: Compositional production-readiness assessment

## Scope and model

This is a compositional argument for the property and operating envelope in
[CLAIM-production-ready](../CLAIM-production-ready.md). It treats that record's
four linked component properties and seven direct deployment assumptions as
axioms; it neither establishes nor re-verifies them. A supported release is a
release and workflow covered by the claim's stated production deployment and
support envelope.

The conclusion combines several kinds of production readiness: successful
formation and liquidity workflows; bounded best-effort notification submission;
protection of component-owned state, funds, and credentials; deadline-bounded
operation; and recovery or pre-traffic actionable failure on supported failure,
restart, backup, restore, and upgrade paths. The argument must show that the
immediate assumptions cover each kind jointly, not merely that each component
has a production-readiness claim.

## FI-client integration boundary

The general consumer premise supplies the ports and storage that `fi-client`
leaves to its consumer, while the operator premise requires protection of
component material. Neither premise by itself defines every recipient or
interface through which FI-client-owned state and credentials can cross that
boundary. The focused FI-consumer integration premise therefore supplies the
necessary local condition: throughout every supported release and workflow,
that integration durably retains or restores its FI-client-owned state and
credentials and releases them only to authenticated, authorized recipients over
protected interfaces.

This is a premise about the consumer integration, not an assertion about the
other components or the root conclusion as a whole. Combined with the FI-client
claim's formation-integrity and recovery properties, it covers the FI-owned part
of the root's state and credential boundary.

## FI-client value-custody boundary

The FI-client claim requires payment-adapter recovery behavior, but its
conclusion only excludes duplicate or unauthorized value commitment. The focused
FI-consumer value-custody premise supplies the additional boundary condition:
the consumer wallet and payment integration retains custody of every
FI-client-directed value amount until it either becomes the exact authorized
quote-bound commitment of a successfully formed federation or returns through a
signed refund. Its durable payment and refund-recovery material,
recovery-before-replacement ordering, exact replay, accurate terminal rejection,
and idempotent signed-refund settlement make that custody condition survive
supported interruption and retry.

This premise concerns only funds directed through FI-client. The other linked
component claims remain responsible for funds within their documented
component-owned boundaries.

## Notification boundary

Notifications are non-load-bearing: workflow correctness and recovery do not
depend on device or application receipt. The Push Gateway resolves a notification
target snapshotted by a durably accepted hook invocation to provider acceptance,
permanent invalid-token handling, or actionable dead letter within its deadline,
as specified by [SPEC-hook-invocation](../../crates/push-gateway/specs/SPEC-hook-invocation.md).
An accepted zero-target invocation has no target-resolution outcome. Delayed or
dropped device delivery is expected behavior, not a production-readiness
counterexample. [ARCH-decentralized-federations](../ARCH-decentralized-federations.md)
records the separate system-wide correctness boundary and latency role.

## Dimensions considered and assumption mapping

| Production-readiness dimension | Immediate assumptions that cover it |
| --- | --- |
| Component workflow behavior, component-owned state, funds, and credentials | The Fleet Manager, Liquidity Manager, and Push Gateway claims state their component protections. The FI-client claim supplies formation integrity, value-commitment protection, and recovery; the focused FI-consumer integration premise supplies retention, backup restoration, and protected authorized disclosure of FI-client-owned state and credentials; and the focused FI-consumer value-custody premise preserves FI-client-directed funds until their commitment belongs to a successfully formed federation or they return through signed refund. |
| Bounded best-effort notification submission and failure visibility | For every target snapshotted by a durably accepted hook invocation, the Push Gateway claim supplies provider acceptance, permanent invalid-token handling, or actionable dead letter; the dependency and operator assumptions bound its provider, network, and monitoring conditions. |
| Compatible composition of supported workflows | The release-combination assumption fixes supported FI, FMan, FLIP, Push Gateway, protocol, schema, trust-profile, dependency, packaging, and upgrade combinations, along with their compatibility for designated workflows. |
| FI consumer ownership and recovery material | The production-consumer assumption supplies the scoped root, encrypted namespaced storage, backups, payment and wallet ports, scheduling, and interaction that `fi-client` leaves to its consumer; `fi-client` derives every FI key from that root. The focused FI-consumer integration premise constrains that supplied integration's retention, recovery, and disclosure of FI-client-owned state and credentials. |
| FI consumer payment and wallet value custody | The focused FI-consumer value-custody premise preserves every FI-client-directed value amount until it belongs to a successfully formed federation or returns through signed refund, including its durable recovery material and retry behavior. |
| External-service availability, authenticity, finality, and durability | The dependency assumption covers routing, issuer and revocation services, Bitcoin, Fedimint, gatewayd, FCM, databases, clocks, and network connectivity under their documented contracts and release preconditions. |
| Production deployment and operations | The operator assumption restricts profiles and topologies, protects secrets, state, and backups, retains recovery material, monitors documented surfaces, and requires documented backup, restore, and upgrade procedures. The lifecycle-procedure premise gates the outcome for each affected component of every supported backup, restore, and upgrade on bounded service recovery or pre-traffic actionable failure. |
| Capacity, deadlines, and recovery objectives | The linked component claims and release-combination assumption state workload limits, availability preconditions, deadlines, and recovery objectives; the dependency and operator assumptions bound the conditions needed to meet them. The lifecycle-procedure premise makes the release service-recovery objective or pre-traffic failure outcome explicit for each affected component of supported backup, restore, and upgrade procedures. |

## Completeness challenge and result

The table is the completeness method for this broad claim: it deliberately
tests whether the assumptions cover functional outcomes, asset protection,
reliability and recovery, compatibility, external dependencies, and
operability. These dimensions cannot be exhaustively derived from component
source alone. The necessary challenge is to construct a scenario within the
stated envelope where every immediate assumption holds but an important
production-readiness outcome fails.

The focused FI-consumer integration premise excludes a disclosure of
FI-client-owned state or credentials to an unauthorized recipient or through an
unprotected interface: either contradicts that premise, while a loss that
cannot be restored contradicts its retention or restoration condition. An
irrecoverable loss of an FI-client-directed value amount before it becomes a
commitment of a successfully formed federation or returns through signed refund
contradicts the focused FI-consumer value-custody premise. The component properties,
release-combination premise, dependency premise, and operator premise map the
remaining component, workflow, composition, environment, and operations
challenges in the table.

An indefinitely hanging supported backup, restore, or upgrade procedure is
also not a counterexample: it contradicts the lifecycle-procedure premise's
enforced completion gate. A procedure that cannot recover service but keeps its
affected component out of traffic and exposes an actionable failure satisfies
the root's stated alternative.

Provider acceptance without device or application receipt is not a
counterexample: the root requires bounded target resolution to its specified
provider outcomes, not receipt.

## Residuals

Unsupported releases, workflows, deployment profiles, topologies, workloads,
dependency states outside their stated availability preconditions, and recovery
paths outside the documented support envelope are outside this claim's
quantifiers.

## Weakest link

Completeness is the weakest part of any umbrella production-readiness argument:
it requires a premise for each material end-to-end concern, not only compatible
component contracts. The notification boundary must remain explicit so future
work does not accidentally make workflow correctness depend on recipient
receipt, while still challenging the mapping with concrete scenarios rather
than treating the table as a mechanically complete enumeration.
