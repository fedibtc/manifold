# Current argument

## Argument

Current read ticks, settlement ticks, release ticks, and all-outgoing accounting
repair the older missing-watermark and withdrawal-only models. Withdrawal
admission nevertheless retains its backend response's local spendable balance
after a monotonic observation upsert can reject that older read, while the
transaction's outgoing accounting uses the newer durable watermark. This mixes
different snapshots and can commit liabilities above the durable balance, as
recorded in
[`falsification-stale-local-withdrawal-balance.md`](falsification-stale-local-withdrawal-balance.md).

## Weakest links

Admission must derive both spendable balance and reconciliation accounting from
one serialized durable snapshot after the observation update outcome is known.
