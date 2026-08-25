# Current argument

## Argument

**L1 (`enum` + `code`) — acceptance trusts one cached balance after subtracting
only active reservations and `Withdrawal` rows.** `plan_allocation` reads the
singleton durable balance and calls `available_balance_for_request`. That
subtracts reserving allocation items and
`active_wallet_withdrawal_amount_tx`; the latter SQL is explicitly filtered to
`operation_type = Withdrawal`. Gateway- and stability-funding operation rows
are not independently subtracted ([`public.rs`](../src/public.rs),
[`funds_admin.rs`](../src/funds_admin.rs), [`wallet.rs`](../src/wallet.rs)).

**L2 (`code`) — production funding settlement does not carry the proposed
watermark.** Production `GatewaydFundsWallet::sync_operations` always returns
an empty list, so `apply_sync_update`—the only status writer that records
`settled_tick`—does not settle production funding operations.
`sync_wallet_operations` instead persists `balance_summary` first and then
runs `sync_chain_evidence`; `claim_chain_evidence` can move the operation to
`completed` without assigning that sequence. Even if it did, L1 ignores every
non-`Withdrawal` row ([`wallet.rs`](../src/wallet.rs),
[`funds_admin.rs`](../src/funds_admin.rs), [`wallet.rs`](../src/wallet.rs)).

**L3 (`code` + concrete execution) — an ordinary delayed-read schedule reuses
spent capacity.** Start with durable balance `B = 100`, no other liabilities,
and a 60-sat gateway FI allocation Q1 (fee reserve zero for clarity). Its item
reserves 60 and the funding worker sends 60. Concurrently, the normal sync task
has started `balance_summary` before that debit but its reply is delayed. The
reply returns the then-accurate old 100 and the task persists it. The same task
then observes Q1's exact confirmed output and settles the funding operation;
the gateway observes its normal credit and completes Q1's item, removing its
60-sat reservation. No row L1 subtracts remains, although the only durable
balance still says 100 while the wallet has 40. Q2, for a different validly
endorsed federation and amount 60, then passes `plan_allocation` and commits.
Its acceptance reuses the 60 sats possibly spent by Q1. This needs no Admin
verb or external balance-writer race; those provide additional schedules.

L1–L3 falsify the claim under A1–A3. The `observation_seq` repair protects only
operator `Withdrawal` operations and therefore does not establish the stated
FI-funding property.

## Residual windows

- The counterexample concerns capacity admission, not duplicate broadcasting:
  the exact-output settlement record remains separate.
- A deployment with no accepted public requests is vacuous, but normal
  production trust inputs and an endorsed federation make the trace applicable.
- Direct database edits and provider wallet activity outside FLIP are excluded;
  neither is needed for L3.

## Weakest links

1. **L3 (`code`)** — the interleaving uses real normal task ordering and must be
   regression-tested by a deterministic delayed-balance fixture.
2. **L2 (`enum`/`code`)** — every settlement writer must be re-enumerated if the
   wallet adapter changes.
3. **L1 (`code`)** — a broader pending-debit query would remove this mechanism.
4. **A1–A3 (`axiom`)** — external wallet/chain and SQLite behavior bottom out
   outside the record.
