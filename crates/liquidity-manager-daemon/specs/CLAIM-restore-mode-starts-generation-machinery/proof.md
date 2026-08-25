# Current argument

## What this record does not claim

- **Restore mode does open SQLite.** `restore_backup` extracts the archive into
  a staging directory and `validate_restored_state` opens *that* database to
  ping it, check secret-record decryptability, and validate the restored setup
  ([`backup.rs`](../src/backup.rs)). The staging directory is a sibling of the
  data directory, not a child, and the open is on the staged copy; the data
  directory's own database is never opened in restore mode. Opening the staged
  database is outside this claim's data-root confinement property.
- **Restore mode replaces the data directory.** That is its purpose.
  `restore_backup` requires the target empty, stages, re-checks empty, then moves
  the staged contents in. Confinement here is about what *runs*, not about what
  is written.
- **The static dashboard is served outside the bearer check.** Under the
  `embedded-operator-ui` feature, `with_operator_ui` merges the static asset
  router outside the auth layer, deliberately, so the shell can load before an
  operator enters a token ([`admin.rs`](../src/admin.rs)). Those routes
  carry assets, not daemon state. The five-route count is the API surface.
- **Bearer authorization is a separate property.**
  `unauthenticated-admin-reaches-privileged-effect` owns it. This record is
  process and route confinement only.

## Argument

**L1 (`code`) — the two boot paths are mutually exclusive and each is fenced.**
`main` dispatches on `args.mode` and returns from `run_restore_daemon` for
`Restore` before it constructs a `PeerBadgeVerifier` at all
([`main.rs`](../src/main.rs)). Each entry point re-asserts its own mode:
`run_daemon` fails on anything but `Normal`, and `run_restore_daemon` fails on
anything but `Restore` ([`daemon.rs`](../src/daemon.rs)). No path reaches one
from the other.

**L2 (`enum`) — `DaemonContext` has one production construction site, and it
sits behind `run_daemon`.** Enumerated by construction expression rather than by
caller, so a new caller in any module is in scope by construction. Searching the
crate for `DaemonContext {` returns six occurrences: one struct definition and
three `impl` headers, which construct nothing, and two struct-literal
expressions — `build_generation` in [`daemon.rs`](../src/daemon.rs) and
`production_test_context` in [`tests/support.rs`](../tests/support.rs).
`build_generation` is called only from `run_generations`, which is called only
from `run_daemon`. With L1, no restore-mode process reaches it.

`test_support` is worth stating precisely. [`lib.rs`](../src/lib.rs) declares it
`#[cfg(test)] pub mod test_support;`, so it is compiled out of every non-test
build and its `DaemonContext` constructor does not exist in the shipped binary.

**L3 (`enum`) — every listener, worker, and publisher named in the claim takes a
`DaemonContext`, so L2 excludes all of them.** Enumerated from the spawn sites
rather than from the task modules. `build_generation` spawns five background
tasks and one phase watcher, each taking `context.clone()`:
`holder_authorization::run_initial_read_task`,
`funds_admin::run_operation_sync_task`,
`gateway::run_gateway_observation_task`,
`gateway::run_gateway_allocation_task`, and
`stability_allocation::run_stability_pool_allocation_task`. `serve_generation`
spawns `public::serve(context.clone())`. The advertisement publisher is
deliberately not spawned with the others — `public::serve` spawns
`advertisement::run_publisher_task(context.clone())` once the Iroh bind has
settled, so it is downstream of the public listener and excluded twice over
([`daemon.rs`](../src/daemon.rs), [`public.rs`](../src/public.rs)).
A `DaemonContext` is the only argument any of them takes, so none can be
constructed without one.

**One further spawn site, named because this lemma claims completeness.**
`serve_generation` also spawns an inline generation-shutdown closure
(`daemon.rs`). It takes a `CancellationToken` rather than a context, so it does
not weaken the lemma — but a spawn-site enumeration that silently omits a spawn
site is the shape that has failed elsewhere in this tree, so it is listed rather
than filtered.

**L4 (`code`) — the restore-mode supervised task set is exactly two.**
`run_restore_daemon` creates the data directory, acquires `DaemonLock`,
constructs `RestoreAdminContext` — three fields: boot arguments, derived paths,
and a cancellation token — and passes `supervise_tasks` a two-element vector:
`admin::serve_restore` and `wait_for_shutdown_signal`
([`daemon.rs`](../src/daemon.rs)). Restore mode constructs no `TaskTracker`,
which is the handle every generation background task is spawned onto. Axum's own
per-connection tasks are not counted here and are not daemon work.

**L5 (`code` + `type`) — restore mode serves five API handlers of its own, plus
a wildcard that refuses, and a normal handler cannot join them.** `restore_app`
builds one unauthenticated route, `GET /health`, and merges four behind
`require_restore_auth`: `GET /admin/health`, `POST /admin/v1/get_health`,
`POST /admin/v1/inspect_backup`, and `POST /admin/v1/restore_backup`
([`admin.rs`](../src/admin.rs)).

The two routers carry different state types — `app` takes `DaemonShell`,
`restore_app` takes `RestoreAdminContext` — so the compiler preserves the
boundary as handlers change. Every normal handler reaches its
state through `State<DaemonShell>` or through `Live`, whose
`FromRequestParts` implementation is written against `DaemonShell` and calls
`shell.current()`. Merging any of them into `restore_app` does not compile.
Restore mode never constructs a `DaemonShell`, so no such handler can be served
even if one were registered.

## Weakest links

1. **L3 (`enum`)** — the completeness claim that every listener, worker, and
   publisher takes a `DaemonContext`. A task that needs no context could be
   spawned in restore mode and this argument would not see it.
2. **L2 (`enum`)** — the construction-site enumeration. Attack a second path to a
   `DaemonContext` that the `DaemonContext {` expression search does not reach:
   a `Clone` handed across a boundary, or a constructor function that returns one
   without a struct literal at its own site. `test_support` is the standing
   example of a real second constructor, and it is excluded by conditional
   compilation (`#[cfg(test)]` in `lib.rs`) rather than by its callers — so it is
   not in a shipped binary at all.
3. **L5 (`code`)** — the route inventory, including the feature-gated merge.
4. **L4 (`code`)** — the task inventory of `run_restore_daemon`.
5. **A1 (`axiom`)** — framework router and task execution.
