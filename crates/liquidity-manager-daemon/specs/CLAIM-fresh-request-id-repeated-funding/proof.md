# Proof for CLAIM-fresh-request-id-repeated-funding

## Scope

This proof concerns
[CLAIM-fresh-request-id-repeated-funding](../CLAIM-fresh-request-id-repeated-funding.md)
in the public `RequestLiquidity` path and the documented live-restore path.
It reads [public acceptance](../../src/public.rs),
[allocation persistence](../../src/allocation_store.rs), the
[baseline migration](../../migrations/20260716000000_initial_schema.sql),
[backup and restore](../../src/backup.rs), [generation replacement](../../src/daemon.rs),
the [Admin handler](../../src/admin.rs), and the
[live restore/replay regression](../../tests/integration_live_liquidity.rs).

## Model and assumptions

Let `K` be one fixed v1 `RequestLiquidity` payload's requester key, provider
key, network, amounts, and federation details. An independent allocation is a
public-service execution that commits the allocation row and item tree, then
returns a signed `Accepted` result. It does not require transport delivery.

The assumptions of
[CLAIM-fresh-request-id-repeated-funding](../CLAIM-fresh-request-id-repeated-funding.md)
are axioms here. Live restore means the normal Admin operation while its current
runtime generation and database remain available for validation. Fresh-host
restore mode has no current generation and is outside this argument.

## Argument

1. **One installed database deduplicates by federation (schema, code, test).**
   `allocations.federation_id` is the primary key; `allocation_items` are
   constrained by federation and source. `insert_allocation` uses `INSERT OR
   IGNORE` and writes items only after that insert succeeds.
   `respond_from_existing_allocation` returns the existing accepted state or a
   request conflict before planning. The
   `request_liquidity_is_idempotent_and_detects_conflict` and
   `concurrent_duplicate_request_creates_one_allocation` tests cover sequential
   and concurrent requests in one installed history.

2. **The staged archive carries an enumerable allocation authority (code).**
   `validate_restored_state` opens the staged SQLite database and records every
   allocation's federation, requester, provider, network, and details
   commitment. `ensure_preserves_live_allocations` separately enumerates the
   running database. It accepts the staged archive only if each running
   allocation has an exactly equal identity in the staged archive. Missing and
   replaced identities return `failed_precondition`.

3. **One generation admits at most one pending restore (code, test).**
   Concurrent restore handlers may finish staging against the same captured
   generation, but they serialize on its acceptance fence. The first handler
   confirms that the fence is open, performs step 2, closes the fence, and
   queues teardown while it still holds the write side. Every later handler
   observes the closed fence and returns `unavailable`; it cannot replace or
   repopulate the process-global pending-restore slot.
   `one_generation_queues_exactly_one_restore` drives the guarded production
   transition twice, requires the first call to close admission, requires the
   second to return `unavailable`, and confirms the first archive remains in the
   pending slot.

4. **No allocation can commit between the comparison and teardown (code).**
   A request may answer from an existing allocation without taking the
   acceptance fence because that path creates no authority. A request that
   could create one takes the fence's read side after external verification and
   holds it through the SQLite commit. Live restore takes the write side before
   step 2's running enumeration, so it waits for every commit already holding a
   read guard. After comparison it closes acceptance before requesting
   generation teardown. A request still in external verification can acquire a
   read guard only after the close and returns `unavailable` without opening a
   transaction. Thus every allocation that can commit in the generation is
   included in the comparison.

5. **A regressing archive leaves the authoritative generation untouched
   (code).** The Admin handler stages and validates the archive before taking
   the fence. If step 2 finds a missing or changed identity, the handler drops
   the staged restore and returns the error without calling `request_restore`.
   The generation, database, workers, and allocation row remain installed.
   Therefore replay of `K` follows step 1's existing-allocation path rather than
   creating new funding work.

6. **An accepted archive preserves the induction hypothesis (code).** If the
   comparison passes, every allocation accepted in the current generation
   exists unchanged in the archive that `commit_live_restore` installs. A
   rebuilt generation therefore starts with all previously accepted semantic
   identities. Applying steps 2 through 5 to each later live restore preserves
   that set across any finite sequence of accepted live restores.

7. **Live-restore conflict handling is pinned end to end (test).**
   `live_restore_rejects_allocation_rollback_and_replay_stays_idempotent`
   constructs a valid fixture-backed request and coherent pre-allocation
   archive, accepts the request through the real public transport, attempts the
   documented live restore, and requires `failed_precondition`. It then replays
   the same signed request and requires an accepted answer from the unchanged
   single allocation. The test observes the required restore refusal rather than
   a second allocation.

Steps 1 through 6 establish one allocation for `K` in ordinary execution and
preserve its identity across every accepted live restore. Step 5 rejects the
only archive transition in the restore-conflict schedule, and step 7 executes
that schedule against the supported interfaces. The claim follows.

## Residuals

- Fresh-host restore mode has no newer live generation from which to establish
  cross-generation authority. Archive selection and reconciliation with
  external funding history remain operator recovery responsibilities.
- Direct database editing, data-root replacement outside the authenticated
  Admin operation, and running another daemon against the data root violate the
  claim's official-operation and single-owner assumptions.
- An archive containing the same allocation identity but older allocation-item,
  wallet-operation, or target-client progress passes this identity check.
  Cross-store backup consistency and repeated external effects without a second
  acceptance are separate properties.

## Weakest links

- The acceptance fence and its placement around the final comparison and every
  new allocation commit, plus the one-restore-per-generation admission check,
  are code-enforced rather than type-enforced.
- Equality covers the accepted semantic identity, not all later funding
  progress belonging to that allocation.
- The end-to-end regression forces the exact missing-row witness but does not
  inject every possible acceptance/restore concurrency schedule.
