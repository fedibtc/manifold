# CLAIM-production-ready: Manifold is ready for production

Within its documented production deployment and support envelope, Manifold can
form and operate supported decentralized Fedimint federations, provide supported
post-formation liquidity, and, for every target snapshotted by a durably accepted
supported hook invocation, resolve bounded best-effort provider submission
without losing or exposing component-owned durable state, funds, or credentials.
While required dependencies satisfy the release's stated availability
preconditions and load remains within its supported limits, every supported
formation and liquidity workflow completes within its documented operation
deadline, and every such target resolves as provider acceptance, permanent
invalid-token handling, or an actionable dead-letter state within its documented
deadline. Supported failure, restart, backup, restore, and upgrade paths recover
service within the release's documented recovery objective or expose an
actionable failure before the affected component serves traffic.

## Assumptions

- [CLAIM-fleet-manager-production-ready](../crates/fman/specs/CLAIM-fleet-manager-production-ready.md)
- [CLAIM-fi-client-production-ready](../crates/fi-client/specs/CLAIM-fi-client-production-ready.md)
- [CLAIM-liquidity-manager-production-ready](../crates/liquidity-manager-daemon/specs/CLAIM-liquidity-manager-production-ready.md)
- [CLAIM-push-gateway-production-ready](../crates/push-gateway/specs/CLAIM-push-gateway-production-ready.md)
- Each production release identifies an exact supported combination of FI, FMan,
  FLIP, Push Gateway, wire protocols, database schemas, trust profiles, Fedimint,
  gatewayd, packaging, and upgrade paths. It also states workload limits,
  dependency-availability preconditions, operation deadlines, and recovery
  objectives, and that combination is wire-, schema-, trust-profile-, and
  packaging-compatible for every workflow the release designates as supported.
- The production Fedi consumer supplies the identity, encrypted and namespaced
  durable storage, backup, wallet and payment ports, scheduling, and user
  interaction required by the `fi-client` contract.
- Within each supported release and workflow, the production Fedi consumer's
  `fi-client` integration durably retains FI-client-owned state and credentials
  or restores them from required backup, and discloses them only to
  authenticated, authorized recipients over protected interfaces.
- Within each supported release and workflow, the production Fedi consumer's
  wallet and payment integration preserves custody of every FI-client-directed
  value amount until it either becomes the exact authorized quote-bound
  commitment of a successfully formed federation or returns through a signed
  refund. It durably retains or restores the payment and refund-recovery
  material, recovers each interrupted operation before creating a replacement,
  replays exact quote-bound payments and refund context, accurately reports
  terminal rejection, and settles signed refunds idempotently.
- Every documented supported backup, restore, and upgrade procedure has an
  enforced completion gate. For each component it affects, the procedure either
  recovers that component's service within the release's stated recovery
  objective or keeps that component out of traffic and exposes an actionable
  failure.
- Deployment-provided Nostr and iroh routing, issuer authority and revocation
  services, Bitcoin backends, Fedimint and gatewayd services, FCM, databases,
  clocks, and network connectivity satisfy the release's availability
  preconditions and their documented authenticity, finality, and durability
  contracts.
- Operators use only documented production profiles and topologies, protect
  component secrets, state, and backups, retain required recovery material,
  monitor every documented health and operator surface, and follow the
  documented backup, restore, and upgrade procedures.
