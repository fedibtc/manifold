# CLAIM-unrelated-manual-review-transaction-completes-operation: Unrelated manual review transaction completes operation

A `manual_review_required` wallet operation cannot transition to `completed`
from an operator-supplied txid unless FLIP has evidence that the named
transaction contains the operation's exact persisted destination and amount.

The adversary is an authenticated but mistaken operator who supplies the txid
of an unrelated Bitcoin transaction. Chain observation and a manual resolution
may race within one fixed SQLite runtime generation; a single FLIP process and
SQLite's normal transaction behavior are otherwise assumed. Bearer-token
authentication authorizes this resolution but does not make the operator's
out-of-band conclusion true. Whole-data-root restore is outside the quantified
domain: it replaces a generation's state rather than transitioning this
existing row.

## Status

Unverified.

## Assumptions

1. **A1 — Bitcoin output meaning.** A transaction id alone does not establish
   that any output pays a particular destination and amount. An operator can
   name a transaction that lacks that output.
