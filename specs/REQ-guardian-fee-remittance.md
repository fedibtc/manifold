# REQ-guardian-fee-remittance: Guardians earn ongoing revenue from the federations they host

Source: Product policy, required for MVP.

A paid seat is a one-time payment for an open-ended hosting obligation
([ARCH-fleet-manager-product-boundary](../crates/fman/specs/ARCH-fleet-manager-product-boundary.md)), so an
operator would otherwise price unbounded future disk, bandwidth, and
availability into a single up-front charge. Operators must earn continuing
revenue from the federations they guard. This is separate from setup pricing:
the per-seat net guarantee in
covers setup settlement only and compensates nothing about operation.

## The interoperability contract

Payer-side collection ships in the Fedi app and is authoritative for the wire
contract. Fee income is enforced by payer software, not by `fedimintd`
consensus.

- Two consensus-metadata keys are read together — `fedi:guardian_fee_send_ppm`
  and `fedi:guardian_fee_remittance_account`. Both or
  neither; a rate of zero stops new accrual while remittance stays available to
  drain what already accrued. The payer's 210,000-ppm compatibility ceiling is
  the one rate bound everywhere: Manifold FI
  producers and every FMan validator enforce that same `0..=210,000` ppm —
  with a 5,000-ppm default. The payer also publishes a **minimum** rate that
  every FMan enforces on new proposals
  ([SPEC-setup-payment-federations](./SPEC-setup-payment-federations.md)).
- The payer accrues per guardian share and deposits a share into the
  **stability-pool BTC balance** of that share's recipient account once it
  clears the module's minimum deposit, so small shares accumulate at the payer
  rather than being paid below the minimum.
- Each recipient is a **single-sig `BtcDepositor` stability-pool account**, and
  each deposit carries an accounting breakdown sealed to that account's public
  key.
- Fees split by **weighted shares** using the versioned recipient list carried
  in consensus metadata.
- In the MVP policy, FI receives four shares, every guardian receives one, and
  one share is the **Guardian Verification Fee**, paid to its deployment-owned
  account. Setup payments and every other fee stream remain unchanged. The FI
  account, every guardian account, and the Guardian Verification Fee account
  use distinct destinations.

## Obligations on this repository

1. **The FI chooses the rate, not the recipient policy.** The FI may propose
   `0..=210,000` ppm (the payer ceiling). It cannot change the 4:1:1 split, drop a recipient,
   duplicate an entitlement, or alter a weight; every FMan validates the
   canonical full-account vector before casting its guardian vote. Each
   accepted account is repeated in that seat's signed post-DKG attestation.
   `fi-client` checks it against the durable signed acceptance before publishing
   the exact all-seat directory into consensus. A `SeatEndpointProof` by the
   final config's Iroh API key covers the complete attestation digest, including
   FMan identity and account, so every later FMan vote can authenticate the
   directory without a second pre-DKG account envelope or retained transcript.
   `fi-client` derives the vector
   only from a consumer capability resolving the formed federation FI's own
   SPv2 `BtcDepositor` account, each durable FMan-signed seat acceptance, and
   the deployment profile's Guardian Verification Fee account. `fi-client`, not
   the operation caller, chooses the exact lookup identity from its persisted formed invite;
   the fee-proposal operation accepts only the rate. A production consumer
   resolves that exact already joined federation client and returns
   `spv2.our_account(BtcDepositor)`. It must not accept an arbitrary recipient
   from user or RPC input. Development tooling may expose an explicit informed
   test override when it documents that weaker source and remains outside
   production use.

   `StartDKG` accepts bare upstream setup codes. It rejects the entire set
   before child mutation unless the count is exact, codes are unique, this
   seat's Iroh API key is present, and its own code exactly recomputes. It does
   not cross-verify endpoint signatures on other peers' codes: Fedimint's DKG
   peer-to-peer handshake authenticates those endpoint keys.
   Because later maintenance
   votes carry the whole metadata object, every submit path also revalidates
   any fee keys it carries against the endpoint-proof-bound directory and
   fixed role split; an unrelated field update cannot copy forward an invalid
   fee policy as this guardian's vote.

2. **Revenue recovers from the operator mnemonic alone.** Earning it must not
   create new backup material: an operator still backs up the mnemonic plus the
   data root and nothing else.

3. **The operator can see and move the money.** Per-federation accrued balance,
   what each payment was for, and swept amounts are visible to the operator, and
   the operator directs where swept value goes.

4. **An FMan can tell when its revenue stops.** Consensus metadata stays
   mutable, so a rate dropped to zero or an entry removed or reduced must
   surface. Silently hosting with no revenue and no signal is not acceptable.

5. **Fee income is best-effort and load-bearing for nothing.** Because payers
   enforce it, no FMan behaviour affecting federation correctness, guardian
   availability, or an existing seat's obligations may depend on fee income
   arriving. A federation that has stopped paying this FMan is reported to the
   operator, who decides whether to keep hosting it; the daemon does not
   withdraw service on its own.

6. **Production recipient configuration fails closed.** The Guardian
   Verification Fee account is deployment-owned public configuration in the
   Manifold environment profile; no FI or FMan may substitute a generated
   fallback.

7. **The production FI recipient is selected by capability, not by request.**
   The consumer capability receives the federation id `fi-client` parsed from
   its durable formed invite and returns that joined client's own SPv2
   `BtcDepositor` account. Failure to resolve it is typed absence and reaches
   no guardian vote. `fi-client` still validates account shape, account
   uniqueness, the complete canonical vector, and exact consensus readback;
   trusting a production consumer to implement its local capability honestly
   does not move recipient policy into that consumer.

How this repository meets these is
[SPEC-guardian-fee-policy](../crates/fman/specs/SPEC-guardian-fee-policy.md)
and the `fman-fedimint` boundary in
[ARCH-fleet-manager](../crates/fman/specs/ARCH-fleet-manager.md).

## Not in scope here

Payer-side accrual, share splitting, remittance scheduling, and the
minimum-deposit accumulation ship in the Fedi app repository. This repository
consumes that contract; it does not implement it.
