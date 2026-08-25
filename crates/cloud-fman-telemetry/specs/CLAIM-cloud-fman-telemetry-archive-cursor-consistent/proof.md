# Proof: Cloud FMan telemetry archive/cursor consistency

## Scope and model

Scope: `crates/cloud-fman-telemetry/src/{archive,archive_tests,config,data_root_lock,iroh_journal_source,journal_catalog,journal_collector,journal_commit,journal_poller,journal_poller_tests,journal_target,journal_types,journal_types_tests,lib,main,server,store}.rs`,
`crates/cloud-fman-telemetry/Cargo.toml`, `Cargo.lock`,
`crates/cloud-fman-telemetry/migrations/{0001_runtime,0002_journal_archives}.sql`,
`crates/cloud-fman-telemetry/tests/daemon_e2e.rs`,
`docs/telemetry/cloud-collector-deployment.md`,
`specs/ARCH-cloud-fman-telemetry.md`.

This leaf has no claim imports. It quantifies over every validated nonempty
journal batch, concurrent revision/lease change, cancellation or fatal sibling,
crash point from response validation through archive sync and SQLite commit,
ordinary restart, retention after the committed reception-day cutoff, and a
supported coupled backup and restore.

## Axioms

The single-owner, common-volume, storage-contract, and protected-lifecycle
assumptions in
[the claim](../CLAIM-cloud-fman-telemetry-archive-cursor-consistent.md) are
trusted. An application argument cannot establish physical durability or detect
an arbitrary self-consistent rollback of the whole volume. The official binary
and pinned dependencies execute the reviewed code without memory corruption or
code injection. SQLite `synchronous=FULL` WAL commits and Rust's successful
filesystem operations have their documented durability and atomicity semantics.

## Argument

1. **[test] Validated representable batch.**
   `cursor_coordinates_must_fit_the_durable_sqlite_domain` forces validation to
   accept `i64::MAX` and reject either cursor coordinate above it.
   `cursor_overflow_is_contained_before_archive_or_cursor_commit` forces that
   validation before archive append and cursor commit, while
   `sqlite_max_cursor_is_archived_and_committed` forces the full admitted
   boundary through both durability layers.
2. **[test + code] Archive before cursor.** Archive tests force exact independent
   frames and concatenation, serialized quota admission, poison after an
   indeterminate append failure, and recovery at committed hash/offset
   boundaries. The sync and SQLite ordering are code-reading evidence:
   `JournalArchive::append_reserved` writes the complete compressed frame, calls
   `sync_data`, and, for a new day file, syncs its directory before returning
   its boundary. Only then can `commit_if_current` call the SQLite transaction
   that inserts that boundary and advances the cursor. A crash before SQLite
   commit therefore leaves the prior cursor plus, at worst, an orphan archive
   tail; a crash after commit has both the synced frame and the atomic
   cursor/boundary row.
3. **[test + code] Failed or stale commit.**
   `stale_registration_after_fetch_rolls_back_frame_and_cursor` forces a stale
   revision to truncate its uncommitted frame rather than adopt its cursor.
   `truncate_uncommitted`'s following `sync_data` is code-reading evidence. Any
   other SQLite failure becomes a fatal worker result and leaves an orphan tail
   for startup recovery; it cannot acknowledge a cursor commit.
4. **[code] Discontinuity.** Full-value incarnation comparison in
   `open_journal_stream`, `commit_incarnation_change`, and
   `ValidatedJournalBatch`, plus the adjacent transactional updates, makes
   reported same-incarnation gaps and incarnation changes increment generation
   and retain gap metadata with the adopted position. Both the authenticated
   listing path in `open_journal_stream` and the typed
   `IncarnationChanged` fetch path reset the cursor and increment the generation
   and gap count; neither orders UUID values. A gap-marked nonempty batch adopts
   its returned position only in the same transaction that records its gap and
   frame.
5. **[test] Joined durability.**
   `fatal_listener_joins_both_started_durability_workers` and
   `shutdown_signal_joins_both_started_durability_workers` force the supervisor
   to signal shutdown, then await both worker outcomes before evaluating and
   returning a failure. Existing poller tests force archive and SQLite work
   already inside their durability segments to finish before those workers
   return.
6. **[enum + code] Cursor and ledger closure.** The only source-cursor SQL
   writers are `open_journal_stream` (listed-incarnation reset),
   `commit_journal_batch` (validated next cursor), and
   `commit_incarnation_change` (typed fetch-incarnation reset). The latter two
   are reachable only through `JournalCatalog::commit_if_current`; admission,
   metrics, including the transactional exposition cleanup, HTTP handlers, and
   retention do not write cursor coordinates.
   `commit_journal_batch` is the sole frame-row insert and atomically writes the
   frame boundary, hash, gap, generation, and cursor under both target-revision
   and expected-stream-state checks. `prune_archive_ledger` is the sole
   frame-row deletion and deletes only reception days before the committed UTC
   cutoff.
7. **[enum + code] Archive and quota closure.** The archive has four mutators:
   `append` writes and syncs a reserved frame; `truncate_uncommitted` syncs a
   stale-CAS rollback; `recover` verifies the last committed frame hash and
   boundary for every retained stream/day and truncates or removes everything
   later; and `prune_before` removes only files before the ledger-committed
   cutoff and syncs each containing directory. One archive mutex serializes all
   four and all quota changes. Append reserves compressed bytes before I/O,
   every indeterminate append error poisons further admission, stale truncation
   and retention subtract exact file bytes, and successful startup recovery
   rescans actual file sizes before clearing poison.
8. **[enum + code] Source, recovery, and lifecycle closure.** The sole production
   fetch constructs its request from the persisted `JournalStreamState`; the
   response can reach append only through `ValidatedJournalBatch`. Startup takes
   the exclusive data-root lock, opens SQLite in `synchronous=FULL` WAL mode,
   commits the current retention cutoff, and calls archive recovery before
   binding either listener or starting either poller. Recovery fails closed on
   a missing day, short committed file, hash mismatch, malformed entry, or
   unsatisfied boundary. Ordinary polling runs retention before spawning the
   cycle's target tasks and joins every task before another cycle, so retention
   cannot race an append. The only official graceful exits go through
   `supervise`, which stops new work and joins both durability workers; abrupt
   process or host loss is handled by the same startup recovery. The documented
   backup/restore procedure stops and joins the sole process, copies the
   complete data directory from one recovery point to an empty private volume,
   retains the matching key and key identifier separately, and admits traffic
   only after this startup recovery succeeds.

## Evidence anchors

Focused evidence includes
`concatenated_frames_decode_to_exact_source_jsonl`,
`recovery_truncates_orphan_tail_to_committed_hash_boundary`,
`indeterminate_sync_failure_poison_blocks_sibling_admission_until_recovery`,
`stale_registration_after_fetch_rolls_back_frame_and_cursor`,
`shutdown_does_not_detach_an_in_flight_archive_append`, and
`capacity_sibling_waits_for_sqlite_commit_before_contained_return`.

`real_daemon_registers_pulls_persists_and_restarts` covers one ordinary append,
clean shutdown, manually appended orphan-tail recovery, and restart. It does not
integrate crash windows, stale CAS, poison, generation replacement,
incarnation/gap handling, expiry/quarantine, or cross-worker fatal cleanup.

## Residuals

FMan journals are best-effort: bounded source retention, producer drops, and
records lost before a successful fetch are outside the claim. Duplicate records
after crash and deletion after documented archive retention are permitted. An
arbitrary whole-volume rollback is undetectable. A restore must force a new
source incarnation for restored source journals. Multi-active, remote SQLite,
split volumes, shared filesystems, and object-store archives are outside this
local commit model.

## Weakest links

Filesystem and SQLite durability and operator compliance with the supported
lifecycle procedure are axiomatic. The writer/exit closure is an exact-revision
enumeration rather than a compiler-enforced capability boundary, while crash
recovery has focused boundary tests but no process-kill test at every
instruction.
