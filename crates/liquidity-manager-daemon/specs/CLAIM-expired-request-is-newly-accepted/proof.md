# Current argument

The earlier falsification argued that expiry was checked only before
verification. `accept_or_reject_request` now rechecks `expires_at` inside the
allocation write transaction, so this record keeps only the boundary that the
recheck does not close. The claim stays Unverified until a fresh verification.

## Residual windows

- Repeating an already committed allocation after expiry is intentionally
  idempotent and lies outside "newly", per `SPEC-flip-rpc`'s
  semantic-idempotency rule.
- **The supported boundary permits the first signed `accepted` response after
  the request's stated expiry.** The expiry recheck is inside the write
  transaction, so the commit is fenced; the response is signed after that
  transaction commits, and nothing bounds the interval between them. Its obvious
  contributor is the commit's own busy timeout under contention, and **no test
  measures it**. Such a response is late, not untrue: it reports an allocation
  the daemon has already durably accepted, inside the fence. A requester that
  reads `expires_at` as a hard deadline on the *response* will occasionally be
  wrong; one that reads it as a deadline on acceptance will not.
