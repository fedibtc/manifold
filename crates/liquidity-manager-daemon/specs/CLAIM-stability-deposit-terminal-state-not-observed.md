# CLAIM-stability-deposit-terminal-state-not-observed: Stability deposit terminal state not observed

Let `I` range over active, production-format stability-pool `allocation_items`
admitted as starting states for this conditional suffix property, and let `O`
be the exact operation id durably decoded from `I.step_json` as
`sp_deposit_operation_id`. It cannot be the case that:

1. `I` remains active (`status` is `pending` or `running`) with that same `O`
   and a local nonterminal deposit status (`initiated` or `tx_accepted`);
2. `O` has a durable upstream terminal predicate defined below; and
3. an unbounded fair sequence of production stability-worker invocations, each
   returning `Ok` and containing unboundedly many **responsive** invocations,
   leaves (1) true after every invocation.

A **responsive** invocation is one whose `observe_deposit(I, O)` drain reaches
stream end within the budget the worker allows it. A6 supplies their recurrence;
without that premise the property is unsatisfiable by any bounded observer, for
the reason recorded there.

This claim admits the durable state satisfying (1)-(2); it does not assert that
a clean current-revision setup reaches that antecedent. No direct database edit
is an action in a quantified continuation. The enumerable domains are every
production `deposit_to_provide` and
`subscribe_deposit_operation` call, every constructor/variant/exit of the
returned `UpdateStreamOrOutcome`, every operation-outcome cache writer, every
local deposit-step and item-terminal writer, and every startup/periodic/manual
executor in Scope. The adversary may hard-crash or ordinarily restart at any
instruction, await, or durable-commit boundary; delay dependencies and worker
polls; make the 500 ms poll time out; and choose all ordinary worker
interleavings. Authenticated Admin/operator setup and actions are trusted. The
adversary cannot maliciously use them, edit either database, supply malicious
configuration, perform out-of-band wallet activity, corrupt memory, or run a
second daemon for the same data directory.

The exact durable predicates are:

- `LocalNonterminal(I,O)`: SQLite durably contains an active item whose decoded
  stability step names `O` and whose `sp_deposit_status` is one of those two
  strings.
- `Accepted(O)`: the pinned Fedimint client database durably contains
  `TxSubmissionStatesSM { operation_id: O, state: Accepted(txid) }` for the
  transaction named by `O`'s stability-pool operation-log metadata.
- `Rejected(O,r)`: that durable state is instead `Rejected(txid,r)`.
- `Successful(O)`: `Accepted(O)` and either the metadata's `change_outpoints`
  is empty, or every named primary-module output has durably reached the final
  successful state which makes `await_primary_module_outputs` return `Ok`.
- `OutputFailed(O,e)`: `Accepted(O)` and a named primary-module output has
  durably reached the final error state which makes that await return `Err(e)`.
- `UpstreamTerminal(O)` is `Rejected(O,_)`, `Successful(O)`, or
  `OutputFailed(O,_)`. On complete consumption these respectively make the
  pinned high-level stream end after `TxRejected`, `Success`, or
  `PrimaryOutputError`. `Accepted` is separately durable and precedes the last
  two; transaction rejection is the mutually exclusive terminal alternative.
- `CachedTerminal(O,t)`: the operation-log entry durably has outcome `t`. This
  is deliberately distinct from `UpstreamTerminal`: it is a derived cache
  written only after a high-level update stream is polled to exhaustion.

## Status

Unverified.

## Assumptions

- **A1 — durable-store semantics.** Committed SQLite and Fedimint client
  transactions are atomic, durable, and faithfully decoded after restart.
  Persisted Fedimint executor terminal states are replayed to a new notifier
  subscription. The host can remove a hard-crash-left daemon lock so restart is
  possible.
- **A2 — official deployment.** The official daemon wiring has one periodic
  stability worker per data directory. Its target Fedimint client at the
  federation directory is the client that owns `O`; no alternate embedder or
  second process mutates the stores. Authenticated operator cancellation or
  retry is correct when requested, but no such request is required to rescue an
  otherwise stuck item.
- **A3 — pinned dependency identity.** The daemon inherits
  `stability-pool-client` 0.3.0 from the root workspace, where client, common,
  and server all select public Fedi revision
  `2f35ea4e3b2516d35b8ed315455718cd3b336758`. The Nix `fedi` input selects the
  same revision for the stability-enabled test server. Workspace patches source
  Fedimint 0.11.1 from `.nix-deps/fedimint`; `flake.nix` names
  `v0.11.2+fedi` and `flake.lock` fixes it at
  `01a203d82f1ac5796645febc8629de224ab59cf6`. Those immutable sources are in
  Scope, rather than assumed library contracts.
- **A4 — fair successful polling.** For the liveness counterexample, the daemon
  can run forever and the scheduler eventually starts every ten-second tick.
  Store/client opening and SQLite writes succeed. This does not assume a prompt
  federation after a stream's initial update: dependency delay and poll exits
  are enumerated separately.
- **A5 — conditional starting state.** The requested property begins once exact
  `O` is durable. L6 may therefore admit that active SQLite state together with
  a completed wallet operation and a cached `Claimed` peg-in outcome, without
  asserting a current clean-start path to it. This is a transition-system
  precondition, not a database mutation in the continuation.
- **A6 — recurring dependency responsiveness.** Responsive invocations recur:
  the target federation answers a fresh subscription inside the worker's drain
  budget unboundedly often. A4 deliberately declines to assume a prompt
  federation after a stream's initial update, and that is right for enumerating
  delay exits — but without *some* recurrence premise the property is
  unsatisfiable rather than merely hard. Every drain is time-bounded, so an
  adversary permitted arbitrary finite delay outlasts every budget and leaves
  the drain restarting from its first update forever, for any implementation
  that bounds its waiting. Removing the bound instead is excluded by
  [`stability-worker-single-target-starvation`](CLAIM-stability-worker-single-target-starvation.md).
  A6 separates "the dependency is down" from "the observer does not make
  progress", and leaves the claim falsifiable by the real defect: a drain that
  completes and still leaves (1) true.
