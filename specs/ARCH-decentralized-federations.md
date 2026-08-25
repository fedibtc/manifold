# ARCH-decentralized-federations: Decentralized federations project

This repository builds the components that let end users form and operate
Fedimint federations without any centrally operated guardian
infrastructure. Four protocol roles interact over Nostr (discovery/trust)
and iroh (control plane); none of them requires a public IP, DNS name, or
TLS certificate.

## Roles and components

- **FMan (Fleet Manager)** — runs guardian seats: one daemon hosts multiple
  `fedimintd` child processes, participates in federation setup (DKG) and
  operations on behalf of an operator. Implemented in
  `crates/fman` — `core` is the daemon and the capability traits it defines,
  `fedimint` and `nostr` implement them, `bin` wires them — with its wire/auth
  vocabulary in `crates/service-fleet-manager`. Governed by
  [ARCH-fleet-manager](../crates/fman/specs/ARCH-fleet-manager.md).
- **FI (Federation Initiator)** — the user-side orchestrator that
  discovers FMans, evaluates trust, creates operator seats, and drives the
  formation ceremony without being a guardian itself. The production
  consumer is the Fedi app via `crates/fi-client`; `crates/fi-cli` is its
  development/test-only terminal and end-to-end consumer, carrying the FI
  side of the FMan key-locked payment protocol (its own wallet plus a
  `payer` module) on the shared `crates/locked-payments` protocol crate. The
  library implements consumer-neutral state, registry discovery, a verified advertisement-only
  preview, explicit-payer Pay-and-create, selected all-zero bootstrap without a
  payer, pinned diagnostic formation, exact
  payment recovery, DKG, invite-code recovery, post-DKG seat-binding
  publication, typed post-formation metadata maintenance, and post-formation
  liquidity orchestration
  ([SPEC-fi-post-formation-liquidity](../crates/fi-client/specs/SPEC-fi-post-formation-liquidity.md)).
  Guardian-fee arrangement is implemented as a separate typed post-formation
  operation.
  Governed by
  [ARCH-fi-client](../crates/fi-client/specs/ARCH-fi-client.md).
- **FLIP (Federation Liquidity Provisioner)** — the provider-run daemon
  offering post-formation liquidity: it advertises trust material and
  endpoints over Nostr, verifies federation eligibility, and funds
  gateway/LN and stability-pool allocations from its configured gatewayd.
  Implemented in `crates/liquidity-manager-daemon` with its wire/auth
  vocabulary in `crates/service-liquidity-manager`. Governed by
  [ARCH-liquidity-manager](../crates/liquidity-manager-daemon/specs/ARCH-liquidity-manager.md).
- **Issuer** — signs credentials that FIs use to evaluate FMans and FLIPs.
  Anyone can be an Issuer; **FCS** is the Fedi-operated one. Credential
  and holder-authorization shapes are recorded in
  [SPEC-holder-trust-envelope](../crates/domain/specs/SPEC-holder-trust-envelope.md).

## Trust and discovery fabric

Actor identities are self-generated keypairs. Trust is carried by signed
documents, not by the transport: Issuers blind-sign credentials to
operators (Holders), operators sign `HolderAuthorization`s binding a
component's pubkey to a credential, and components embed those in signed
Nostr advertisements. `crates/manifold-environment` owns the shared deployment
identity, relay routing, issuer-identity data, and minimum PeerBadge trust
level. FI, FLIP, push-gateway guardian telemetry, and the cloud FMan telemetry
collector use the cloneable verifier
in `crates/peer-badge-verifier`; deployments pin only
issuer identity roots, while each verification fetches and admits the current
identity-signed authority and current revocation state without durable caching,
then applies the environment's minimum to the authenticated badge.
FLIP's request-carried FMan trust pipeline applies the same shared domain
policy after its direct envelope verification.
FMan carries its own HolderAuthorization but does not judge its own badge
([ARCH-manifold-environment](../crates/manifold-environment/specs/ARCH-manifold-environment.md),
[SPEC-peer-badge-verifier](../crates/peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)).
The cross-program contracts for the carried shapes are
[SPEC-fman-nostr-events](../crates/nostr/specs/SPEC-fman-nostr-events.md),
[SPEC-holder-trust-envelope](../crates/domain/specs/SPEC-holder-trust-envelope.md),
and
[SPEC-federation-trust-directory](../crates/domain/specs/SPEC-federation-trust-directory.md);
component specs must not contradict them, and byte-level changes are
coordinated with the external consumer programs rather than made
unilaterally here.

## Supporting crates

- `crates/fedi-iroh-rpc` (+ `-macros`) — typed RPC framing over iroh
  connections; every FI ↔ FMan verb travels over this.
- `crates/nostr`, `crates/nostr-clients` — Nostr event shapes and relay
  client helpers shared by advertisers and discoverers
  ([ARCH-nostr-clients](../crates/nostr-clients/specs/ARCH-nostr-clients.md)).
- `crates/setup-payment-publisher` — production key-custodian tool that signs,
  durably receipts, publishes, and verifies the common setup-payment policy
  ([SPEC-setup-payment-federations](./SPEC-setup-payment-federations.md)).
- `crates/manifold-environment` — synchronous canonical deployment profiles
  shared by FI, FMan, FLIP, push-gateway guardian telemetry, and the cloud
  FMan telemetry collector
  ([SPEC-manifold-environment](../crates/manifold-environment/specs/SPEC-manifold-environment.md)).
- `crates/peer-badge-verifier` — the one complete authority, revocation,
  credential, holder-authorization, and schema verification path shared by FI,
  FLIP, push-gateway guardian telemetry, and the cloud FMan telemetry collector
  ([SPEC-peer-badge-verifier](../crates/peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)).
- `crates/fman/nostr` — the deep FMan Nostr integration: ads, operator-driven
  durable Holder-authorization enrollment, and common setup-payment policy
  refresh.
- `crates/fman/fedimint` — fedimint-client-backed ecash wallet used
  by the FMan to receive seat payments and to move collected guardian fees.
- `crates/services`, `crates/domain` — shared service plumbing and domain
  types.
- `crates/push-gateway*` — Fedi push notification gateway
  ([ARCH-push-gateway](../crates/push-gateway/specs/ARCH-push-gateway.md)); a
  separate service that can receive an auxiliary, non-load-bearing DKG
  completion callback after formation. The callback changes notification
  latency only: formation correctness and recovery do not depend on delivery.
- `crates/defe` (+ `-api`, `-client`, `-portalloc`) — local test resource
  runner (bitcoind, relays, ports) used by integration and E2E tests; has
  its own [ARCH-defe](../crates/defe/specs/ARCH-defe.md).
- `crates/tests-e2e` — end-to-end formation tests wiring fi-cli against a
  real FMan and fedimintd under defe.

## Notification role

Push notifications are non-load-bearing across federation workflows: formation,
liquidity, and recovery correctness do not depend on device or application
receipt. They can reduce response latency in larger multi-party workflows,
especially when reaching FIs, which may be the least-online but most-agentic
participants ([SPEC-hook-invocation](../crates/push-gateway/specs/SPEC-hook-invocation.md);
[CLAIM-production-ready](CLAIM-production-ready.md)).

## Documentation landscape

Current FMan, FI, and FLIP knowledge lives in Linked Specs records under
`crates/fman/specs/`, `crates/service-fleet-manager/specs/`,
`crates/fi-client/specs/`, `crates/fi-cli/specs/`,
`crates/liquidity-manager-daemon/specs/`, and
`crates/service-liquidity-manager/specs/`. Shared environment and verification
contracts live in `crates/manifold-environment/specs/` and
`crates/peer-badge-verifier/specs/`; the cross-program protocol
contracts live in `crates/nostr/specs/` and `crates/domain/specs/`. The
Fedi app design (`docs/fedi-app/`) uses
its own documentation structure, and `docs/liquidity-manager/` retains
FLIP's open-items tracker, the trust-validation implementation guide, and
Docker packaging notes.
