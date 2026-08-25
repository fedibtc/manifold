# CLAIM-unrelated-transaction-settled-operation: Unrelated transaction settled operation

Chain-observer reconciliation cannot advance a wallet operation from Bitcoin
evidence unless it exclusively claims one transaction output paying the
operation's persisted destination and amount. For a zero-amount operator top-up,
it instead claims exactly one positive output and persists that observed amount.
A known txid alone is insufficient, and one outpoint cannot advance two
operations.

The adversary may send dust or wrong amounts to watched addresses, produce
multiple matching outputs, reuse a destination across operations, race sync
attempts, reorder observer results, and crash around database commits. The
configured observer is honest about its current Bitcoin view. Output
provenance, deep reorg response, observer/network configuration, target-side
credit, and manual-review resolution are outside this claim.

## Status

Unverified.

## Assumptions

1. **A1 — SQLite.** `BEGIN IMMEDIATE` serializes writers, committed
   transactions survive crashes, and a partial unique index rejects duplicate
   non-null `(txid, tx_vout)` pairs.
2. **A2 — observer transport.** Conforming Esplora and Bitcoin Core responses
   describe transaction output indexes, scripts/addresses, values, and
   confirmation state in the configured observer's Bitcoin view. Transaction
   ids use Bitcoin's one canonical text encoding, and Bitcoin Core
   `scantxoutset addr(A)` returns only outputs whose script matches descriptor
   address `A`.
3. **A3 — cryptographic identity.** Distinct transaction-id/output-index pairs
   identify distinct Bitcoin outputs.
