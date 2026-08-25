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
  there is no such observation, B is zero and admission is forbidden. Thus the
  current schema's missing reconciliation watermark does not make B undefined;
  it makes U persist until a future mechanism supplies the proof.
- **C**, in `explicit_cap` mode, is the configured explicit allocation cap.

The feared outcome is a committed state in which a transaction has just added
or reactivated a wallet liability and either

```text
R + W + U + F > B
```

or, in `explicit_cap` mode, `R > C`. The cap is an allocation cap, so W, U,
and the global F are constrained by the wallet bound but are not charged to C;
the source fee reserve inside each active allocation is already in R.

The enumerable transaction domain is every production writer which can make
one of R, W, U, or F increase, make C decrease, or remove R/W coverage from a
possibly-spent send: (T1) accepted-request persistence in
`public::accept_or_reject_request`; (T2) operator-withdrawal insertion
in `funds_admin::request_withdrawal_with_wallet`; (T3) manual retry in
`manual_ops::retry_funding_step_with_database`; (T4) wallet sync/status writers
`wallet::apply_sync_update`, `mark_operation_failed`, and the
`mark_withdrawal_broadcast`/`mark_operation_in_doubt` send sequence; (T5)
allocation completion/failure/cancellation writers in `allocation_store` and
`manual_ops`; (T6) wallet balance observation upsert; and (T7) setup/provider
configuration writers `setup_store::apply_setup_config` and
`setup_store::update_provider_config`, when they raise F or lower C; and (T8)
a restore state replacement, which can roll durable rows/B/config back while
external gatewayd wallet state is outside the archive. Creation of an
allocation funding wallet-operation row does not add a second liability because
its amount remains covered by R. Startup recovery is read-only apart from
expiring unaccepted requests and recording its run.

The concurrency, failure, and input model permits concurrent authenticated
public requests and concurrent operator acceptance, withdrawal, retry, cancel,
sync, funds-observation, and configuration verbs; delayed and reordered
gatewayd and chain-observer responses; stale but once-correct observations; and
a daemon crash before or after any await, SQLite statement, commit, gatewayd
send, or response. After restart or restore the same schedule may continue.
These are ordinary state-consistency schedules and do not require a faulty
gatewayd, chain observer, or operator. External behavior outside A3 and
arbitrary database editing are excluded by A1.

**This claim is falsified by current code.** The argument records the writer
inventory and concrete admitted traces rather than weakening the predicate.

**Live restore.** The daemon can replace its data directory without exiting
([ARCH-liquidity-manager](../specs/ARCH-liquidity-manager.md), "Startup and
readiness"), so a restore need not be preceded by a process boundary. This does
not widen the domain above. The runtime generation holding every handle on the
data dir — periodic workers, the database, the secret store, target-federation
clients — is torn down in full before any file moves, and its replacement is
built through the ordinary startup path including startup recovery, so no state
derived from the old data dir crosses the swap. The admitted executions are
therefore exactly those of a restore performed by stopping the process,
replacing the data dir, and starting it again, which this domain already
admits. The process-lifetime shell retains only boot arguments, the
data-directory lock, and the Admin API listener, none of which this argument
reads; holding the lock across the swap rather than releasing and retaking it
narrows the window in which a second process could hold it. One process-global
value does survive a swap that a restart would reset — the wallet operation-id
counter — and it only increases, feeding an id already made unique by its
nanosecond component, so a surviving counter is narrower than a reset one.

## Status

Unverified.

## Assumptions
