# fa-client agent notes

- Read [`ARCH-fa-client`](./specs/ARCH-fa-client.md) and `SECURITY.md` before
  changing public APIs, persistence, or lifecycle behavior.
- Consumers provide capabilities; they never provide trust conclusions or
  lifecycle transitions.
- Reuse `FaNostrClient`, `ManifoldGuardianService`, and sibling protocol types.
  Do not create parallel wire traits.
- Persist only durable formation facts required by recovery: resolved intent
  and selected locators, exact non-secret signed quotes bound to each seat's
  admission, and lifecycle checkpoints. Never persist or log
  raw bearer ecash, payment signatures, refund secrets, or identity secret
  material.
- Preserve the ordering in `SECURITY.md`: seats are bought one at a time, and
  each seat binds its exact quote before its payment; every payment is
  recoverable by quote before value moves; exact refund settlement is
  retry-safe.
- Keep the library compatible with native mobile targets and
  `wasm32-unknown-unknown`; avoid native-only dependencies in this crate.
