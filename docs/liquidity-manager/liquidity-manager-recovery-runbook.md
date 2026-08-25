# FLIP target-client recovery runbook

What an operator does when provider e-cash reaches a target federation's client
and FLIP cannot deposit it into that federation's stability pool.

This is the manual route. It exists because FLIP has no target-client sweep:
returning that value to the provider wallet is a peg-out from the target
federation, with its own send-once fence and settlement evidence, and it is not
built. Until it is, the value is reachable only with Fedimint client tooling
against the database FLIP retains. The absence is tracked in
[`liquidity-manager-open-items.md`](./liquidity-manager-open-items.md).

## When this applies

Use this when a stability-pool item is `action_required` or `failed` and the
target federation will not accept the provider deposit — most plainly, when the
pool rejects provision permanently.

Do **not** use it for an item that is merely slow or retryable. `retry_funding_step`
covers a step that failed for a transient reason, and FLIP resolves an
interrupted submit by itself from the target client's own operation log. Read
[`SPEC-flip-funding-safety`](../../crates/liquidity-manager-daemon/specs/SPEC-flip-funding-safety.md)
before deciding an item is unrecoverable.

## What is and is not reachable

Reachable: e-cash sitting in the target client's **mint** balance — value that
was pegged in for the provider deposit and never deposited. That is the case
this runbook is for.

Not reachable this way: value already inside the stability pool. Withdrawing it
needs the stability-pool module, and the tooling below does not carry that
module. If `inspect_target_client` reports a nonzero
`observed_provided_amount`, that part of the value is in the pool and this
runbook does not return it.

## Step 1 — look before deciding

```
curl -sS -H "Authorization: Bearer $FLIP_ADMIN_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"federation_id":"<federation-id>"}' \
  http://127.0.0.1:8173/admin/v1/inspect_target_client
```

`spendable_balance` is what the client holds in mint notes. It is a client-wide
total, not this item's share, so read it as information rather than as a
settlement. `observed_provided_amount` is what the pool reports for this
provider account.

If the item's deposit may simply be unrecorded rather than absent,
`bind_target_deposit` is the right verb and this runbook is not.

## Step 2 — release the capacity and record the write-off

```
curl -sS -H "Authorization: Bearer $FLIP_ADMIN_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"federation_id":"<federation-id>","reason":"pool rejected provision permanently"}' \
  http://127.0.0.1:8173/admin/v1/abandon_target_client_value
```

This fails the item, releases its reservation, and records the amount left
behind. Without it, one federation that rejects provision consumes provider
capacity permanently: a settled funding send makes both `cancel_allocation` and
`retry_funding_step` refuse, so the item can never complete.

It writes off FLIP's ability to manage funds it already sent. The `reason` is
required and lands in the audit log; write what actually happened.

## Step 3 — release the database lock

The target client's RocksDB is held open by the daemon under a file lock, and
the tooling in step 4 needs it.

```
curl -sS -H "Authorization: Bearer $FLIP_ADMIN_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"federation_id":"<federation-id>"}' \
  http://127.0.0.1:8173/admin/v1/reopen_federation_client
```

`closed: true` means the handle was open and is now closed. The database stays
on disk; that is what makes step 4 possible.

Do this **after** step 2, not before. Once the item is failed, no worker selects
that federation again, so nothing reopens the client underneath the recovery.
Stopping the daemon works too and is the safer choice if anything about the
deployment is uncertain.

## Step 4 — recover the notes with `fedimint-cli`

FLIP's target client database lives at

```
<data-dir>/federations/<federation-id>/client.db
```

`fedimint-cli` reads `<data-dir>/client.db`, so point it at the directory
containing that file:

```
export FLIP_TARGET_DIR=<flip-data-dir>/federations/<federation-id>
fedimint-cli --data-dir "$FLIP_TARGET_DIR" info
fedimint-cli --data-dir "$FLIP_TARGET_DIR" module wallet withdraw \
  --address <your-address> --amount-msat <amount>
```

Three facts make this work, each checked against both sides rather than
assumed:

- **Same file layout.** FLIP writes `client.db` under the per-federation
  directory (`target_fedimint.rs`, `federation_client_db_path`), and
  `fedimint-cli` opens `<data-dir>/client.db`
  (`fedimint-cli/src/lib.rs`, `load_database`).
- **Same secret.** FLIP stores a 12-word BIP39 entropy inside that same
  database with `Client::store_encodable_client_secret` and derives
  `RootSecret::StandardDoubleDerive(Bip39RootSecretStrategy::<12>::to_root_secret(..))`.
  `fedimint-cli` loads and derives identically (`load_or_generate_mnemonic`).
  No key material has to be carried across from FLIP.
- **The missing module is not fatal.** `fedimint-cli` does not register the
  stability-pool module, and the client builder skips a module kind it has no
  initializer for rather than failing
  (`fedimint-client/src/client/builder.rs`: "Module kind of instance not found
  in module gens, skipping"). The mint and wallet modules still load, which is
  what a withdrawal needs.

## Verification status

The three facts above are verified by reading FLIP's code and the pinned
Fedimint sources. **A live rehearsal has not been run**: doing it end to end
needs a real stability-pool federation and an item driven into the abandoned
state, which is the live stability stack rather than a unit test.

Rehearse it against a regtest deployment before relying on it in an incident,
and correct this section with what the rehearsal finds.

## Afterwards

The database stays on disk. Nothing in FLIP deletes it, which is deliberate —
it is the only remaining record of value FLIP has stopped managing. Do not
delete it until the recovery is confirmed and the value is somewhere you
control.
