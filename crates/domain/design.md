# Design decisions

This document records design decisions for the shared domain crate that are not
large enough for a top-level component spec but still affect cross-component
protocol compatibility.

## Cross-component protocol testing strategy

Status: inferred

Shared protocol contracts in this crate need tests that make cross-component
drift obvious before transport or service code is wired up.

For every signed payload, metadata value, or serde wire shape:

- add a canonical/golden serialization test for the exact bytes or JSON value;
- add a digest/signature-domain test when the payload is signed;
- add negative validation tests for size limits, malformed identifiers, duplicate
  entries, non-canonical ordering, and ownership/binding mismatches;
- keep the selection of policy values and final relying-party decisions out of
  the domain crate, and document which caller layer owns them. Reusable,
  deterministic policy mechanisms may live here when every caller must validate
  and enforce the same caller-supplied value.

When a wire encoding is provisional, tests should still pin the current Rust
serde shape. A later protocol migration can update the tests together with an
explicit versioned spec change instead of silently changing bytes.
