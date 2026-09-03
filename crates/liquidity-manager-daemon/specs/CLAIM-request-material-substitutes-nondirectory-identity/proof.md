# Current argument

## Argument

**L1 (`code`) — the directory, not request material, selects identities.**
`run_pipeline` parses and verifies consensus `fman_seat_bindings`, then derives a
distinct `BTreeSet` of operators from those verified bindings
([`verification.rs`](../src/verification.rs)).

**L2 (`code`) — resolution consults only selected identities.**
`resolve_trust_material` scans request material for duplicate public keys in
L1's operator set and ignores extras. While iterating that operator set it finds
material by the expected key and calls `verify_for_fman(expected)`, so missing
identities remain untrusted and one operator's signed material cannot substitute
for another's.

**L3 (`code`) — policy counts resolved directory identities.** Candidate envelopes
retain the resolved FMan key and the policy stage evaluates distinct resolved
identities, not arbitrary request entries ([`verification.rs`](../src/verification.rs)).

## Residual windows

- This does not assert that every required revocation authority fails closed:
  `missing-nostr-revocation-fails-open.md` already records that separate
  counterexample.
- The endorsement bearer model deliberately does not bind the requester or
  transport actor to a directory identity, per
  `SPEC-flip-rpc`.

## Weakest links

1. **L1–L3 (`code`)** — identity-set and policy-counter joins.
2. **A1–A2 (`axiom`)** — consensus and cryptographic boundaries.
