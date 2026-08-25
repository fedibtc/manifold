# fi-client agent notes

- Read [`ARCH-fi-client`](./specs/ARCH-fi-client.md) and `SECURITY.md` before
  changing public APIs, persistence, or lifecycle behavior.
- Consumers provide capabilities; they never provide trust conclusions or
  lifecycle transitions.
- Reuse `FiNostrClient`, `FleetManagerService`, and sibling protocol types.
  Do not create parallel wire traits.
- Persist only durable formation facts required by recovery: resolved intent
  and selected locators, exact non-secret signed quotes, the aggregate
  exact-quote authorization, and lifecycle checkpoints. Never persist or log
  raw bearer ecash, payment signatures, refund secrets, or identity secret
  material.
- Preserve the ordering in `SECURITY.md`: complete quote-set readiness and its
  exact aggregate authorization precede independent spends; every payment is
  recoverable before value moves; exact refund settlement is retry-safe.
- Keep the library compatible with native mobile targets and
  `wasm32-unknown-unknown`; avoid native-only dependencies in this crate.
