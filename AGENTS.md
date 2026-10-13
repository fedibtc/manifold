This project uses the Linked Specs convention; consult the `linked-specs`
skill before working with specs or governed code.

Read [`SECURITY.md`](SECURITY.md) before changing project-wide trust or
production security boundaries.

Before adding, removing, or changing a tracing event marked
`safe_to_share = true`, consult the `safe-to-share-tracing` skill.

# Project glossary

Common domain-specific term used in this repository:

Main components/services:

- **MG** - Manifold Guardian - a component/service providing Fedimint guardian service: participation in setup and operations of Fedimint federations; One MG can host multiple Fedimint `fedimintd` nodes. Previously called **FMan** / Fleet Manager; shipped binary names, image names, and some wire and persisted identifiers keep the old name. Architecture: [`ARCH-manifold-guardian`](./crates/mg/specs/ARCH-manifold-guardian.md).
- **Issuer** - credential issuer that signs credentials used by FAs and other services to evaluate MGs, FLIPs, and other actors. Anyone can be an Issuer if the relying app/service chooses to trust it.
- **FCS** - Fedi credential service; one Fedi-operated Issuer, not the general protocol role.
- **FLIP** - Federation Liquidity Provisioner - the Federation Liquidity Provisioner: a component/service that offers liquidity to federations. It advertises its capacity and credentials to FAs looking for it for their Fedimint federations. Architecture: [`ARCH-liquidity-manager`](./crates/liquidity-manager-daemon/specs/ARCH-liquidity-manager.md).
- **FA** - Federation Ambassador - user who drives federation setup through a consumer of `fa-client` (the Fedi mobile app, or `fa-cli`); orchestrates the guardian set without being a guardian. Previously called **FI** / Federation Initiator, and before that **LGO** / Lead Guardian Orchestrator. Distinct from the MG Operator, who runs guardians (`fedimintd`).
- **fa-client** - the Federation Ambassador component's implementation: a consumer-agnostic, stateful client library carrying out the FA role (registry discovery, trust evaluation, operator selection, the formation ceremony, maintenance, liquidity, and fee arrangement). Consumers supply identity, storage, payments, and UI; the Fedi app bridge is the primary consumer and `fa-cli` is a thin wrapper for testing. Architecture: [`ARCH-fa-client`](./crates/fa-client/specs/ARCH-fa-client.md).

Other:

- **DKG** - Distributed Key Generation; Fedimint/federation setup ceremony.

## Public repository

This repository is public. Commits, PR text, and code must not name people,
operators, MGs, npubs, or seat and federation ids from production
incidents. Describe the scenario in general terms instead.

## Production compatibility

Production network-interface and persisted-format changes are governed by
[`GATE-production-compatibility`](specs/GATE-production-compatibility.md).
Consult it before changing interfaces used across deployments, persistence
schemas, serialized state, or backup formats.

## Tools

### `defe` test runner

For tests using `defe`, run `just defe-serve` in a separate terminal. See the
[`defe-testing` skill](./.agents/skills/defe-testing/SKILL.md) for local test
guidance.

### `selfci`

- `selfci check` runs the full local CI verification.
- It is slower than focused checks, but runs independently of the working copy state, so it can safely run in parallel while files are being modified.
- Prefer running it as the final verification step after major changes and/or before publishing PRs.
