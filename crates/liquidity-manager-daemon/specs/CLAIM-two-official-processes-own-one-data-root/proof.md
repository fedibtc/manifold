# Current argument

## Argument

**L1 (`code`) — both boot paths acquire the same root lock before serving.**
`run_daemon` creates the data/federation directories, acquires `paths.lock_file`,
then opens state and starts workers/listeners. `run_restore_daemon` creates the
same root and acquires the same lock before its restore listener
([`daemon.rs`](../src/daemon.rs), [`config.rs`](../src/config.rs)).

**L2 (`code`) — contention aborts startup.** `DaemonLock::acquire` maps
`WouldBlock` to an error and retains the locked file for the daemon lifetime.
Thus no second official path reaches a listener after L1 under A1.

## Residual windows

- `sqlite_path` can be configured outside `data_dir`; this claim is root
  ownership, not a guarantee against an operator intentionally overlapping only
  one storage file. This follows the `DaemonPaths` root/SQLite separation in
  `config.rs`.
- Network filesystems or lock-path mutation violate A2/A3 and are excluded by the
  explicit adversary model, not accepted operational residuals.

## Weakest links

1. **L1 (`code`)** — lock acquisition ordering in both boot modes.
2. **L2 (`code`)** — lock lifetime and error path.
3. **A1–A3 (`axiom`)** — OS lock and protected-path identity.
