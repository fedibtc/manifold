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

## FA-client integration boundary

The general consumer premise supplies the ports and storage that `fa-client`
leaves to its consumer, while the operator premise requires protection of
component material. Neither premise by itself defines every recipient or
interface through which FA-client-owned state and credentials can cross that
boundary. The focused FA-consumer integration premise therefore supplies the
necessary local condition: throughout every supported release and workflow,
that integration durably retains or restores its FA-client-owned state and
credentials and releases them only to authenticated, authorized recipients over
protected interfaces.

This is a premise about the consumer integration, not an assertion about the
other components or the root conclusion as a whole. Combined with the FA-client
claim's formation-integrity and recovery properties, it covers the FA-owned part
of the root's state and credential boundary.

## FA-client value-custody boundary

The FA-client claim requires payment-adapter recovery behavior, but its
conclusion only excludes duplicate or unauthorized value commitment. The focused
FA-consumer value-custody premise supplies the additional boundary condition:
the consumer wallet and payment integration retains custody of every
FA-client-directed value amount until it either becomes the exact authorized
quote-bound commitment of a successfully formed federation or returns through a
signed refund. Its durable payment and refund-recovery material,
recovery-before-replacement ordering, exact replay, accurate terminal rejection,
and idempotent signed-refund settlement make that custody condition survive
supported interruption and retry.

This premise concerns only funds directed through FA-client. The other linked
component claims remain responsible for funds within their documented
component-owned boundaries.

## FLIP operational-custody boundary

The FLIP premise supplies operational custody of technically authorized managed
funds. It does not supply stability investment performance, dependency solvency,
or legal or economic ownership. The root's funds-protection conclusion retains
that boundary; this import does not establish the FLIP claim or its premises.

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
| Component workflow behavior, component-owned state, funds, and credentials | The Manifold Guardian, Liquidity Manager, and Push Gateway claims state their component protections. The FA-client claim supplies formation integrity, value-commitment protection, and recovery; the focused FA-consumer integration premise supplies retention, backup restoration, and protected authorized disclosure of FA-client-owned state and credentials; and the focused FA-consumer value-custody premise preserves FA-client-directed funds until their commitment belongs to a successfully formed federation or they return through signed refund. |
| Bounded best-effort notification submission and failure visibility | For every target snapshotted by a durably accepted hook invocation, the Push Gateway claim supplies provider acceptance, permanent invalid-token handling, or actionable dead letter; the dependency and operator assumptions bound its provider, network, and monitoring conditions. |
| Compatible composition of supported workflows | The release-combination assumption fixes supported FA, MG, FLIP, Push Gateway, protocol, schema, trust-profile, dependency, packaging, and upgrade combinations, along with their compatibility for designated workflows. |
| FA consumer ownership and recovery material | The production-consumer assumption supplies the identity, encrypted namespaced storage, backups, payment and wallet ports, scheduling, and interaction that `fa-client` leaves to its consumer. The focused FA-consumer integration premise constrains that supplied integration's retention, recovery, and disclosure of FA-client-owned state and credentials. |
| FA consumer payment and wallet value custody | The focused FA-consumer value-custody premise preserves every FA-client-directed value amount until it belongs to a successfully formed federation or returns through signed refund, including its durable recovery material and retry behavior. |
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

The focused FA-consumer integration premise excludes a disclosure of
FA-client-owned state or credentials to an unauthorized recipient or through an
unprotected interface: either contradicts that premise, while a loss that
cannot be restored contradicts its retention or restoration condition. An
irrecoverable loss of an FA-client-directed value amount before it becomes a
commitment of a successfully formed federation or returns through signed refund
contradicts the focused FA-consumer value-custody premise. The component properties,
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
