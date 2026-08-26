# Proof: Guardian-fee revenue is completely accounted

This proof supports
[CLAIM-fleet-manager-guardian-fee-revenue-accounted](../CLAIM-fleet-manager-guardian-fee-revenue-accounted.md).
The current implementation does not establish the full conjunctive claim. The
three counterexamples below survived independent hostile verification; the
remaining sections record the freshly derived current paths rather than treating
surviving subarguments as a pass.

## Scope and model

The derivation reads guardian-code validation, formation endpoint proofs,
metadata proposal/read paths, account derivation and collection, durable payout
storage and orchestration, native Lightning v1/v2 start and observation, wallet
recovery, the SQLite schema, focused tests, manifests and dependency pins, and
the governing guardian-fee requirement/design records.

The adversary and trusted boundary are exactly those in the claim. Quantification
covers every production whole-metadata submit, accepted setup-payment claim,
guardian-fee collection operation, and payment/guardian payout job, including
crashes at either durable payout commit, replay, status/await observation,
dependency refund or reclaim, and first-use client recovery.

## Assumption boundary

All immediate assumptions are granted. The linked authority claim supplies
ordinary-client authority for accepted-payment claim and collection. The direct
payout-authority premise supplies the corresponding boundary for native start,
replay, observation, refund/reclaim, and lazy recovery; neither premise supplies
account or final-recipient binding. A1 supplies durable commit and operator
ownership, A3 supplies the external payer parser, and A4 supplies the pinned
clients' stated operation-log and downstream state-machine semantics. None trusts
the remote LNURL service or changes the current DKG wire format.

## Current counterexamples

1. **Bare `StartDKG` codes (`code`, `test`).** `GuardianCode` is explicitly a
   bare upstream base32 `PeerSetupCode`. `GetDkgCode` encodes only that type, and
   `validate_dkg_codes` decodes every peer value as that type, recomputing only
   this seat's code before `DkgCodeSet::validate` checks count, uniqueness, and
   own-code presence. Arbitrary distinct bare peer fixtures successfully start
   DKG in focused tests. There is no account/FMan envelope, endpoint signature,
   or persisted account transcript, so clauses 1 and the DKG-transcript wording
   in clause 2 fail. See
   [falsification-bare-dkg-codes.md](falsification-bare-dkg-codes.md).

2. **Joint hostile directory copy-forward (`code`).** Formation admission verifies
   endpoint proofs and requires the local peer entry to name this FMan identity.
   Consensus metadata does not retain those proofs. After a hostile threshold
   installs an internally matching directory and payer-valid recipient vector,
   `validate_carried_guardian_fee_policy` reruns only
   `FmanSeatBindings::verify_for_federation` and split validation. Those checks
   verify each attestation under its claimed FMan key but do not repeat endpoint
   proof or local identity/account binding. An unrelated generic field update can
   therefore carry the hostile object into this guardian's vote, contrary to the
   claim's explicit no-copy promise. See
   [falsification-hostile-directory-copy-forward.md](falsification-hostile-directory-copy-forward.md).

3. **LNURL payee substitution (`code`).** The payout job immutably binds the
   operator-configured LNURL/Lightning-address string, but `lnurl_pay` follows the
   remote service's callback and accepts any correct-amount BOLT11 invoice. Native
   metadata and replay bind only the original string; they do not prove that the
   invoice payee belongs to it. A compromised configured service can return an
   adversary's invoice while all immediate assumptions hold. See
   [falsification-lnurl-payee-substitution.md](falsification-lnurl-payee-substitution.md).

Any one witness falsifies this conjunctive claim.

## Surviving current-path derivation

1. **Formation's actual post-DKG account binding (`code`, `test`).** After final
   config exists, each seat signs its peer attestation and account and signs a
   `SeatEndpointProof` with the configured endpoint key. `ProposeFormationMeta`
   verifies all endpoint proofs, requires this peer's entry to name this FMan,
   validates the directory against final config, and derives the initial
   directory/fee-recipient target. This protects the ordinary formation path, but
   occurs after DKG and therefore cannot establish clause 1.

2. **Accepted role split is unique and fail-closed (`code`, `test`).** The wire
   parser enforces the full A3 entry, canonical sort, account/id equality,
   uniqueness, positive/non-overflowing weights, count, version, and unknown-field
   rules. `canonical_proposal` requires exactly one distinct FI at weight four,
   every distinct guardian at weight one, and the Guardian Verification Fee at
   weight one. FI/guardian, FI/Guardian Verification Fee, guardian/Guardian
   Verification Fee, duplicate, and purported combined FI-and-guardian
   weight-five entries fail closed. The FMan checks shape and the FI role
   supplied the account; ownership of the FI account itself is not established
   by an immediate assumption or this code.

3. **The two submit semantics (`enum`, `code`, `test`).** Production has one
   `.meta_submit` call below an occurrence-bound target pin, but two semantic
   admission paths. Formation independently verifies endpoint proofs and builds
   all three formation fields before entering the shared target. Generic
   maintenance reads the whole object and runs
   `validate_carried_guardian_fee_policy` before entering the target. Partial fee
   keys, invalid parser shapes, noncanonical/incorrect role splits against an
   honest directory, stale bases, and absent production Guardian Verification
   Fee accounts fail closed.
   The hostile-directory witness identifies the missing generic recheck.

4. **Current reporting separates parser validity and policy match (`code`,
   `test`).** `fee_policy_from_meta` derives `configured` and `our_share` only
   after the strict two-key parser accepts the value and compares a full-account-
   derived id. `share_matches_policy` separately treats absent recipients as an
   allowed no-fee policy. Admin output exposes `configured`,
   `share_matches_policy`, and weights; it has no `pays_us` field. The stale claim
   vocabulary must not be mistaken for a verified current predicate.

5. **Collection uses the mnemonic/seat account (`code`, `test`, `claim`).** Core
   derives `GuardianFeeAccountKey` from the fleet mnemonic and seat id on every
   call. The sole vault opens a public-invite `ClientScope::Guardian`, and
   `signing_account` rejects a stability-pool module whose carried account differs
   from that key. Collection submits only an idle claim and all-balance unlock for
   that account, then awaits their exact operation ids. The imported authority
   premise and A4 bound the dependency state machines.

6. **Durable payout job binding (`schema`, `code`, `test`).** The owner-only admin
   path stores one nonempty bounded payout destination. `PayoutWorker::start_job`
   reads it only while creating a request; `payout_jobs` persists request id,
   exact payment or guardian wallet scope, retained public guardian invite, and
   destination before calling the wallet. Triggers make those inputs immutable
   and jobs undeletable. Existing requests use the snapshot despite configuration
   changes. Store and worker tests pin mutation refusal, concurrent replay,
   restart, and lost responses. This establishes string binding, not ultimate
   invoice-payee binding.

7. **Native v2/v1 start and replay (`code`, `test`).** The worker selects only the
   stored payment scope or mnemonic/seat-derived guardian scope. Nested per-scope
   fences search operation metadata for the exact request id and stored destination
   before start. Lightning v2 `send` and v1 `pay_bolt11_invoice` receive the remote
   LNURL invoice plus that metadata. V1 additionally rejects an already completed
   invoice and validates the exact returned operation's rail, metadata, and amount.
   A successful-return native commit surviving before the SQLite link is found and linked by replay;
   the stated A4 guarantee covers successful-return commits and enumeration, but
   does not explicitly settle every interruption inside a dependency start.

8. **Observation, refund/reclaim, and lazy recovery (`code`, `test`, `claim`,
   `assumption`).** Status and await reopen the stored scope and pass one committed
   operation id to an observation capability exposing only cached status and
   rail-specific await methods—no invoice or start. It reconciles v1 and v2
   terminal/refund states while retaining active-state-machine encumbrance.
   Native funding change, cancellation, and refund/reclaim remain operations of
   that client under the direct payout-authority premise and A4. Accepted-payment
   and stability-pool/mint outputs remain client operations under the linked claim
   and A4. A lazily opened/restored wallet validates its prefix-partitioned
   payment or guardian scope, completes required mint recovery, reopens, and
   waits for output state machines before publishing the handle. No local second
   destination input was found in these paths.

## Residuals and limitations

- A hostile threshold's ability to replace metadata is outside the promise, but
  this daemon's later copy-forward of it is expressly inside and is not a residual.
- A previously adopted zero rate is payer-valid and can be carried forward; a
  new FI proposal must satisfy the current published floor. Production also
  requires the configured Guardian Verification Fee account before voting.
- Availability, cadence, minimum accumulation, transaction/gateway fees,
  consolidation, dust/rounding, and eventual settlement remain outside exact
  conservation and liveness.
- The missing LNURL-service trust premise is not silently granted. Fixing it
  requires a product trust decision or a verifiable final-payee mechanism.

## Weakest links

1. The three durable falsifications prevent a pass at this source state.
2. A4 does not explicitly classify interruption after a dependency has affected
   value but before its start future returns; this remains proof debt even apart
   from the concrete counterexamples.
3. Whole-object submit and value-moving route inventories remain source
   enumerations. The authority claim adds a focused source lint for the latter.
