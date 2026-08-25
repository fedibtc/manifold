# Nostr protocol crate instructions

- Read [`./specs/SPEC-fman-nostr-events.md`](./specs/SPEC-fman-nostr-events.md)
  before changing any event kind, tag, constant, or document schema in this
  crate: they are cross-program contracts shared with programs outside this
  repository, and byte-level changes must be coordinated, not made
  unilaterally.
- Read [`./SECURITY.md`](./SECURITY.md) before changing signing,
  verification, or canonicalization behavior.
- Keep this crate free of relay I/O and `nostr-sdk` usage; those belong in
  `crates/nostr-clients`.
- See [`./testing.md`](./testing.md) for the fixture conventions that pin
  cross-program event shapes.
