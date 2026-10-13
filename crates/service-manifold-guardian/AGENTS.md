# Manifold Guardian service protocol agent notes

- Read [`./specs/ARCH-service-manifold-guardian.md`](./specs/ARCH-service-manifold-guardian.md)
  before changing protocol DTOs, errors, or the Iroh RPC service trait.
- Keep changes synchronized with this crate's Linked Specs and the relevant
  `crates/mg/specs/` records.
- Do not add daemon runtime policy or a dependency on `crates/mg` to
  this shared protocol crate.
