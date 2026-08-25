# PeerBadge verifier crate notes

- Read [`specs/SPEC-peer-badge-verifier.md`](specs/SPEC-peer-badge-verifier.md) and
  [`SECURITY.md`](SECURITY.md) before changing trust roots, relay resolution,
  authority selection, revocation handling, time, resource bounds, failure
  behavior, or verified output.
- Environment identity, canonical relay routing, configured issuer identities,
  and the minimum PeerBadge trust level are owned by
  `../manifold-environment`.
- Verification must fail closed on incomplete authority or revocation lookup.
- A verified badge subject is not proof of control; callers must authenticate
  the complete advertisement and bind its author to the returned subject.
