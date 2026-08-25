# CLAIM-restore-mode-starts-generation-machinery: Restore mode starts generation machinery

A restore-mode process constructs no `DaemonContext`, and therefore binds no
public RPC listener, spawns none of the six generation background tasks
enumerated in L3, and publishes no advertisement. "Generation background task"
is that enumeration and nothing wider: the staged SQLite pool a restore opens
starts its own connection maintenance, which is a residual below rather than a
contradiction.
Its supervised task set is exactly the restore Admin API server and the shutdown
waiter. Its API route set is exactly six routes: four behind the restore
bearer check, plus an unauthenticated `GET /health` and an unauthenticated
refusing wildcard, and a catch-all fallback under `embedded-operator-ui`. The
adversary can send authenticated and unauthenticated requests to the restore
listener and race them with boot, but cannot modify the executable or the boot
arguments.

**A restore-mode process also performs no outbound effect derived from the data
it was handed.** It opens the staged SQLite and decrypts the restored secret
records to validate the archive — both stated as residuals below — and does
nothing else with them. In particular it opens no socket to any host the restored
configuration names.

This is what makes restore mode safe to point at a data directory an operator is
repairing: nothing derived from that directory runs, and nothing derived from it
is dialled.

## Status

Unverified.

## Assumptions

- **A1 — router and task-set semantics.** Axum serves only the routes merged
  into the router it is given, and a process runs only the tasks its boot path
  spawns.
