# REQ-fman-trusted-peer-badge: FMan operators must present Trusted-or-higher PeerBadges

Source: Fedi product requirement, required for MVP.

A badge from a configured issuer is not sufficient on its own to admit an
FMan operator. Every relying decision that accepts an FMan identity must also
require the badge's authenticated `fedi-trust-score-v1.0` `trust_level` to meet
the selected environment profile's minimum. This includes FI seat selection,
FLIP liquidity-request endorsement and federation-policy evaluation, and
push-gateway guardian-telemetry registration and cloud FMan telemetry collector
registration.

The current profile minimum is `9`. Manifold interprets the schema's bounded
numeric `trust_level` as ordered relying-party policy and therefore accepts
`9..=12`, described here as **Trusted-or-higher**. The external credential
app's exact-`9` “Trusted” display label is UI metadata, not a protocol
capability or an allowed-value set. This interpretation follows issue #195's
“minimum” and “below the threshold” language.

An authentic badge below the selected environment profile's minimum must not
be counted as trusted. Environment-profile ownership and the role-neutral
shared-verifier scope are separate design choices recorded in
[ARCH-manifold-environment](../crates/manifold-environment/specs/ARCH-manifold-environment.md)
and
[SPEC-peer-badge-verifier](../crates/peer-badge-verifier/specs/SPEC-peer-badge-verifier.md).
