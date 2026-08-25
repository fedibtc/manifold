# CLAIM-unrelated-stability-balance-completes-item: Unrelated stability balance completes item

A production stability-pool allocation item cannot become `completed` merely
because an unrelated provider-account deposit makes the account-wide
staged-plus-locked balance large enough. Completion requires the same item's
persisted `deposit_to_provide` operation to report success for the item's exact
committed amount.

The adversary is a hostile FI with an accepted federation. It may schedule
ordinary third-party deposits to that federation, worker ticks, target API
responses, and crashes at each await. It cannot compromise FLIP's database,
the provider wallet, or the target federation/client's truthful operation
stream; Admin is trusted.

## Status

Unverified.

## Assumptions

- **A1 — durable local writes.** SQLite commits survive ordinary crashes and
  only the official daemon writes allocation state.
- **A2 — truthful target-client operation state.** The pinned stability-pool
  client reports `Success` only for its own transaction after the target
  federation accepted that transaction and resolved its change outputs.
- **A3 — target-client call integrity.** The operation id returned by
  `deposit_to_provide(amount, ...)` identifies the submitted output for that
  exact `amount` and FLIP's derived provider account.
