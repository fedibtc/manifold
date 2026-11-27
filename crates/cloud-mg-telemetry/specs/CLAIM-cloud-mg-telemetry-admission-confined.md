# CLAIM-cloud-mg-telemetry-admission-confined: Target admission is confined

In badged admission mode, against arbitrary registration bytes and identities,
replay, stale credentials, and concurrent registrations, only a fresh NIP-98 request that signs the
collector-configured URL, POST method, and SHA-256 digest of the submitted body,
and whose signer matches a fully verified current Holder subject can create a
registration or replace its registration-owned MG public key, Iroh endpoint,
capability, or generation. A lower generation cannot replace those authority
fields, and a same-generation heartbeat can refresh its lease and authorization
material but cannot replace its capability. In unbadged admission mode the
same holds with any NIP-98 signer in place of a verified Holder subject, and a
new target is created only within the configured target cap and hourly
creation limits. The admission mode is fixed per instance at startup and
defaults to badged, so a badged instance never admits a signer without a badge.

## Status

Unverified.

## Assumptions

- The pinned Nostr primitives verify signatures and event freshness correctly.
- The collector host's wall clock stays within the configured NIP-98 freshness
  tolerance of the signer's clock.
- [`SPEC-peer-badge-verifier`](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)
  completely verifies the configured Holder authority, credential, revocation,
  schema, issuer, environment, and minimum-trust policy or fails closed.
- SQLite transactions and encrypted persistence satisfy their documented
  contracts or fail detectably.
