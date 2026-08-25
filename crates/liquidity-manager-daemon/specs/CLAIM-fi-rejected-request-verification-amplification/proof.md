# Current argument

## Argument

**L1 (`code`) — an FI chooses an arbitrarily long replay window.** Public
prevalidation rejects only an expiry at or before `now`; it has no maximum
lifetime or `issued_at` freshness bound. The signed request's details hash
commits to that chosen future expiry, so one valid signature remains a valid
request throughout that window.

**L2 (`enum` + `code`) — each replay of a fixed late-rejection trace reruns
outbound verification.** Consider two FMan identities under `AllTrusted`: FMan
0 has the valid endorsed seat and a live advertisement with a valid envelope;
FMan 1's relay answers with no usable advertisement. The pipeline admits FMan
0, previews the invite, resolves both advertisements, runs the later required
revocation lookup, then returns `policy_mismatch`. A request with no existing
allocation calls `VerificationProvider::verify` outside a transaction, and
that rejection returns `stateless_rejection` with no durable
request/rejection/cache row. Each replay of this construction therefore takes
the same outbound path. Other rejection classes can exit earlier and are not
claimed to run every stage.

**L3 (`code` + concrete execution) — one credentialed FI repeats unbounded
network fanout.** The FI signs a request with a far-future expiry and the L2
endorsement/policy construction. Every sequential delivery passes admission,
invokes L2's preview/relay work, reaches `policy_mismatch`, and persists
nothing. The FI repeats after each response. The shared Iroh protocol limits
simultaneous handlers to 128, but releases a permit after each complete
response; it does not impose a cumulative request rate, cache, or quota. The
outbound work therefore grows with sequential retries without bound. This
falsifies the claim.

## Residual windows

- The incomplete-stream finding is separate: this trace uses complete,
  signature-valid frames and consumes trust dependencies rather than holding a
  pre-frame permit.
- An accepted first request takes the existing-allocation fast path on replay;
  L3 deliberately chooses a stateless late rejection.
- Relay failure may shorten one attempt with `provider_unavailable`; it does not
  create a negative cache or rate limit. L3 uses answering relays and the
  deliberate `policy_mismatch` trace instead.

## Weakest links

1. **L2 (`enum`/`code`)** — rejection exits and lookup stages must be
   regenerated when the trust pipeline changes.
2. **L1 (`code`)** — request freshness policy is a local check.
3. **A1–A2 (`axiom`)** — network-effect and feasible-policy-failure premises.
