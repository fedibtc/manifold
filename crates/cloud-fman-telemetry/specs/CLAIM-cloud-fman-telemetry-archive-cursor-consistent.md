# CLAIM-cloud-fman-telemetry-archive-cursor-consistent: Archive progress is cursor-consistent

For every nonempty safe-journal batch returned to the collector, it cannot
durably advance that stream's source cursor unless the exact returned JSONL is
recoverable from the archive at that commit point and after crash recovery,
until documented retention later removes it. A reported source-incarnation change or
same-incarnation continuity gap cannot be silently represented as continuous
history.

## Assumptions

- Exactly one active collector process owns the data root.
- The official binary and its pinned dependencies execute the reviewed code
  without memory corruption or code injection.
- SQLite database, WAL, and archive reside on the same correctly functioning
  persistent volume, and SQLite, filesystem write, `fdatasync`, directory sync,
  rename/truncate, and locking operations satisfy their documented contracts or
  fail detectably.
- The encrypted volume, its key, and backups remain available and protected
  across each supported lifecycle operation.
- A supported backup and restore follows the documented collector procedure:
  it stops and joins the sole process, copies the complete data directory from
  one recovery point, retains the matching key and key identifier separately,
  restores them to a private empty volume, and withholds traffic until startup
  recovery and readiness succeed.
