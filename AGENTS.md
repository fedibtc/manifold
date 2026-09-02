This project uses the Linked Specs convention; consult the `linked-specs`
skill before working with specs or governed code.

Before changing the pinned Fedimint source, bundled daemon version/vendor
wiring, or FI release-cohort rules, read the bundled-guardian requirements in
[`SECURITY.md`](SECURITY.md).

Before adding, removing, or changing a tracing event marked
`safe_to_share = true`, consult the `safe-to-share-tracing` skill.

# Project glossary

Common domain-specific term used in this repository:

Main components/services:

- **FMan** - Fleet Manager - a component/service providing Fedimint guardian service: participation in setup and operations of Fedimint federations; One FMan can host multiple Fedimint `fedimintd` nodes. Architecture: [`ARCH-fleet-manager`](./crates/fman/specs/ARCH-fleet-manager.md).
- **Issuer** - credential issuer that signs credentials used by FIs and other services to evaluate FMans, FLIPs, and other actors. Anyone can be an Issuer if the relying app/service chooses to trust it.
- **FCS** - Fedi credential service; one Fedi-operated Issuer, not the general protocol role.
- **FLIP** - Federation Liquidity Provisioner - the Federation Liquidity Provisioner: a component/service that offers liquidity to federations. It advertises its capacity and credentials to FIs looking for it for their Fedimint federations. Architecture: [`ARCH-liquidity-manager`](./crates/liquidity-manager-daemon/specs/ARCH-liquidity-manager.md).
- **FI** - Federation Initiator - user who drives federation setup through a consumer of `fi-client` (the Fedi mobile app, or `fi-cli`); orchestrates the guardian set without being a guardian. Previously called **LGO** / Lead Guardian Orchestrator. Distinct from the FMan Operator, who runs guardians (`fedimintd`).
- **fi-client** - the Federation Initiator component's implementation: a consumer-agnostic, stateful client library carrying out the FI role (registry discovery, trust evaluation, operator selection, the formation ceremony, maintenance, liquidity, and fee arrangement). Consumers supply identity, storage, payments, and UI; the Fedi app bridge is the primary consumer and `fi-cli` is a thin wrapper for testing. Architecture: [`ARCH-fi-client`](./crates/fi-client/specs/ARCH-fi-client.md).

Other:

- **DKG** - Distributed Key Generation; Fedimint/federation setup ceremony.

## Pre-production persisted formats

Manifold has not been released into a production-like environment. Until the
first deployment whose persisted state operators expect to preserve or roll
back, or until maintainers explicitly declare persisted-format compatibility,
persisted database and backup formats may keep fixed placeholder versions and
change incompatibly. Reviews must not require migrations, backward
compatibility, or rollback compatibility during this period.

This exception ends at the first of those two boundaries. Establish the
supported format baseline and its versioning and migration policy before
crossing that boundary; do not apply this pre-production exception to later
format changes.

## Tools

### `defe` test runner

For tests using `defe`, run `just defe-serve` in a separate terminal. See the
[`defe-testing` skill](./.agents/skills/defe-testing/SKILL.md) for local test
guidance.

### `selfci`

- `selfci check` runs the full local CI verification.
- It is slower than focused checks, but runs independently of the working copy state, so it can safely run in parallel while files are being modified.
- Prefer running it as the final verification step after major changes and/or before publishing PRs.
