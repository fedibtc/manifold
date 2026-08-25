# Proof: Payment and fee value moves use only client authority

This proof supports
[CLAIM-fleet-manager-value-moves-use-client-authority](../CLAIM-fleet-manager-value-moves-use-client-authority.md).

## Scope and model

The derivation reads the production intent traits and callers in `fman-core`, the
wallet construction/open/recovery path, accepted-payment claim code,
guardian-fee collection, durable payout worker/store, native Lightning start,
operation lookup/status/await, and their focused tests in `fman-fedimint`. The
source lint `authority_surface_tests` pins counts for the known direct client-call
spellings and forbids child/guardian authority names in the wallet implementation
and narrow core intent boundaries.

The adversary is a source regression and may also drive every public input,
crash/race point, and dependency outcome allowed by the claim. The quantified
predicate is the authority presented by each Fedimint request, not economic
success, amount conservation, final recipient identity, or whether earlier
read-only discovery queried the guardian process.

## Assumption boundary

A1 grants the Rust call graph. A2 is the explicit dependency boundary: this proof
does not re-prove Fedimint module/state-machine authority. A3 makes absence of a
child-data path meaningful. No immediate assumption grants use of `SeatApiAuth`,
a child/admin mutation, or direct child database access.

## Argument

1. **Accepted setup-payment claim work carries no seat capability (`enum`, `code`,
   `test`).** The production producers are startup continuation for unfinished
   durable payment records, accepted `CreateSeat` replay before or after lock,
   and fresh accepted `CreateSeat`. The continuation captures only accepted claim
   evidence, fleet SQLite, `EcashWallet`, and task guards. `ClaimWorker` prepares
   the payment-scope client, starts or recovers exactly one mint-v1 reissue or
   mint-v2 receive, and awaits that operation. Signatures accept no `Seat`,
   `SeatKeys`, `FedimintApi`, local address/path, or `api_auth`.

2. **Guardian collection selects one ordinary account client (`enum`, `code`,
   `test`).** Production `CollectGuardianFees` is the sole collection entry and
   delegates through `GuardianFeeVault::collect`. Core supplies the public invite,
   seat id, and mnemonic/seat-derived `GuardianFeeAccountKey`. The sole production
   vault implementation opens `ClientScope::Guardian`, installs that key as the
   stability-pool `BtcDepositor`, and rejects a module carrying a different
   account. Its complete submissions are `withdraw_idle_balance` and
   all-balance `withdraw`; subscriptions only await those exact operation ids.
   Deferred primary outputs and later unlock/claim state machines remain ordinary
   module/account work under A2.

3. **Durable payout entries preserve the client scope (`enum`, `schema`, `code`,
   `test`).** `SweepPaymentFees` and `SweepGuardianFees` create/replay one durable
   job; `PayoutStatus` and `AwaitPayout` reconcile and observe it. The job fixes a
   payment federation or guardian federation/seat/public-invite scope before any
   wallet call. `WalletPayoutNative` maps those variants only to
   `ClientScope::Payment` or `ClientScope::Guardian`, the latter with the derived
   guardian account key. Status/await and guardian resume use the stored scope;
   they do not reacquire a child capability after decommission.

4. **Native Lightning v2/v1 starts use only client modules (`enum`, `code`,
   `test`).** Under per-scope fences, replay first enumerates the selected ordinary
   client's operation log. Only no matching request permits `start_payout`.
   Lightning v2 calls the client's `LightningClientModule::send`; v1 calls the
   client's `pay_bolt11_invoice`. Their reachable authority is the client handle,
   module database, client/module keys and notes, chosen public gateway, invoice,
   and request metadata. Neither start signature or module contains a seat,
   `SeatApiAuth`, child RPC client, or child database handle.

5. **Replay and observation cannot start or elevate (`type`, `code`, `test`).**
   `payout_for_request` and `payout_status` read the ordinary client operation log.
   The `PayoutObservation` trait type-confines generic orchestration to `status`
   and `await_terminal` for one exact operation id. Its production implementation
   holds a full ordinary-client handle, but its code calls only operation-log
   status, v1 `await_outgoing_payment`, or v2
   `await_final_send_operation_state`; it accepts no destination, invoice, or
   child handle. The mock-observer test proves repeated generic status/await
   orchestration adds no operation, while production confinement remains a code
   reading. Other tests retain refund/activity distinctions.

6. **Refund/reclaim and lazy recovery remain inside the same client (`code`,
   `test`, `assumption`).** Native Lightning funding, change, cancellation,
   refund/reclaim, stability-pool deferred outputs, and accepted-payment mint
   outputs are dependency state machines rooted in the already selected client;
   A2 classifies their authority. Opening a retained payment or guardian scope
   validates its prefix mapping, opens or resumes recovery against that partition,
   waits for required mint recovery, reopens, and waits for recovered output state
   machines before publishing the handle. The wallet root is separate from child
   storage under A3. Recovery receives no seat auth or child path and cannot change
   a payment scope into a guardian/admin client.

7. **Every client open uses the same scope/recovery boundary (`code`,
   `assumption`).** Every production opener enters `Wallet::join`,
   `Wallet::client`, or `guardian_fee_client`. Join and guardian opens converge on
   `join_inner`; retained payment opens go directly through
   `open_initialized_scope`. Both routes select an explicit `ClientScope` and
   apply lemma 6's validation/recovery before returning a handle. Thus any trigger
   that lazily opens a client and resumes its durable executor—including the
   setup-payment policy joiner, quote/refund preparation, payment verification,
   payment-federation status/drain, guardian collection, and guardian
   status/history/balance/drain—has the same A2 authority classification. Payment
   callers supply `ClientScope::Payment`. Guardian collection and reads obtain the
   public invite from the cached seat report and supply the mnemonic/seat-derived
   `ClientScope::Guardian`. `Seat::report` checks final-data existence and watched
   child state; it performs no child API call or mutation and carries no
   transaction, amount, destination, or account. Policy proposal's intentional
   authenticated `meta_submit` has no edge to a wallet-client open or value-moving
   start.

The entry inventory in lemmas 1--3 covers explicit accepted-payment claim,
collection, and both payout scopes. Lemmas 4--6 cover reachable submissions,
replay/observation, downstream state machines, and recovery. Lemma 7 proves
trigger-independent confinement for implicit resumption from any production
client open. Under A1--A3, none can present guardian/admin or direct child-storage
authority for a value move. ∎

## Residuals

- Correct fee accrual, amount conservation, payout destination/payee binding,
  Lightning settlement, and the public federation's acceptance of an otherwise
  ordinary client request are separate monetary-correctness properties.
- A paid-refusal refund transaction is returned to the FI for submission; FMan
  has no production caller of the wallet's submission helper, so it is outside
  this production-call-site domain.
- Formation and later fee-policy votes intentionally use the guardian's
  authenticated meta-module authority. They decide consensus policy but are not
  value-moving requests in this claim's enumerated domain.
- Host compromise, a compromised child, operator access to seat files, and
  dependency behavior contrary to A2 can bypass these Rust authority boundaries.

## Weakest links

1. A2 is the dependency trust boundary. A Fedimint client/module change that uses
   server/admin authority internally requires a new premise or falsifies the
   argument.
2. The source lint catches forbidden authority vocabulary in the current narrow
   boundaries and changes to the known direct call vocabulary. A new indirect
   value mover, alias, or wrapper can evade spelling-based checks and still
   requires hostile enumeration review.
3. Cached seat-report discovery and later wallet work remain separated by code
   and trait boundaries rather than a type that makes every child mutation
   impossible.
