# ARCH-service-manifold-guardian: MG wire-protocol crate

This crate owns the shared Rust protocol surface for Manifold Guardian
integrations: request/response DTOs, domain wrappers, typed service errors,
and generated iroh RPC traits/clients/servers. `ManifoldGuardianService` is the FA
control plane. `GuardianTelemetryApi` is a separately-ALPN'd, bearer-scoped
raw-metrics and safe-event pull transport consumed by the Fedi telemetry
collector; it is not an FA diagnostic extension. The `fleet-manager` daemon
consumes both.

It must not depend on the `fleet-manager` daemon crate or own MG
authorization, allowlisting, or consensus policy. In addition to wire- and
storage-facing DTOs, it may own small semantic protocol values whose parsing
and validation must be identical in clients and the daemon. The shared
validated maintenance values are one such contract: they prevent
client/MG drift, while MG alone decides which keys it will authorize and
cast into consensus. Dependency or feature additions should preserve this
lightweight shared contract. When a spec introduces a typed protocol knob,
model it here first so clients and daemon cannot drift.

## Public identity display names

The crate owns `MgName`, the deterministic, presentation-only two-word name
derived from an MG's canonical `fman/v1/service-nostr` public identity. FA
and the daemon use this one shared mapping. It is a human-readable fingerprint,
not an identity or uniqueness claim: consumers authenticate and deduplicate
MGs by their public keys. The name is neither advertisement state nor an
operator setting. It is not stored or transmitted; deriving it from the public
key reproduces the same name after restart or mnemonic recovery, while replacing
that identity changes the name. Two-word collisions are accepted because the
public key remains available to disambiguate an MG.

`start_dkg` carries an optional callback in its signed payload, while
`restart_dkg` deliberately carries only the complete guardian-code set and
retains the first start's callback choice. Both reject unknown fields. The
callback value redacts `Debug` and bounds both bearer URL and idempotency key.
The protocol, persistence, and fake-gateway verification split is recorded in
[`crates/mg/testing.md`](../../mg/testing.md).

Behavior is governed by this crate's `specs/` records
([SPEC-signed-envelopes](./SPEC-signed-envelopes.md)) and the records in
`crates/mg/specs/` (notably
[SPEC-fa-rpc](../../mg/specs/SPEC-fa-rpc.md) and
[SPEC-locked-payment](../../mg/specs/SPEC-locked-payment.md));
keep the crate synchronized with them.
Guardian telemetry is governed by
[SPEC-guardian-telemetry-proxy](../../mg/specs/SPEC-guardian-telemetry-proxy.md).

The serialized service error vocabulary contains only remote domain outcomes.
Generated clients expose a typed `transport()` view for consumers that need
the local RPC transport result outside that wire result; transport
classification must remain in the consumer's connector adapter.
