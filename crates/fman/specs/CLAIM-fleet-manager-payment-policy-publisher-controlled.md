# CLAIM-fleet-manager-payment-policy-publisher-controlled: Accepted setup-payment policy is publisher-controlled

For the official production Fleet Manager daemon and one data root, after the
database is initialized:

1. every federation ID which the FMan treats as a member of its accepted
   setup-payment set is derived from the content of a complete kind-37707
   Nostr event whose ID and signature verify, whose author is the
   deployment-pinned setup-payment publisher, and which passed all of the
   admission checks enumerated in L2;
2. no actor other than that publisher can add, remove, or replace an accepted
   member, including by operating the configured relay, authoring or replaying
   events under arbitrary keys, choosing an author in request data, or calling
   any FI or operator RPC; and
3. whenever replacement of the retained event removes at least one previously
   accepted federation, the removal, complete replacement event, replacement
   membership, and a newly drawn offer epoch become visible in one SQLite
   commit. No quote or fresh allocation decision can observe the removed
   membership with the preceding epoch, and an outstanding quote carrying that
   preceding epoch is refused rather than accepted.

“Treats as a member” covers every policy consequence in the daemon: selection
for a priced quote, the operator's `accepted` status, and wallet join
reconciliation. The join reconciler deliberately retains wallet
state for removed members, but that state is not acceptance and cannot make a
new quote name the removed member.

The adversary controls arbitrary Nostr relays and event authors, including
forged, replayed, stale, self-authorized, malformed, and oversized candidates;
arbitrary FI RPC bytes and verb concurrency; and crash points before, during,
or after durable writes. The adversary does not control the host, the database
files, the official daemon binary, or the deployment-pinned publisher key.

This claim is about authority over FMan policy and the epoch consequence of a
removal. It does not claim that the pinned publisher chooses a safe or live
federation, that relays deliver updates, that a wallet join succeeds, or that a
removed wallet and its balance are erased.

## Status

Falsified: a withholding relay can suppress newer publisher history and make an older authentic signed policy current.

## Assumptions

- **A1 Nostr cryptography:** Schnorr signatures are unforgeable and the Nostr
  event ID hash is collision/preimage resistant. `Event::verify` therefore
  binds the complete event's ID, author, timestamp, kind, tags, and content to
  the corresponding secret key.
- **A2 publisher pin and deployment integrity:** the Manifold environment
  selected by the official daemon supplies the intended setup-payment
  publisher public key without an attacker-controlled production override;
  the publisher key is uncompromised. Publisher malice or damaging
  misconfiguration is outside this claim, as documented in `SECURITY.md`.
- **A3 invite dependency semantics:** the pinned Fedimint invite parser
  faithfully rejects invalid invites, reports an embedded API bearer secret,
  and derives the canonical federation ID represented by an invite.
- **A4 SQLite and randomness:** SQLite/SQLx provides the stated transaction
  isolation, atomic commit, rollback, and crash durability; the schema and
  foreign/check constraints execute as written. `rand::random()` supplies an
  unpredictable 32-byte value which does not repeat a preceding live offer
  epoch in practice.
- **A5 official single-instance wiring:** the official daemon is the sole
  process with write access to the data root, holds its exclusive data-root
  lock, binds the data root to its selected Manifold environment before
  onboarding or loading fleet state, revalidates retained policy before exposing
  FI RPC, and starts its Nostr runtime once. Safe Rust privacy and module
  boundaries are not bypassed; test-only/direct library callers, memory
  corruption, a modified binary, and external database mutation are excluded.
- **A6 clock:** `Timestamp::now()` reflects the host's approximately correct
  wall clock, so the stated 24-hour future-timestamp check has its intended
  meaning.
