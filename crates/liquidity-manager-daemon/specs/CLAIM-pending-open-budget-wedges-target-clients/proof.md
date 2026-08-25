# Current counterexample and work

## Failure

Four unanswering targets fill the separate `MAX_PENDING_OPENS` budget. Every
later `create_or_load` for
a federation not already installed returns `TargetFedimintError::OpensAtCapacity`
until the process restarts.

## Practical impact

A hostile FI needs four endorsements for federations it controls, and needs each
target only to answer the config download and then go quiet. After that, FLIP
allocates to no new federation — including federations belonging to other,
honest FIs. Already-installed clients keep working, so allocations in progress
complete.

Recovery is a daemon restart. Nothing in the Admin surface reclaims a pending
open.

## Current limitation

The separate pending-open budget bounds retained clients, RocksDB handles, and
tasks, but accepts this admission-progress failure. No option below an upstream
change closes both sides of the tradeoff.

`ClientBuilder` has no
`with_task_group`: `build` calls `TaskGroup::new()` itself and the group is
local to that future. `TaskGroup` has no `Drop` implementation, so dropping the
build does not shut its tasks down. And the task is spawned *before* the wait
that hangs — `refresh_common_api_version_static` calls
`task_group.spawn_cancellable("refresh peers api versions", ...)`, handing that
task a `Database` clone, and only then enters the loop that `continue`s
unconditionally while `block_until_ok`. So a timeout at this layer detaches a
live task holding the RocksDB file lock, which is
[`stability-worker-single-target-starvation`](../CLAIM-stability-worker-single-target-starvation.md)
— a worse failure than the one it would repair.

**The trade is fundamental, not a missing idea.** A stuck open permanently
consumes a pending slot, a RocksDB handle, and a task. Any scheme that lets FLIP
keep opening new targets must let those accumulate without bound; any scheme
that bounds them must refuse work.
Moving stuck opens to a separate abandoned set only relocates the choice: bound
that set and the wedge returns later, leave it unbounded and the original
counterexample returns. There is no third position without cancelling the open.

The current mitigation makes the bounded fault visible and attributable:

- `PendingOpen` records when it started. A pending open that passes
  `STUCK_OPEN_REPORT_AFTER` (5 minutes, far above any healthy open) is reported
  at `warn` with its federation id and age, **once**. Once, because the
  condition does not clear without a restart and a message repeated on every
  later open would bury the log for as long as the fault lasts.
- The report fires **before** the capacity branch, so it reaches the operator
  while slots remain rather than after the deployment has already stopped
  opening target clients.
- A capacity refusal names every occupying federation with its age, oldest
  first. "At capacity" alone says the budget is full and not which target filled
  it, and choosing which federation to stop endorsing is the only action
  available before a restart.

**A per-requester pending budget is insufficient.** It would stop one FI taking all
four slots, which is the stated adversary, but four distinct endorsed FIs
would still wedge it, and it needs requester identity threaded through all
thirteen `StabilityPoolBackend` methods, which take a `FundingTargetRecord` that
does not carry one. Reconsider it if the upstream change does not happen and a
measured deployment sees single-requester saturation.

**A bounded pre-join API-version probe does not stop the stated adversary.**
`Client::fetch_common_api_versions`
is public and takes neither a `TaskGroup` nor a `Database`, so FLIP could probe
under its own timeout before `join` and drop it safely. It would prevent the
*accidental* case — a federation that is simply broken — but a hostile FI
chooses when its target goes quiet, so it defeats the probe by answering it. A
mitigation the stated adversary trivially defeats is not worth a
duplicate config download and two more pinned-API dependencies.

**The complete fix is bounded upstream negotiation.** A pinned Fedimint change
that makes the loop terminable closes both the wait and the bounded wedge.
Re-evaluate the local tradeoff on such a pin bump, or when monitoring observes
pending-open saturation in a deployment.

**No metric surface exists.** The reports are log lines. FLIP has no admin verb
or gauge exposing pool occupancy, so an operator alert has to come from log
matching. Adding one is worth doing when FLIP gains a metrics surface, and it is
not worth inventing that surface for this alone.
