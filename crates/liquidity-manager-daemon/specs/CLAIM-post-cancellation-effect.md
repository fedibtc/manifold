# CLAIM-post-cancellation-effect: Post cancellation effect

For every durable allocation-item row and wallet-operation row created by the
official daemon, once its status commits as `cancelled`, and likewise once an
item commits any terminal status (`completed`, `failed`, or `cancelled`) or a
wallet operation commits any terminal status (`completed`, `failed`, or
`cancelled`), no invocation that started from an older snapshot may:

1. invoke a later irreversible `send_onchain` or `deposit_to_provide` money
   effect for that item/operation; or
2. overwrite that terminal status, its failure/completion evidence, or its
   source-specific step meaning with a nonterminal or different terminal
   outcome.

The enumerable effect domain is the one production `send_onchain` call in
`GatewaydFundsWallet::submit_prepared_withdrawal`, reached from the shared
allocation funding path and the operator-withdrawal path, and the one
production `deposit_to_provide` call in
`FedimintStabilityPoolBackend::submit_deposit_to_provide`, reached from the
stability-pool worker. The enumerable concurrency domain is both allocation
workers, the manual retry and cancel verbs, both wallet-sync update paths
(backend sync and chain evidence), and every item, item-step, and wallet-status
writer listed in E1 below.

The required fence immediately before each irreversible call is a successful
predecessor-state compare-and-set in the durable database. It must include the
relevant item/operation predecessor status (and the expected stability step
before `deposit_to_provide`), and zero affected rows must fence the call.
Terminal mutations must likewise use a checked predecessor CAS or an
indivisible transaction-time check and mutation that rejects an illegal
predecessor. Merely loading `pending`/`running` earlier, or issuing an unchecked
update immediately before a call, does not satisfy the claim.

The adversary may schedule an authorized cancel or retry concurrently at any
`await`, delay or reorder dependency responses, return success or error after
an external money effect occurred, and crash/restart the daemon before or after
any database commit or external request. It may supply hostile remote service
behavior but cannot directly write the database or bypass safe Rust. The claim
must hold even under the daemon's intended single-process deployment; it does
not rely on two daemon instances.

## Status

Unverified.

## Assumptions

- **A1 — SQLite/SQLx semantics and store integrity:** for process crashes, a
  committed SQLite transaction is atomic and durable; concurrent write
  transactions are serialized according to SQLite semantics; an `UPDATE`
  without a status predicate may update a row regardless of its current
  status; and SQLx's affected-row count faithfully reports whether a
  conditional update matched. No process other than the official daemon writes
  these tables, and there is no pre-existing corruption. Sudden power/storage
  failure is excluded: the configured WAL database uses
  `synchronous=NORMAL`, so this record does not derive power-loss durability
  from source.
- **A2 — external-effect boundary:** invoking gatewayd `send_onchain` may
  irreversibly broadcast value, and invoking Fedimint
  `deposit_to_provide` may irreversibly spend/deposit value. Neither backend
  transaction is rolled back by a later local SQLite write or process crash.
  A response may be delayed or lost after the effect, and these APIs provide
  no transaction encompassing the local SQLite predecessor check and the
  remote effect.
- **A3 — execution model:** Tokio tasks are cooperatively scheduled at
  genuinely pending awaits and between task polls; an immediately-ready future
  need not yield. A completed SQL/network result may wait to be polled, and a
  future that retains a previously loaded Rust value continues with that value
  unless code reloads it. Process death stops local execution but does not
  revoke already submitted remote effects. Safe Rust, the official binary, and
  ordinary operating-system/runtime behavior are trusted.
- **A4 — observation ordering:** a configured chain observer may return
  transaction/address evidence, including the configured confirmation depth,
  after the transaction is visible but before a delayed `send_onchain`
  response is delivered. This axiom is used only by E5's chain/delayed-response
  counterexamples; C1 does not need it.
