# Fleet Manager service protocol agent notes

- Read [`./specs/ARCH-service-fleet-manager.md`](./specs/ARCH-service-fleet-manager.md)
  before changing protocol DTOs, errors, or the Iroh RPC service trait.
- Keep changes synchronized with this crate's Linked Specs and the relevant
  `crates/fman/specs/` records.
- Do not add daemon runtime policy or a dependency on `crates/fman` to
  this shared protocol crate.
