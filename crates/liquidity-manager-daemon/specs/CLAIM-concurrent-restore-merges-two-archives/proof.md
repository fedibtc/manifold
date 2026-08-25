# Current counterexample and work

## Failure

`restore_backup` (`src/backup.rs`) runs four steps: check the data directory is
empty, stage the archive, check again, move the staged contents in.

Staging is safe. `staging_dir_for` carries the pid and a unique suffix, so two
calls never stage into the same directory.

**The unguarded window is between the second check and the move.** Nothing holds
a lock across them. Two concurrent authenticated calls can both pass the second
check while the data directory is still empty, and then both move.
`move_staged_contents` renames entries into the data directory without a
predicate, so neither move fails on the other's output. The result is one data
root holding files from two archives — a SQLite database from one and Fedimint
client state from the other, or either store split between them.

## Practical impact

An operator repairing a data directory drives restore mode by hand, so the
concurrent case needs either two operators, a retried request whose first attempt
is still running, or a client that resends. None of those is exotic during an
outage, which is exactly when this verb runs.

The merged root is not detected. Checksum verification happens per archive during
staging, before the move, so both archives verify. What lands is two verified
archives interleaved, and the daemon then starts against a data root that no
backup describes.

## Recommended fix

Hold a lock, or take the data directory by an atomic operation that fails for the
second caller. `ensure_restore_target_empty` followed by a rename is a
check-then-act on a shared resource; the check is not what makes it safe.

Note that `move_staged_contents` cannot simply refuse on an existing entry: a
partially-completed move would then leave the second caller a data root it can
neither use nor clean up. The exclusion belongs before the move, not inside it.
