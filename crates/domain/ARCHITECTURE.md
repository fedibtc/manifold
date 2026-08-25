# Architecture

`fedi-decentralized-domain` holds shared protocol primitives used by multiple
decentralized federation services. It is intentionally deterministic and
side-effect free: no network I/O, storage, environment-specific policy
selection, or service workflow state lives here.

The crate owns:

- lightweight wrapper types for identities, hashes, timestamps, URLs, and
  federation identifiers;
- signed-object canonicalization and signature-domain constants that must be
  byte-compatible across services;
- validation helpers for shared wire contracts such as `FmanApiUrlsMetadata` and
  public FMan trust-material envelopes.
- validated, deterministic policy mechanisms whose values are supplied by the
  relying component, such as the minimum PeerBadge trust-level check.

Service crates remain responsible for transport framing, authentication,
rate-limit storage, final Fedimint config matching, selecting issuer roots and
environment policy values, applying those results to service workflows, and
revocation lookups. Domain helpers deliberately stop at deterministic checks
that every verifier should perform the same way.
