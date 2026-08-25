# Current argument

## Argument

**One pass is the unit, and that is the load-bearing choice.** A pass is what
touches both stores together: it reads an item from SQLite, acts on a Fedimint
client, and writes the result back. A SQLite lock would order the first and last
of those and say nothing about the middle.

**L2 (`code`) — recovery state spans the independently copied stores.**
Allocation rows and wallet-operation identities live in SQLite, while stability
client state lives under the target-client directory. Startup recovery resumes
active allocation work after opening those stores
([`allocation_store.rs`](../src/allocation_store.rs),
[`stability_allocation.rs`](../src/stability_allocation.rs),
[`target_fedimint.rs`](../src/target_fedimint.rs)).

Under the barrier, `stage_payload` writes the whole payload into a staging
directory: SQLite through `VACUUM INTO`, which produces a complete
self-contained database containing every committed transaction, and every other
file by copy. The `-wal` and `-shm` sidecars are skipped, matched by suffix on
the database's own file name, so the archive cannot carry a half-checkpointed
pair. The barrier is then released and the archive is compressed from the
staging copy, which nothing is writing.

**Two writers are deliberately outside the barrier**, and A1 does not reach
them. Admin verbs and allocation admission write SQLite but not the client
directories, and `VACUUM INTO` takes its own consistent read snapshot, so such a
write lands wholly inside the snapshot or wholly outside it. Neither can tear a
client directory, because neither touches one.

**Conclusion.** By L1 no worker pass runs while the payload is copied; by L2 the
recovery state that spans the two stores is exactly what those passes move; by L3
both stores are copied inside that window and the window is recorded. So the
archive has one common recovery point, and names it.

## Residual windows

- This is about official archives made while normal work runs, not an operator's
  separately quiesced filesystem snapshot; `SPEC-flip-admin-api` identifies the
  official API-created archive as the claimed domain.
- Archive confidentiality and malformed-archive resource exhaustion are separate
  properties; they do not repair the missing common point.

## Weakest links

1. **L1's enumeration of what writes the client directories.** The barrier gates
   `run_interval_task` and nothing else. Any future writer of `federations/`
   outside a periodic worker pass — a request-path client open that writes, a
   spawned task, an admin verb that reaches a target client — is outside the
   barrier and breaks the claim without touching a lemma. Regenerate this from
   `target_fedimint` and `federations_dir` outward, not from `run_interval_task`'s
   callers.
2. **L3's `VACUUM INTO` premise.** That the copy is complete without the
   write-ahead log, and that the suffix match skips exactly the two sidecars and
   nothing else.
3. **L2 (`code`)** — that the recovery state spanning both stores is exactly what
   a worker pass moves.
4. **A1 (`axiom`)** — filesystem snapshot semantics.
