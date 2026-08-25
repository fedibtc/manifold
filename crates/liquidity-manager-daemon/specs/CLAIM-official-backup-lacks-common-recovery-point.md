# CLAIM-official-backup-lacks-common-recovery-point: Official backup lacks common recovery point

An archive created by FLIP's official `create_backup` operation cannot lack one
common recovery point for SQLite allocation/wallet state and target-Fedimint
client state. The adversary schedules workers, crashes, and backup file reads,
but cannot modify the archive or bypass the official daemon.

## Status

Unverified.

## Assumptions

- **A1 — filesystem reads are not snapshots.** Reading two mutable files without
  a shared snapshot or quiescence barrier may observe different instants.
