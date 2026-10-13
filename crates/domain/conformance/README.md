# Conformance vectors

Fixed-byte vectors for the cross-component derivations in `crates/domain`.
Independent implementations — the MG that signs a
[`MgPeerAttestation`](../src/mg_peer_attestation.rs), FLIP verifying one
against an invite-code preview, any external verifier — must reproduce these
bytes exactly. A disagreement is a protocol break that silently fails every
gated request rather than surfacing an error, which is why the values are
committed rather than recomputed.

**Changing a value here changes the protocol.** Attestations already signed by
a deployed MG stop matching what a verifier derives. Treat an edit as a
version bump of the derivation it pins, not as a test fixup. The 2026-08-06
guardian-fee-account addition changed these pre-launch vectors before any v1
MG deployment existed; there is no legacy signed directory to accept.

## `federation-config-hash-v1.json`

Pins [`federation_config_hash`](../src/federation_config.rs) and the facts
derived alongside it. Each vector carries a consensus-encoded Fedimint
`ClientConfig` as hex, which is decoded with an *empty* module decoder
registry — so the vectors also pin that the derivation needs no module crates.

`four-guardians` and `four-guardians-with-consensus-metadata` differ only in
`global.meta` and share a `federation_config_hash`. That pair is the executable
statement of the exclusion rule: the FA's `fedi:fman_seat_bindings` write must
not move the hash the guardians already signed.

## `fman-seat-bindings-v1.json`

Pins the canonical `fedi:fman_seat_bindings` value: JCS key order, the
`version` marker, the size caps, and the ascending-numeric peer-id ordering
that `numeric-peer-id-order` distinguishes from a lexicographic one, plus the
full public fee account covered by each attestation signature.

These vectors exercise structural validation only, so their `proof` fields are
fixed stub bytes rather than real signatures and their MG identities are
placeholders. Signature and seat-matching behaviour is covered by
`MgSeatBindings::verify_for_federation` and its unit tests.
