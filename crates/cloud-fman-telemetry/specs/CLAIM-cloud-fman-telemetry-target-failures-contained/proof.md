# Proof: Cloud FMan telemetry target-failure containment

## Scope and model

Scope: `crates/cloud-fman-telemetry/src/*.rs`,
`crates/cloud-fman-telemetry/migrations/*.sql`,
`crates/cloud-fman-telemetry/tests/{daemon_e2e,metrics_policy,metrics_snapshot}.rs`,
`docs/telemetry/cloud-collector-deployment.md`.

This leaf has no claim imports. It quantifies over every admitted target and
stream within declared bounds; arbitrary remote response bytes and delays;
concurrent polling, registration, scrape, expiry, quarantine, shutdown, and
fatal sibling events; and every cancellation point or blocking operation from
queue admission through durability completion.

## Axioms

The enforced release limits, runtime/dependency behavior, and orchestrator grace
period in
[the claim](../CLAIM-cloud-fman-telemetry-target-failures-contained.md) are
trusted.

## Argument

1. **[test] Fair bounded attempts.**
   `hostile_target_deadlines_leave_every_due_target_a_slot_within_cadence`,
   `wake_uses_the_durable_deadline_after_a_long_cycle`,
   `hot_first_stream_cannot_starve_a_later_backlogged_stream`,
   `elapsed_target_budget_releases_permit_after_one_slow_fetch`, and
   `elapsed_retry_rotates_to_the_next_unfetched_stream` cover the named fair-slot,
   wake, stream-rotation, and elapsed-budget mechanisms.
2. **[test] Isolation and freshness.**
   `private_routes_keep_local_readiness_independent_of_remote_snapshots`,
   `partial_remote_poll_is_degraded_while_local_readiness_stays_ready`, the
   snapshot freshness tests, and
   `remote_failure_is_exposed_without_removing_healthy_targets` cover local
   readiness and non-retimed stale exposition.
   `same_day_archive_saturation_preserves_cursor_and_other_target_work` fills
   the shared archive with one target's valid frame, forces that target's next
   valid frame to be refused, and verifies that the cycle succeeds, another
   target is still fetched, and the refused stream's cursor remains at its
   archived frame.
3. **[test] Lease and revision fences.**
   `no_snapshot_target_disappears_from_cache_at_lease_expiry`,
   `metrics_exposition_and_quarantine_have_one_lifecycle_order`,
   `expiry_renewal_preserves_the_durable_attempt_deadline`,
   `stale_registration_after_fetch_rolls_back_frame_and_cursor`, and the slow
   response/cache-allocation tests cover focused expiry, revision, and retained
   allocation behavior. `Store::metric_exposition` reads the revision, next
   lease expiry, eligible snapshots, and eligible target health inside one
   immediate transaction at one caller-supplied time. Quarantine uses an
   immediate write transaction, so the two operations have one SQLite order.
   The named lifecycle-order test pauses exposition between its snapshot and
   health reads, verifies that another immediate transaction cannot cross that
   boundary, asserts both pre-transition rows, and then verifies the next
   post-quarantine view. A scrape ordered first may finish after quarantine
   commits; no scrape ordered after that commit can expose the deleted snapshot.
4. **[test] Resource bounds.** Admission/stream caps, configuration bounds,
   hostile metrics body/cardinality, aggregate-render serialization, archive
   quota, and traversal-bound tests cover the listed independent bounds.
5. **[test] Malformed target containment.**
   `cursor_overflow_is_contained_before_archive_or_cursor_commit` forces an
   authenticated cursor outside SQLite representation to become a transient
   target outcome before archive/cursor mutation; `sqlite_max_cursor_is_archived_and_committed`
   prevents an off-by-one contraction of the valid domain. Validation must occur
   before append because a later storage conversion would turn hostile target
   input into a daemon-fatal error after a durable side effect.
6. **[test] Joined durability.**
   `fatal_listener_joins_both_started_durability_workers` and
   `shutdown_signal_joins_both_started_durability_workers` inject listener or
   shutdown outcomes while both workers are inside modeled durability work and
   force `supervise` to await both definite outcomes before returning. Existing
   reservation, snapshot, archive-append, and SQLite-commit tests force each real
   worker's durability segment to finish before its worker future returns.
   Error evaluation follows both joins so no early failure can detach sibling
   durability work.
7. **[test] No queued work after shutdown.**
   `shutdown_does_not_connect_or_list_a_target_waiting_for_a_permit` uses two
   targets and one permit to force shutdown while the second waits, then verifies
   that releasing the first does not let the queued target connect, list, or
   fetch. The immediate-shutdown poller test separately forces no first connect.
   `fatal_sibling_fences_queued_target_before_permit_release` holds the fatal
   target's connect while it owns the sole permit, then confirms its sibling has
   polled the unavailable semaphore and registered as a waiter before letting the
   fatal target return. It next holds the fatal target after it publishes its
   admission fence and before it releases the permit, so the coordinator cannot
   yet observe its result. After that release, the queued sibling cannot connect
   or list. This covers the typed `PollError::Fatal` path only; the broader
   closure, including worker join errors, remains Unverified.
8. **[enum] Closure.** A hostile check must regenerate every queue, permit,
   network/parse bound, allocation, cache generation, cancellation point,
   blocking operation, worker error classification, and supervisor exit in the
   full crate scope. This complete enumeration remains Unverified.

## Evidence boundary

The daemon E2E covers one healthy target, initial registration, one metrics and
journal pull, clean SIGTERM, orphan-tail recovery, and ordinary restart. It does
not integrate crash windows, stale CAS, poison, hostile/unreachable two-target
isolation, fairness, stale transitions, bounds rejection, generation
replacement, source incarnation/gap handling, expiry/quarantine, or cross-worker
fatal cleanup. These mechanisms have focused unit/component tests; expiry and
quarantine include focused server and transactional store tests rather than only
poller tests.

## Residuals

Loads above documented bounds, dependency failures outside configured budgets,
host resource exhaustion outside reserved capacity, and orchestrator termination
before the grace period are outside the claim. Prometheus scrape availability
and history retention belong to the external backend. Malformed authenticated
remote bytes and worker failures are in scope and cannot be filed as residuals.

## Weakest links

The complete cancellation/error/allocation enumeration and real runtime
scheduling are weaker than focused deterministic tests. Cross-worker tests model
durability workers; the separate worker tests establish the corresponding real
durability joints.
