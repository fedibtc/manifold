# CLAIM-wallet-budget-overcommit: Wallet budget overcommit

For the official FLIP daemon's one SQLite data root, define the durable wallet
budget at a committed database state as follows (all sums are in sats and use
the rows/config effective in that state):

- **R** is the sum of `allocation_items.reserved_amount_sats` for items whose
  status is `pending` or `running`. A reservation already contains the item's
  committed principal plus its source-specific funding fee reserve.
- **W** is the sum of `wallet_operations.amount_sats` for operator
  `withdrawal` operations whose status is `pending`, `broadcast`, `confirmed`,
  `in_doubt`, or `manual_review_required`. Funding withdrawals attached to an
  allocation item are covered by R, not W.
- **U** is the sum of possibly-spent provider-wallet sends which are no longer
  covered by R or W and whose debit is not known to be included in B, plus their
  fee exposure. “Known” requires a durable ordering/watermark connecting the
  send or its terminal reconciliation to a wallet observation; wall-clock
  timestamps alone do not establish it. Without such proof, an unwatermarked
  debit remains in U and cannot disappear merely because its row becomes
  terminal.
- **F** is the maximum of the configured gateway-funding,
  stability-pool-funding, and operator-withdrawal fee reserves, matching
  `funds_admin::fee_reserve`.
- **B** is the latest *usable* durable gatewayd spendable observation: it has
  the configured network and has not been superseded by a newer observation.
  Every send not durably proven included in that observation remains in U. If
  there is no such observation, B is zero and admission is forbidden.
- **C**, in `explicit_cap` mode, is the configured explicit allocation cap.

The feared outcome is a committed state in which a transaction has just added
or reactivated a wallet liability and either

```text
R + W + U + F > B
```

or, in `explicit_cap` mode, `R > C`. The cap is an allocation cap, so W, U,
and the global F are constrained by the wallet bound but are not charged to C;
the source fee reserve inside each active allocation is already in R.

The concurrency, failure, and input model permits concurrent authenticated
public requests and concurrent operator acceptance, withdrawal, retry, cancel,
sync, funds-observation, and configuration verbs; delayed and reordered
gatewayd and chain-observer responses; stale but once-correct observations; and
a daemon crash before or after any await, SQLite statement, commit, gatewayd
send, or response. After restart or restore the same schedule may continue.
These are ordinary state-consistency schedules and do not require a faulty
gatewayd, chain observer, or operator. External behavior outside A3 and
arbitrary database editing are excluded by A1.

## Status

Unverified.

## Assumptions
