# Current argument

## Argument

**L1 (`code`) — expiry is checked only before verification.**
`pre_validation_failure` rejects `expires_at <= now_timestamp()`, but
`accept_or_reject_request` calls it before `verification_provider.verify(...).await`
and before opening the allocation transaction
([`public.rs`](../src/public.rs)).

**L2 (`enum` + `code`) — no later acceptance fence exists.** The new-allocation
path after that await contains planning, insert, commit, and `accepted_response`;
none rechecks expiry. The existing-allocation fast path is deliberately separate.

**L3 (`code`) — slow verification crosses expiry.** Deliver a request
valid for one second, let L1 pass, delay verification beyond that second, and
return a successful verification. L2 commits and signs acceptance after expiry.
The claim is false.

## Residual windows

- Repeating an already committed allocation after expiry is intentionally
  idempotent and lies outside “newly”, per `SPEC-flip-rpc`'s
  semantic-idempotency rule.
- This record does not require a remote verifier and SQLite to share a clock or
  transaction; it requires only FLIP's durable acceptance fence to recheck time.
- **The supported boundary permits the first signed `accepted` response after
the request's stated expiry.** The expiry recheck is inside the write
  transaction, so the commit is fenced; the response is signed after that
  transaction commits, and nothing bounds the interval between them. Its obvious
  contributor is the commit's own busy timeout under contention, and **no test
  measures it**.

  Such a response is late, not untrue: it reports an allocation the daemon has
  already durably accepted, inside the fence. A requester that reads `expires_at`
  as a hard deadline on the *response* will occasionally be wrong; one that reads
  it as a deadline on acceptance will not.

## Weakest links

1. **L3 (`code`)** — slow external verification schedule.
2. **L1–L2 (`code`/`enum`)** — acceptance-path inventory.
3. **A1–A2 (`axiom`)** — time and dependency-delay semantics.
