# ARCH-service-fleet-manager: FMan wire-protocol crate

This crate owns the shared Rust protocol surface for Fleet Manager
integrations: request/response DTOs, domain wrappers, typed service errors,
and generated iroh RPC traits/clients/servers. `FleetManagerService` is the FI
control plane. `GuardianTelemetryApi` is a separately-ALPN'd, bearer-scoped
raw-metrics and safe-event pull transport consumed by the Fedi telemetry
collector; it is not an FI diagnostic extension. The `fleet-manager` daemon
consumes both.

The current FI control-plane DTO set uses `fedi/fleet-manager/0.2`. Its
`GetAvailabilityResponse` carries one exact `fedimintd_version`; the preceding
0.1 list shape is a different protocol and must not share this ALPN.

It must not depend on the `fleet-manager` daemon crate or own FMan
authorization, allowlisting, or consensus policy. In addition to wire- and
storage-facing DTOs, it may own small semantic protocol values whose parsing
and validation must be identical in clients and the daemon. The shared
Guardianito-compatible maintenance values are one such contract: they prevent
client/FMan drift, while FMan alone decides which keys it will authorize and
cast into consensus. Dependency or feature additions should preserve this
lightweight shared contract. When a spec introduces a typed protocol knob,
model it here first so clients and daemon cannot drift.

## Public identity display names

The crate owns `FmanName`, the deterministic, presentation-only two-word name
derived from an FMan's canonical `fman/v1/service-nostr` public identity. FI
and the daemon use this one shared mapping. It is a human-readable fingerprint,
not an identity or uniqueness claim: consumers authenticate and deduplicate
FMans by their public keys. The name is neither advertisement state nor an
operator setting. It is not stored or transmitted; deriving it from the public
key reproduces the same name after restart or mnemonic recovery, while replacing
that identity changes the name. Two-word collisions are accepted because the
public key remains available to disambiguate an FMan.

`start_dkg` carries an optional callback in its signed payload, while
`restart_dkg` deliberately carries only the complete guardian-code set and
retains the first start's callback choice. Both reject unknown fields. The
callback value redacts `Debug` and bounds both bearer URL and idempotency key.
The protocol, persistence, and fake-gateway verification split is recorded in
[`crates/fman/testing.md`](../../fman/testing.md).

Behavior is governed by this crate's `specs/` records
([SPEC-signed-envelopes](./SPEC-signed-envelopes.md)) and the records in
`crates/fman/specs/` (notably
[SPEC-fi-rpc](../../fman/specs/SPEC-fi-rpc.md) and
[SPEC-locked-payment](../../fman/specs/SPEC-locked-payment.md));
keep the crate synchronized with them.
Guardian telemetry is governed by
[SPEC-guardian-telemetry-proxy](../../fman/specs/SPEC-guardian-telemetry-proxy.md).

The serialized service error vocabulary contains only remote domain outcomes.
Generated clients expose a typed `transport()` view for consumers that need
the local RPC transport result outside that wire result; transport
classification must remain in the consumer's connector adapter.
