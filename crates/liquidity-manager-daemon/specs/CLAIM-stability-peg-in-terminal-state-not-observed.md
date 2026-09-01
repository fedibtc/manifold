# CLAIM-stability-peg-in-terminal-state-not-observed: Stability peg in terminal state not observed

For every official-daemon stability-pool allocation item `I`, after all of the
following durable predicates hold:

1. `I` is returned by `active_stability_pool_items` (its SQLite
   `allocation_items.status` is `pending` or `running` and its `source_type` is
   `stability_pool`), its attached `wallet_operations` row is `completed`, and
   its `step_json.peg_in_operation_id` names the exact operation `O` created by
   `safe_allocate_deposit_address` for `I`;
2. the operation-log entry for `O` has wallet deposit metadata containing
   `(address, tweak_idx)`, the wallet module database contains a committed
   `ClaimedPegInKey { peg_in_index: tweak_idx, btc_out_point: P }` with
   `ClaimedPegInData { claim_txid, change }` for the deposit output `P`, every
   primary-module output in `change` is durably accepted so
   `await_primary_module_outputs(O, change)` returns `Ok(())`, and the claimed
   e-cash is included in `get_balance_for_btc`; and
3. no terminal `DepositStateV2` is cached yet in `O`'s operation-log outcome,

then a fair sequence of successful processing invocations for `I` cannot
persist only an earlier peg-in waiting state, while status readers continue to
re-report the item/overall allocation merely as `running`, forever. Public and
Admin responses do not expose the peg-in substatus itself. Here a
**successful processing invocation for `I`** is one invocation that loads `I`,
reaches `observe_peg_in(I, O)`, and returns `Ok` after committing its selected
local step. A **responsive** such invocation is one whose `observe_peg_in(I, O)`
drain reaches stream end within the budget the worker allows it. **Fair bounded
progress** requires that some finite number `N` exists such that within the next
`N` responsive invocations `step_json.peg_in_status` is committed as `claimed`
(advancement to the downstream stability deposit is not part of the bad thing).
`N` is independent of wall-clock timing in the sense that no schedule of delays
*between* responsive invocations changes it; A5 supplies their recurrence.

The enumerable domain is every FLIP production `subscribe_deposit` call and
exit,
every `DepositStateV2` update, every production local peg-in-status writer,
every active-item worker entry, and the upstream peg-in monitor and
operation-outcome writers in Scope. The adversary may hard-crash or restart at
ordinary instruction, await, and commit boundaries; delay dependencies for an
arbitrary finite time; and choose worker/dependency interleavings. Authenticated
Admin/operator setup and actions are trusted. It may not use a malicious Admin,
direct database edits, malicious configuration, out-of-band target-wallet
activity, memory corruption, or two daemon processes for one data directory.

## Status

Unverified.

## Assumptions

- **A1 (store and runtime):** successful SQLite and Fedimint client-database
  commits are atomic, durable, and faithfully reloaded after an ordinary crash
  or restart. Tokio/futures implements `yield`, `next`, stream drop, and timeout
  as written. This is the platform bottom of the argument.
- **A2 (official deployment):** the official daemon wiring has one serial
  `run_stability_pool_allocation_task` loop per data directory. An invocation is
  allowed to fail or be interrupted, but only invocations satisfying the
  Claim's explicit successful-invocation predicate count toward `N`.
- **A3 (Fedimint finality and availability):** for the durable predicate above,
  the federation has accepted the claim transaction outputs, the cryptographic
  operation/outpoint/tweak identities are collision-resistant and authentic,
  and the client's committed output state makes those notes spendable. The
  argument does not infer this from confirmations or elapsed time.
- **A4 (pin identity):** `flake.nix` exposes the locked Fedimint input as
  `.nix-deps/fedimint`; root workspace dependencies request Fedimint 0.11.1,
  the daemon inherits them with `{ workspace = true }`, and root `[patch]`
  entries select that Nix-provided source pinned by `flake.lock` to
  `fedibtc/fedimint@5703f543f76746369f0a11e0d1635ac395b2efac` (original ref
  `v0.11.1-fedi18`). Its delta changes DKG/version configuration only, not the
  wallet/client paths in this claim. The downstream stability client is pinned
  in the daemon manifest and `Cargo.lock` to
  `fedixyz/fedi@2f35ea4e3b2516d35b8ed315455718cd3b336758`. Downstream deposit
  submission is outside the core bad thing; its pin is stated to prevent
  silently importing different post-claim semantics.
- **A5 (recurring dependency responsiveness):** responsive invocations recur —
  the target federation answers a fresh subscription inside the worker's drain
  budget on at least one of every finite run of successful invocations. Without
  this premise the property is unsatisfiable rather than merely hard: the
  adversary may delay dependencies for an arbitrary finite time, every drain is
  time-bounded, and an adversary who outlasts every budget leaves the drain
  restarting from its first update forever, so no `N` exists for *any*
  implementation that bounds its waiting. Removing the bound instead is
  excluded by
  [`stability-worker-single-target-starvation`](CLAIM-stability-worker-single-target-starvation.md),
  which requires that one unresolved target cannot hold the worker. This axiom
  is the standard partial-synchrony assumption that separates "the dependency is
  down" from "the observer does not make progress", and it leaves the claim
  falsifiable by the real defect: a drain that completes and still fails to
  commit `claimed`.
