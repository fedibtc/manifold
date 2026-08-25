# Current argument

## Argument

### L1 — allocation-funding has exactly two production send-trigger call sites (`enum`, `code`)

`allocation_funding::submit_funding_withdrawal` is the shared irreversible
provider-wallet send trigger for an allocation item. Its current callers are
`gateway::process_gateway_item` and
`stability_allocation::process_stability_pool_item`; each calls it only after
loading the item's durable wallet operation as `pending`. The operator-withdrawal
path in `funds_admin` is deliberately outside this claim because it is not an
allocation worker. The same source search finds direct `#[cfg(test)]` calls
below `gateway`'s `mod tests`, which assert zero sends and are not
production triggers.
Regenerate this list with:

```text
rg -n 'submit_funding_withdrawal' crates/liquidity-manager-daemon/src
```

Before calling `submit_prepared_withdrawal`, the shared trigger atomically
changes the operation from `pending` to `in_doubt`. It sends only if that
conditional transition succeeds.

### L2 — the lost-response state reaches manual review without evidence (`code`, `test`)

After the first call in L1, `SubmitWithdrawalError::InDoubt` retains the
operation as `in_doubt`, with no txid. `funds_admin` escalates an unresolved
`in_doubt` operation through
`wallet::escalate_in_doubt_to_manual_review`; that conditional update
requires the configured age threshold and no intervening settlement. The focused
test
`gateway::tests::manual_safe_to_retry_resubmits_an_accepted_unknown_gateway_send`
uses a test wallet that records the externally accepted first send before
returning `InDoubt`, keeps sync and gateway evidence absent, then explicitly
performs that escalation. It observes the durable sequence:

```text
pending --submission fence--> in_doubt
        --no evidence + review threshold--> manual_review_required
```

A1 supplies the production meaning of the test wallet's accepted-but-unanswered
first call.

### L3 — authenticated manual resolution trusts the retry assertion (`code`, `test`)

`admin::app` installs the manual-review route behind `require_auth`, and
`admin` delegates it to `manual_ops::resolve_manual_review`. The
resolution checks only that `SafeToRetry` carries no txid and that the operation
is still `manual_review_required`. It does not query the wallet, chain observer,
or target before `wallet::resolve_manual_review_tx` writes:

```text
manual_review_required --SafeToRetry--> pending
```

The focused test drives that real resolution transaction and observes the
persisted `pending` status. Its operator conclusion is intentionally mistaken;
authentication does not rule that out.

### L4 — the next worker pass sends the same durable operation again (`test`, `code`)

The test then resumes the gateway worker. Its active item reloads the same
operation in `pending`, so L1's submission fence succeeds and its test wallet
records a second send. The deterministic count changes from one accepted
lost-response send to two sends. No production behavior changes: the test-only
wallet and test-only access seam reproduce the existing production transitions.

### L5 — the sequence falsifies the claim (`test`, `code`)

Combining L2 through L4 yields the exact counterexample:

```text
first external send accepted
  -> response/txid lost; no evidence available
  -> in_doubt
  -> manual_review_required
  -> authenticated but mistaken SafeToRetry
  -> pending
  -> resumed allocation worker submits a second send
```

This execution reaches the named bad thing under A1, so the claim is false.

## Residual windows

- A `Completed` or `Failed` manual resolution does not return the operation to
  `pending`; it is outside this duplicate-send counterexample.
- `in_doubt` and `manual_review_required` operations without a `SafeToRetry`
  resolution are not automatically resubmitted. That existing fence does not
  constrain the manually reopened state.
- This record does not claim that the second send settles, that the two sends
  pay distinct outputs, or that a target credits either one. The bad thing is
  the second provider-wallet submission itself.
- Standalone operator withdrawals, whole-data-root restore, and multiple daemon
  processes are outside the stated allocation-worker, one-generation domain.

## Weakest links

1. **L1 (`enum`/`code`)** — a new allocation-worker call site or send trigger
   must regenerate the scope of this counterexample.
2. **L3 (`code`/`test`)** — `SafeToRetry` currently accepts an authenticated
   operator assertion without affirmative non-send evidence.
3. **L4 (`test`/`code`)** — the focused reproduction ties the reopened durable
   state to the actual second worker submission.
4. **L2 (`code`/`test`)** — the escalation path supplies the exact
   no-evidence reviewed state.
5. **L5 (`test`/`code`)** — A1 transfers the test backend's accepted call to
   the external-send counterexample.
