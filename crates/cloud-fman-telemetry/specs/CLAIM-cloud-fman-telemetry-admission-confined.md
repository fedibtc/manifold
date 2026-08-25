# CLAIM-cloud-fman-telemetry-admission-confined: Target admission is confined

Against arbitrary registration bytes and identities, replay, stale credentials,
and concurrent registrations, only a fresh NIP-98 request that signs the
collector-configured URL, POST method, and SHA-256 digest of the submitted body,
and whose signer matches a fully verified current Holder subject can create a
registration or replace its registration-owned FMan public key, Iroh endpoint,
capability, or generation. A lower generation cannot replace those authority
fields, and a same-generation heartbeat can refresh its lease and authorization
material but cannot replace its capability.

## Assumptions

- The pinned Nostr primitives verify signatures and event freshness correctly.
- The collector host's wall clock stays within the configured NIP-98 freshness
  tolerance of the signer's clock.
- [`SPEC-peer-badge-verifier`](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)
  completely verifies the configured Holder authority, credential, revocation,
  schema, issuer, environment, and minimum-trust policy or fails closed.
- SQLite transactions and encrypted persistence satisfy their documented
  contracts or fail detectably.
