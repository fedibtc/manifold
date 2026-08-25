# Manifold environment crate notes

- Read [`specs/ARCH-manifold-environment.md`](specs/ARCH-manifold-environment.md),
  [`specs/SPEC-manifold-environment.md`](specs/SPEC-manifold-environment.md),
  and [`SECURITY.md`](SECURITY.md) before changing environment names, relay
  URLs, issuer identities, publisher identities, defaults, or public
  configuration types.
- Keep this crate synchronous, network-free, storage-free, and
  browser/WASM-safe.
- Relay endpoints are routing data. Consumer-specific trust, publication, and
  failure semantics belong to each consumer.
