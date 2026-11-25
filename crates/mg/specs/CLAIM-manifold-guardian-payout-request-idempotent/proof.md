# Proof: Payout request IDs fence outgoing operations

## Scope

This argument covers `payout_jobs` in the MG SQLite schema, the payout-job
database methods and payment/guardian orchestration in `mg-fedimint`, the
intent-level `EcashPayoutWorker` boundary, and the Fedimint payout
operation-id lookup and start paths.

It covers concurrent operator calls, crashes before and after either database
commit, restarts, and response loss. It does not cover loss or inconsistent
restoration of either database, which the claim excludes through its storage
assumption.

## Model and quantifiers

Fix one valid caller request ID `r` in one MG data root. An outgoing operation
is committed when the pinned Fedimint client commits its operation log entry and
state machines. The bad outcome is two distinct committed native operations
started for `r`.

Every payout start enters through `SweepPaymentFees` or `SweepGuardianFees`.
`PayoutStatus` and `AwaitPayout` are observation paths and have no start
authority.

## Assumptions

The proof treats each assumption in
[CLAIM-manifold-guardian-payout-request-idempotent](../CLAIM-manifold-guardian-payout-request-idempotent.md)
as an axiom. Single-instance ownership bounds the in-memory scope fence to all
writers. Database durability transfers the ordering argument across restart.
The pinned-client assumptions identify the native commit boundary, make lookup
by operation id a recovery oracle, and fix which operation id each start
records. The legacy assumption excludes jobs whose operation the ledger did not
name.

## Argument

1. **One durable identity (`schema`, `test`).** `payout_jobs.request_id` is the
   primary key. Scope and destination are immutable, and
   `payout_store::create` accepts an existing row only when both equal the new
   inputs. The test
   `payout_jobs_pin_inputs_and_committed_operations_across_restart` checks
   identical replay, divergent replay, immutable operation linking, deletion
   refusal, and reopen.

2. **The job precedes start (`code`).** Both public sweep methods delegate to
   `PayoutWorker::start_job`. Its new-request branch calls
   `payout_store::create` before `PayoutNative::start`, while a committed job returns
   immediately. Thus every start attempt has one durable `r`, scope, and
   destination already fixed.

3. **The ledger names the operation before start (`code`, `test`).** An
   ecash job's operation id is `PayoutOperationId::ecash(r)`, a hash of `r`
   alone. A Lightning job's is `LightningAttempt::operation_id`, a function of
   the attempt's rail and invoice that `payout_store::attempt` writes before
   `payout_native::pay_payout` pays it; `pay_payout` fails unless the client
   returns that id. `prepare_payout` refuses an invoice whose operation, or for
   v1 whose earlier attempt, already exists in the wallet, so the named
   operation can only be this job's. The test
   `payout_jobs_pin_inputs_and_committed_operations_across_restart` checks that
   an attempt is replaceable before the link and immutable after it.

4. **Lookup and start share the wallet-scope fence (`code`, `test`).**
   `WalletPayoutNative::start` takes `Wallet::payout_exclusion` for the job's
   scope before it opens the client and holds it through lookup and start.
   For ecash, `start_ecash_payout` looks up the named operation and spends
   under that id only when it is absent; the pinned client refuses a second
   spend under the id even outside the fence. For Lightning, it rereads the
   job, returns an operation already linked or found for the recorded attempt,
   and otherwise records a fresh attempt and pays it. An attempt is replaced
   only under the fence after finding no operation for it, and no payment of
   it can be running, because each payment starts and returns under the
   fence. `same_scope_payout_starts_are_serialized` checks the wallet fence,
   and `concurrent_retries_start_one_native_payout` checks that concurrent
   sweeps link one operation.

5. **Every crash point preserves at most one start (`enum`, `test`).**
   Before the job commit, no wallet call occurred. Between the job commit and
   native commit, the named operation does not exist, so replay may start;
   for Lightning it replaces an attempt that was never paid. After the native
   commit, the durability assumption makes the lookup find the named
   operation, whether or not the wallet call returned or SQLite linked it.
   After the SQLite link, orchestration returns the committed job without
   entering start. These intervals exhaust the ordered durable commits. The
   Defe E2E `mg_remits_collects_and_recovers_guardian_fee_payout_under_defe`
   pauses immediately after the native guardian-fee operation commit, kills
   the MG before the SQLite link, and checks recovery plus same-request
   replay.

6. **Committed work remains discoverable (`schema`, `code`).** The SQLite
   no-delete trigger retains jobs, and a linked operation and its attempt are
   immutable. If a crash hid the link, `PayoutWorker::reconcile` looks up the
   named operation in the exact stored scope and records it. Guardian jobs
   retain their public invite in that immutable scope, so decommission, stop,
   or a later DKG attempt cannot make the wallet unreachable. Both status and
   await invoke reconciliation, and repeated sweep performs the same lookup
   under the fence.

Lemmas 1–5 exclude two committed native operations for `r`. Lemma 6 establishes
restart discovery for the operation that did commit.

## Residuals

- Independent loss, rollback, or mismatched restoration of the MG SQLite
  database and wallet databases is outside the storage assumption.
- A dependency that acknowledges a payout before its operation commit, loses a
  committed operation from lookup after restart, or records a payout under an
  id other than the one the ledger derives, violates the pinned-client
  assumptions. `pay_payout` detects the last for Lightning before linking.
- The claim does not assert eventual Lightning settlement or diagnose a pending
  native operation; those are separate liveness and diagnostics properties.

## Weakest links

The cross-database crash argument still bottoms out in the pinned Fedimint
commit and lookup assumptions, and the Lightning operation ids are derived in
MG from the pinned clients' private derivations. The injected-crash E2E covers
the exact native-commit-before-SQLite-link interval, but the lookup-before-start
ordering remains code-enforced rather than expressed as one type.
