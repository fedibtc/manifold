# Domain crate instructions

- Read `./ARCHITECTURE.md` before changing shared protocol types, signed
  payloads, validation helpers, canonicalization, or crate boundaries.
- Read `./design.md` before changing cross-component protocol test strategy.
- Keep cross-component protocol data shapes here when more than one service crate
  must agree on canonical bytes, validation rules, or signature domains.
- Public structs and fields need doc comments because these types are the Rust
  contract for the specs.
- Add canonical/golden tests whenever introducing signed payloads, metadata
  values, or serde wire shapes.
- Keep network I/O and service-specific policy out of this crate; expose
  deterministic validation helpers that callers can combine with their local
  policy.
