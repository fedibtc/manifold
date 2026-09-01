# CLAIM-failed-stability-allocation-strands-ecash: Failed stability allocation strands ecash

For every production stability-pool allocation item `I`, **no automatic path in
this binary can make `I` stranded**.  Only a deliberate, authenticated operator
action can, and `abandon_target_client_value` is the only such action: it refuses
unless the peg-in is already claimed, records the abandoned amount and the
operator's reason in `I.failure_json`, and writes an `audit_log` row in the same
transaction that fails the item.

The claim is about **who can strand value**, not about whether stranded value can
be recovered.  It says FLIP never writes off provider value on its own, and that
no write-off happens without an operator choosing it and a durable record of the
choice.  It does not say the value comes back; see
`## What this record does not claim`.

Stranded is exactly this enumerable conjunction:

1. SQLite durably has `allocation_items[I].status = failed`;
2. the unique `wallet_operations` row `W` for `I` and operation type
   `stability_pool_funding` durably has `status = completed`;
3. the peg-in operation id durably recorded in `I.step_json` identifies the
   safe peg-in allocated by the FLIP-owned client for `I`'s exact
   `funding_targets.target_json.federation_id`, and the pinned wallet client has
   reached `DepositStateV2::Claimed` for that operation;
4. at least `I.committed_amount_sats = C` from that claimed funding is again
   spendable mint e-cash in that same FLIP-owned target client's durable
   database, while the stability-pool provider account did not accept `C` for
   the failed deposit operation; and
5. every official automatic path and every authenticated operator action
   exposed by this binary lacks a transition that can either resume a
   stability deposit of that value for `I` or recover/return the value from the
   target client.

The mechanically enumerable domain is: all production post-`Claimed`
stability-worker exits; all allocation-item and wallet-operation terminal
writers; all active-item/operation selectors; both manual retry/cancel guards;
all authenticated Admin wallet/allocation verbs; all target-client wallet and
stability-pool calls; startup recovery and periodic task wiring; and the pinned
stability-pool transaction plus primary-mint input/refund state machines in
Scope.

The adversary may hard-crash the daemon at any instruction, await, SQLite
commit, target-client commit, or dependency-return boundary; restart it
arbitrarily; delay a correct dependency or consensus response; cause an
ordinary stability-pool output rejection; and interleave the periodic worker,
wallet sync, target-client state machines, and authenticated operator calls.
Authenticated setup and operator actions are trusted.  The adversary is not a
malicious Admin or federation, cannot directly edit or roll back either store,
cannot install malicious configuration, and performs no out-of-band target
wallet or stability-pool activity.

## Status

Unverified.

## Assumptions

- **A1 — durable stores and ordinary execution.** Successful SQLite/SQLx and
  Fedimint-client RocksDB commits are atomic and durable, and later reads decode
  them faithfully.  A correct Fedimint federation eventually reports accepted
  or rejected transactions; the client executor resumes durable state machines
  after an ordinary restart.  Hardware loss, database corruption, and endless
  dependency outage are not used.
- **A2 — official deployment boundary.** One official daemon owns a data
  directory and runs the task wiring in `daemon.rs`.  Its target-client root
  secret and RocksDB therefore remain FLIP-owned.  Direct database edits,
  alternate embedders, backup rollback, a second writer, and extracting the
  secret to use an external client are excluded.  A host may remove a
  crash-left daemon lock so restart is possible.
- **A3 — pinned sources.** `flake.nix`, `flake.lock`, the root workspace
  dependencies, and Cargo patches select Fedimint tag `v0.11.2-fedi2`, exact
  revision `a6fa6d83f4bea26d4f51cbf26d305d0b64727e00`. The update from fedi18
  leaves the Fedimint client transaction, mint input/refund, and operation-log
  sources used by this claim unchanged. The daemon manifest and
  `Cargo.lock` select `stability-pool-client` 0.3.0 and common at Fedi
  revision `2f35ea4e3b2516d35b8ed315455718cd3b336758`.  Those sources, rather
  than similarly numbered crates.io code, define the external semantics below.
- **A4 — normal rejection and refund progress.** A non-malicious stability
  federation may ordinarily reject `DepositToProvide`, including because no
  cycle exists or the amount/fee falls outside its consensus limits.  For the
  witness, its primary mint accepts the pinned client's automatic refund and
  finalizes the refund outputs.  The witness chooses ordinary funding reserve
  sufficient that refund fees still leave at least `C` spendable.  This is an
  execution allowed by the pinned code, not a promise that every refund always
  succeeds.
- **A5 — honest identity and balance reporting.** Invite-code parsing and
  federation-id routing are honest, cryptographic operation/transaction ids do
  not collide, and the target client's primary-mint balance counts its durable
  spendable notes.  This bottoms the exact-client and value-ownership parts of
  the claim; the counterexample neither forges identity nor attributes another
  client's value to `I`.
