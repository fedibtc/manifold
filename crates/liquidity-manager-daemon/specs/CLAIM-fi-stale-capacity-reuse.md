# CLAIM-fi-stale-capacity-reuse: Fi stale capacity reuse

After an FI-triggered gateway or stability funding send may have debited the
configured provider wallet, a later distinct FI request cannot be durably
accepted against capacity that includes that debit until a durable wallet
observation is known to include it.

At a committed state, an observation is "known to include" a debit only if its
backend balance read occurred after FLIP durably observed that send settle. A
wall-clock timestamp, an observation persisted before settlement in the same
sync pass, or a later-looking database sequence alone is insufficient.

The adversary can submit valid endorsed FI requests, delay/reorder ordinary
wallet-balance and chain-observer replies, allow normal chain confirmation and
gateway credit, and crash/restart FLIP around awaits and commits. It cannot
forge chain evidence, alter SQLite directly, or make gatewayd report a balance
that was never true at some instant.

## Status

Unverified.

## Assumptions

- **A1 — wallet and chain semantics.** A successful or in-doubt gatewayd
  funding send may debit the wallet; exact confirmed chain output evidence can
  settle it; gateway credit can complete its item.
- **A2 — SQLite/process integrity.** Transactions are atomic/durable and a
  crash preserves committed state but not uncommitted work.
- **A3 — admission semantics.** A fresh valid endorsed request with its reserve
  no greater than `plan_allocation`'s computed capacity commits an allocation.
