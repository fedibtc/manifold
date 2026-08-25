# FLIP release envelope

The workload limits, dependency-availability preconditions, allocation
deadlines, and recovery objective a FLIP release operates under.

[`CLAIM-liquidity-manager-production-ready`](../../crates/liquidity-manager-daemon/specs/CLAIM-liquidity-manager-production-ready.md)
assumes these exist. Its bounded-completion and recovery language has no
referent without them: "accepted supported allocations reach completion or an
actionable terminal state within their documented deadline" is not a claim
about anything until the deadline is written down.

Values marked **derived** are read out of the code and change with it. Values
marked **proposed** are not derivable and are stated here as the release's
commitment; they are the ones to argue with.

## Deployment shape

One FLIP process, one runtime generation, one SQLite data root, one configured
gateway. The Admin API binds to a local or private interface, or sits behind
operator-controlled access enforcement.

## Workload limits

| Limit | Value | Source |
|---|---|---|
| Concurrent public RPC stream handlers | 128 | `fedi-iroh-rpc`, `iroh_protocol.rs` — **derived** |
| Verification runs per federation per 5-minute window | 12 | `verification_budget.rs` — **derived** |
| Federations tracked by that budget | 4096 | `verification_budget.rs` — **derived** |
| Request lifetime (`expires_at - issued_at`) | ≤ 3600 s | `public.rs`, matching `FI_LIQUIDITY_REQUEST_VALIDITY` — **derived** |
| `issued_at` clock skew tolerated | 120 s | `public.rs` — **derived** |
| Trust material validity | 3600 s | `verification.rs` — **derived** |
| Target federation clients held open | 8, configurable | `--max-open-target-clients` — **derived** |
| Target-client operation-log scan depth | 50 | `stability_pool.rs` — **derived** |
| Allocations per federation | 1, one item per source | schema unique index — **derived** |

Two of these are ceilings on requester-driven work rather than capacity
figures, and are the ones that decide behaviour under load: the per-federation
verification budget bounds outbound trust work within each five-minute window,
and the request lifetime bounds how long one signature stays deliverable. The
renewable budget is a rate limit, not a lifetime cumulative bound.

Nothing here bounds the number of *federations* an FI can get endorsed. That is
an issuer and FMan question, not a FLIP one, and several FLIP bounds are
per-federation, so the release's real workload ceiling is set by the endorsement
policy of the issuers an operator installs.

## Dependency-availability preconditions

FLIP holds no allocation to a deadline while a dependency it needs is
unavailable. Each of these must be reachable and answering:

| Dependency | Used for | Timeout | Source |
|---|---|---|---|
| gatewayd Admin API | balance, deposit address, sends | 15 s validation | `setup_store.rs` — **derived** |
| Chain observer (Esplora or Bitcoin Core) | settlement evidence, target-client peg-in | 2 s validation | `setup_store.rs` — **derived** |
| Nostr relays (issuer revocation) | fresh revocation checks | 5 s connect, 5 s fetch, 16 events | `nostr.rs`, `revocation.rs` — **derived** |
| Target Fedimint federation | peg-in, stability deposit | 30 s per item per pass | `stability_allocation.rs` — **derived** |
| SQLite data root and filesystem | everything durable | — | — |

FLIP fails closed rather than proceeding when it cannot complete a required
fresh revocation check. Treat relay availability as a precondition for
*admission*, not merely for freshness.

A Bitcoin Core chain observer cannot serve a target federation's wallet client,
which has no Bitcoin Core path. Deployments that fund stability-pool
allocations need an Esplora observer, or their target clients fall back to the
endpoint the target federation advertises.

## Worker cadence

| Worker | Interval | Source |
|---|---|---|
| Gateway allocation | 10 s | `gateway.rs` — **derived** |
| Stability-pool allocation | 10 s | `stability_allocation.rs` — **derived** |
| Gateway observation | 30 s | `gateway.rs` — **derived** |
| Wallet operation sync | 30 s | `funds_admin.rs` — **derived** |
| Advertisement reconcile | 60 s | `advertisement.rs` — **derived** |

## Allocation deadlines — **proposed**

An accepted allocation reaches completion or an actionable terminal state
within:

| Source | Deadline | Reasoning |
|---|---|---|
| Gateway | 6 hours | Dominated by on-chain confirmation of the funding send at the configured depth (3 on mainnet), plus the gateway's own claim. |
| Stability pool | 12 hours | The same funding send, then a target-federation peg-in claim, then the provider deposit — two chain-confirmation waits in series rather than one. |

These are wall-clock commitments *given the preconditions above*. Time during
which a required dependency is unavailable does not count against them: FLIP
retries rather than failing, and an allocation waiting on an unreachable
federation is not a missed deadline but an unmet precondition.

"Actionable terminal state" means `failed` or `action_required` with a
non-secret failure reason reachable through the Admin API. `action_required` is
the honest outcome for work that needs an operator, and the recovery surfaces
for it are `retry_funding_step`, `cancel_allocation`, `inspect_target_client`,
`bind_target_deposit`, `abandon_target_client_value`, and `resolve_manual_review`.

An in-doubt wallet send escalates to manual review after 21 600 s (6 hours) by
default (`in_doubt_review_after_secs` — **derived**), which is the mechanism
that keeps a send from sitting unresolved past the gateway deadline.

## Recovery objective — **proposed**

| Objective | Value |
|---|---|
| Recovery time after restart | 5 minutes to serving requests |
| Recovery time after restore from backup | 30 minutes, operator-driven |
| Recovery point | The last committed SQLite transaction |

Restart recovery is bounded by startup recovery over active allocations and
wallet operations, all local reads. Restore is operator-driven — inspect the
archive, then apply it — so its figure is a target for the procedure, not
something the daemon enforces.

The recovery *point* deserves care. FLIP's durable state is one SQLite root, so
a restore returns it to the archive's state exactly. What a restore does not
return is the external world: the gateway wallet, the chain, and target
federations kept moving. An allocation committed after the archive was taken is
gone from FLIP's records while its funding send may have happened. That
reconciliation is not automated and is the residual under
[`wallet-budget-overcommit`](../../crates/liquidity-manager-daemon/claims/wallet-budget-overcommit.md).

## Excluded from this envelope

- Bitcoin mainnet with `--trust-fixtures` or
  `--allow-private-federation-endpoints`. Both are refused against a stored
  mainnet configuration.
- More than one process against one data root.
- Federations whose configuration lacks a module a requested source needs;
  these are refused at admission rather than operated under.
- Returning value that reached a target client and could not be deposited.
  There is no sweep; the manual route is
  [`liquidity-manager-recovery-runbook.md`](./liquidity-manager-recovery-runbook.md).

## Standing of this document

The derived values are checkable against the code and should be corrected when
it changes. The proposed ones — the two allocation deadlines and the three
recovery figures — are stated so the claim has a referent, and have not been
measured against a running deployment. Replace them with measured figures when
the live suite can produce them.
