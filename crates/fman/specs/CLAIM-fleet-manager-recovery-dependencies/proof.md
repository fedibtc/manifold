# Proof: Fleet Manager guardian recovery dependencies

## Scope and model

This is a compositional conditional argument for
[CLAIM-fleet-manager-recovery-dependencies](../CLAIM-fleet-manager-recovery-dependencies.md).
It covers exactly the external peer-state and dependency-contract premises in
that claim. It is not a software-readiness claim and does not inspect peer
state, dependency source, documentation, or failure-detection implementations.

The model grants every immediate assumption. “Enough correct consensus state”
means the state required for guardian recovery. “Contract-visible failure” means
a dependency either satisfies its documented contract or exposes failure to
FMan, its caller, or its operator; it does not silently violate the contract.
This proof does not establish that the premise holds, that a visible failure is
automatically remediated, or that recovery proceeds, persists, or meets a
deadline after failure.

The named integration boundaries are pinned or named deliberately: the Fedimint
client and mint cryptography use the reviewed `fedimint` flake input; iroh is the
workspace dependency; SQLite/RocksDB/filesystem and OS process semantics are
platform contracts; and Nostr is the `nostr-sdk` integration. Naming a source
pin does not verify its behavior.

## Assumption boundary and argument

1. **[assumption] Peer state.** The first premise supplies sufficient correct
   peer-held consensus state, an external recovery input FMan cannot create.
2. **[assumption] Protocol, cryptography, networking.** The second premise
   supplies contract-or-visible-failure behavior for pinned Fedimint, mint
   cryptography, and iroh.
3. **[assumption] Persistence.** The third premise supplies it for SQLite,
   RocksDB, and filesystem.
4. **[assumption] Process isolation.** The fourth premise supplies it for OS
   process isolation.
5. **[assumption] Nostr.** The fifth premise supplies it for Nostr primitives.
6. **[logic] Joint sufficiency.** These five external premises cover each named
   recovery input and dependency exactly once. They do not turn an unavailable,
   failed, or insufficient dependency into a successful recovery.

## Residuals

The claim does not establish peer retention, dependency adequacy, failure
remediation, persistence semantics, deadlines, or actual deployment behavior.
Those are external assumptions or properties of a separately stated mechanism.

## Additional current evidence

# Evidence: relay withholding injects an unreadable restore event




Scope: `crates/fman/core/src/{backup,restore}.rs`,
`crates/fman/nostr/src/backup.rs`,
`crates/nostr-clients/src/nostr_relay_client.rs`, `Cargo.lock`, and
[CLAIM-fleet-manager-restore-adopts-authentic-consistent-state](../CLAIM-fleet-manager-restore-adopts-authentic-consistent-state.md)

## Claim

Under V1's omission/delay-only fault model in
the production-readiness fault model, a withholding relay cannot exploit the pinned
verification-cache defect to inject or repeat a forged unreadable event. Every
arriving event has an authentic outer signature, while that cache trace requires
an invalid matching outer event. This record owns the cache residual
commissioned by AV8; it does not claim every authentic historical event is
readable by every build.

It does not claim that an admitted unreadable event is skipped:
recover_from_events aborts recovery on the first one.

## Axioms (trusted, not checked here)

- **A-authentic:** V1's “nothing forges, arriving bytes are authentic” means an
  arriving event was signed by the mnemonic-derived backup key and its content
  is exactly what that publisher signed.
- **A-publisher:** the in-scope FMan publisher constructs events only by sealing
  a typed BackupDocument in this build's supported envelope version.
- **A-crypto:** NIP-44 authenticated decryption and backup-key derivation behave
  as specified.

## Argument

**L1 (code) — the commissioned cache defect requires a forged outer event.**
As established by L2 of [the preserved restore proof](../CLAIM-fleet-manager-restore-adopts-authentic-consistent-state/proof.md), the pinned relay pool can
cache an event id before signature verification succeeds. Repeating that same
invalid matching event may bypass outer verification and deliver malformed
content. The first copy is nevertheless an invalid attacker-authored event
masquerading as the mnemonic-derived author. It violates A-authentic;
withholding, delay, and duplication alone cannot create it.

**L2 (code + axiom) — authentic current-build publications are readable.**
BackupIdentity::event serializes a supported typed document, NIP-44-encrypts it
to the mnemonic-derived identity, and signs with that identity. Restore derives
the same keys; unseal authenticates, decrypts, checks the supported version, and
parses the typed JSON. Under A-publisher and A-crypto, replaying such an event
yields the same readable document.

**L3 (enum) — omission can change completeness, not forge the cache seed.** V1
permits delay, omission, and answer termination over authentic events.
Delay/unavailability yields a transport error; omission plus EOSE can yield the
smaller/empty restore falsified by
relay-outage-makes-incomplete-restore-permanent.md. Duplication, if considered as extra robustness, repeats the same event. None
creates the invalid outer-signature seed needed by the cache exploit.

Thus the pinned malformed-event trace is a real weakness under a forging relay,
but that specific trace is unreachable under V1's explicitly weaker adversary. ∎

## Residual windows

- An authentic event published by a newer incompatible FMan version may be
  unreadable to an older restoring build and abort every retry after relay
  recovery. V1 states no version fault and A-publisher fixes the current format;
  compatibility needs its own fault model.
- Corruption after signing is not authentic. Mnemonic compromise, a malicious
  publisher, and cryptographic failure are outside V1.
- This verdict does not soften the whole-seat A3 falsification: all bytes in
  that trace are authentic and readable.
- If the root intends “nothing forges” to constrain only accepted bytes rather
  than all arriving bytes, it must say so. Under that stronger hostile-byte
  model, the verification-cache trace repeatedly aborts recovery.

## Weakest links

A-publisher is the load-bearing compatibility bound. Without it, authentic does
not imply readable by this build. L1 imports the cache finding rather than
re-deriving pinned SDK internals. This is a fault-model reachability result, not
a defense of fail-fast recovery or of the verification cache.
