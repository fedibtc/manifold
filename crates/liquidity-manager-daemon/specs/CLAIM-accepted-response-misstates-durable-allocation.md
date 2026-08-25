# CLAIM-accepted-response-misstates-durable-allocation: Accepted response misstates durable allocation

A first signed `accepted` response cannot describe an allocation unless, before
that response, one committed transaction contains exactly one restart-recoverable
pending item for every and only requested positive-minimum source, with each
committed amount within that source's requested bounds. The adversary controls
request fields, delivery races, response loss, and crashes at every statement or
commit boundary, but cannot write SQLite directly.

## Status

Unverified.

## Assumptions

- **A1 — SQLite atomic durability.** A committed transaction persists all its
  writes and an uncommitted transaction persists none.
- **A2 — source semantics.** A source is requested exactly when its minimum is
  positive; a missing maximum means no upper bound beyond the selected minimum.
