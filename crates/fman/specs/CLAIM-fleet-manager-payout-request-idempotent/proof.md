# Proof: Payout request IDs fence outgoing operations

## Scope

This argument covers `payout_jobs` in the FMan SQLite schema, the payout-job
database methods and payment/guardian orchestration in `fman-fedimint`, the
intent-level `EcashPayoutWorker` boundary, and the Fedimint payout metadata lookup/start paths.

It covers concurrent operator calls, crashes before and after either database
commit, restarts, and response loss. It does not cover loss or inconsistent
restoration of either database, which the claim excludes through its storage
assumption.

## Model and quantifiers

Fix one valid caller request ID `r` in one FMan data root. An outgoing operation
is committed when the pinned Fedimint client commits its operation log entry and
state machines. The bad outcome is two distinct committed native operation IDs
whose FMan metadata names `r`.

Every payout start enters through `SweepPaymentFees` or `SweepGuardianFees`.
`PayoutStatus` and `AwaitPayout` are observation paths and have no start
authority.

## Assumptions

The proof treats each assumption in
[CLAIM-fleet-manager-payout-request-idempotent](../CLAIM-fleet-manager-payout-request-idempotent.md)
as an axiom. Single-instance ownership bounds the in-memory scope fence to all
writers. Database durability transfers the ordering argument across restart.
The pinned-client assumption identifies the native commit boundary and makes
metadata enumeration a recovery oracle.

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
   `payout_store::create` before `start_native`, while a committed job returns
   immediately. Thus every start attempt has one durable `r`, scope, and
   destination already fixed.

3. **Lookup and start share nested wallet-scope fences (`code`, `test`).**
   `PayoutWorker::start_job` first holds `start_exclusion` for the immutable
   payment or guardian scope across `start_native` and
   `PayoutNative::start_or_recover`; the
   `concurrent_retries_start_one_native_payout` test yields inside that seam
   between lookup and native commit, so removing the worker fence permits two
   starts. In production, `WalletPayoutNative::start_or_recover` takes the
   matching `Wallet::payout_exclusion` before it opens the client, enumerates
   native operations for `r`, and calls `payout_native::start_payout` only when
   none exists. `same_scope_payout_starts_are_serialized` checks this inner
   wallet fence. Concurrent calls in one process therefore cannot both start a
   native operation for the same request and wallet scope.

4. **A committed operation carries the recovery key (`code`).**
   `payout_native::start_payout` supplies metadata binding `r` and its immutable
   destination snapshot to both Lightning v1 and v2 start APIs.
   `drain_status::payout_for_request` enumerates both native operation kinds,
   accepts only the FMan purpose plus exact request ID and destination, and
   rejects multiple matches rather than selecting one.

5. **Every crash point preserves at most one start (`enum`, `test`).**
   Before the job commit, no wallet call occurred. Between the job commit and
   native commit, replay enters the scope fence and may safely start because no
   operation exists. After the native commit, the durability assumption makes
   metadata enumeration find the operation, whether or not the wallet call
   returned or SQLite linked it. After the SQLite link, orchestration returns
   the committed job without entering start. These intervals exhaust the two
   ordered durable commits. The Defe E2E
   `fman_remits_collects_and_recovers_guardian_fee_payout_under_defe` pauses
   immediately after the native guardian-fee operation commit, kills the FMan
   before the SQLite link, and checks recovery plus same-request replay.

6. **Committed work remains discoverable (`schema`, `code`).** The SQLite
   no-delete trigger retains linked jobs. If a crash hid the link,
   `PayoutWorker::reconcile` asks the exact stored scope for native metadata
   and records the returned operation. Guardian jobs retain their public invite
   in that immutable scope, so decommission, stop, or a later DKG attempt cannot
   make the wallet unreachable. Both status and await invoke
   reconciliation. Repeated sweep invokes the same request-aware wallet start,
   which performs the same lookup under the start fence. Status and await pass
   both the linked operation ID and request ID back through the wallet boundary;
   native observation rejects an operation whose metadata names another request.

Lemmas 1–5 exclude two committed native operations for `r`. Lemma 6 establishes
restart discovery for the operation that did commit.

## Residuals

- Independent loss, rollback, or mismatched restoration of the FMan SQLite
  database and wallet databases is outside the storage assumption.
- A dependency that acknowledges a payout before its operation metadata commit,
  or omits committed metadata from enumeration after restart, violates the
  pinned-client assumption.
- The claim does not assert eventual Lightning settlement or diagnose a pending
  native operation; those are separate liveness and diagnostics properties.

## Weakest links





The cross-database crash argument still bottoms out in the pinned Fedimint
commit and enumeration assumption. The injected-crash E2E covers the exact
native-commit-before-SQLite-link interval, but the lookup-before-start ordering
remains code-enforced rather than expressed as one type.
