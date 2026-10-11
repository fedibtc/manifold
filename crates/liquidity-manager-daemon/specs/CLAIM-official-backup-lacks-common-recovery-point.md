# CLAIM-official-backup-lacks-common-recovery-point: Official backup lacks common recovery point

An archive created by FLIP's official `create_backup` operation cannot lack one
common recovery point for SQLite allocation/wallet state and target-Fedimint
client state. The adversary schedules workers, crashes, and backup file reads,
but cannot modify the archive or bypass the official daemon.

## Status

Falsified: official backup quiesces periodic worker passes but leaves retained
target-client background writers and detached/Admin-triggered client opens live
while copying mutable RocksDB files individually
([evidence](CLAIM-official-backup-lacks-common-recovery-point/falsification-live-target-client-writers.md)).

## Assumptions

- **A1 — filesystem reads are not snapshots.** Reading two mutable files without
  a shared snapshot or quiescence barrier may observe different instants.
