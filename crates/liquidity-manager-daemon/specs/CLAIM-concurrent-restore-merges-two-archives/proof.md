# Proof for CLAIM-concurrent-restore-merges-two-archives

## Scope

This proof concerns
[CLAIM-concurrent-restore-merges-two-archives](../CLAIM-concurrent-restore-merges-two-archives.md)
in restore mode. It reads [backup and restore](../../src/backup.rs), the
[restore-mode router and handler](../../src/admin.rs),
[restore-mode boot](../../src/daemon.rs), and the
[restore-target regression](../../tests/backup.rs).

The normal-mode live restore is a separate path with its own exclusion, and
[CLAIM-fresh-request-id-repeated-funding](../CLAIM-fresh-request-id-repeated-funding.md)
covers its acceptance fence. `DaemonMode` is a boot argument, so one process
serves one of the two paths and never both.

## Model and assumptions

A restore applies one staged archive to one data directory. Two restores merge
when the data directory commits entries from two different archives. The
assumption of
[CLAIM-concurrent-restore-merges-two-archives](../CLAIM-concurrent-restore-merges-two-archives.md)
is an axiom here.

## Argument

1. **Restore mode reaches the data directory through one function (code).**
   `restore_backup` is private to `backup.rs`, and its only caller is
   `RestoreTarget::restore`. The restore-mode router binds
   `/admin/v1/restore_backup` to `restore_restore_backup`, which goes through
   `RestoreAdminContext::restore_target`. The rest of `restore_app` serves
   `get_health`, `inspect_backup`, the unauthenticated `/health`, and a
   catch-all that reports the verb unavailable. `inspect_backup` reads an
   archive and does not write the data directory, so no other restore-mode
   route, worker, or boot step commits entries into it.

2. **One target admits one restore at a time (code).** `RestoreTarget` holds a
   `tokio::sync::Mutex`. `begin` takes it with `try_lock_owned` and returns a
   `RestoreTargetGuard` that releases on drop. `restore` holds that guard across
   the whole of `restore_backup`: the first empty-directory check, staging, the
   second check, and `move_staged_contents`. A caller that cannot take the
   target receives `unavailable` and runs no part of that sequence, so it reads
   no archive and creates no staging directory.

3. **One process holds one target (code).** `run_restore_daemon` builds one
   `RestoreAdminContext` carrying one `RestoreTarget` and hands it to
   `serve_restore`. Axum clones the context per request, and `RestoreTarget`
   clones over an `Arc`, so every request shares one mutex.

4. **A second process cannot reach the same data directory (code).**
   `run_restore_daemon` acquires `DaemonLock` on `data_dir/flip.lock` before it
   builds that context, and holds it for the process lifetime.
   `DaemonLock::acquire` takes an exclusive file lock and fails when another
   process holds it. Concurrent calls into one process are therefore the whole
   domain point 2 has to cover.

5. **The move is the only writer of committed entries, and it runs under the
   guard (code).** `move_staged_contents` renames staged entries into the data
   directory without a predicate, so it cannot reject an interleaved peer by
   itself. By point 2 there is no interleaved peer: at most one caller sits
   between the checks and the move at any instant. The exclusion has to be
   there rather than inside the move, because a move that refused partway would
   leave a data root that is neither usable nor cleanly removable.

6. **The property is pinned by a test.**
   `a_restore_cannot_land_while_another_holds_the_target` builds two archives
   carrying distinct marker files, holds the target, and requires the refused
   restore to report `Unavailable` and to leave its marker out of the data
   directory. It then releases the target, restores the other archive, and
   requires the data directory to carry that archive's marker and not the
   other. A `restore` that stops consulting the target fails it: the refused
   restore succeeds and its marker lands beside the first.

## Residuals

The target is process-local state, and it does not survive a crash. It does not
need to. A restore-mode process that dies during the move leaves a partly
filled data directory, and the next process refuses it at the first
`ensure_restore_target_empty`. Recovering from that is an operator step rather
than an interleaving.

`RestoreTarget` rejects rather than queues, so a client that retries a refused
restore controls the retry. Nothing here bounds how often it may do so.
