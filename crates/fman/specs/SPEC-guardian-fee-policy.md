# SPEC-guardian-fee-policy: Formation fixes recipients; the FI controls the rate

## Record justification

The contract spans formation transcripts, FI maintenance RPCs, guardian metadata votes, payer parsing, and status reporting, so no single implementation artifact can own it coherently.

The two guardian-fee metadata keys jointly decide how much the federation
charges and who gets paid. The recipient policy is fixed in Manifold code; the
FI controls only the rate within the payer's supported range and published
minimum.

## Formation owns the recipient mapping

Each seat's single-signature SPv2 `BtcDepositor` account derives from the FMan
mnemonic and seat id and is returned in the signed seat acceptance. After DKG,
the FMan repeats it in its signed peer attestation. `fi-client` rejects any
mismatch before proposing formation metadata.

`GetPeerAttestation` also returns a `SeatEndpointProof`: an Ed25519 signature by
the seat endpoint key over the domain `fedi-fman-seat-endpoint-proof/v1\0`
followed by the attestation statement digest. The FI carries structural entries
pairing every attestation with exactly one proof in `ProposeFormationMeta`.
Each FMan constructs and bounds the canonical directory from those attestations,
validates it against the final federation config, verifies every paired proof
with that peer's configured API endpoint key,
and requires its own peer entry to name its own FMan identity. Endpoint proof
validation happens only while admitting the formation proposal; proofs are not
stored in consensus metadata.

The same typed proposal installs, as one guarded whole-object target:

- `fedi:fman_seat_bindings`;
- `fedi:guardian_fee_remittance_account`; and
- `fedi:guardian_fee_send_ppm`.

The fixed split gives FI weight four, every guardian weight one, and the
Guardian Verification Fee weight one, for total weight `guardian_count + 5`.
Entries are keyed and canonically ordered by destination account. Accounts are
single-signature `BtcDepositor` accounts and must be unique across FI, the
Guardian Verification Fee account, and every guardian.
The FI account comes from the consumer's formed-federation account provider,
guardian accounts from signed seat acceptances and attestations, and the
Guardian Verification Fee account from the Manifold environment profile. The
canonical directory remains the authoritative source of each guardian account.
The initial rate is the greater of the 5,000-ppm Manifold default and the
admitted published minimum.

`ProposeFormationMeta` carries the paired attestation/proof entries, FI and
Guardian Verification Fee accounts, rate, FI and seat identities, timestamp,
and exact `MetaConsensusBase`. Each FMan
derives the complete recipient list itself. It refuses a rate below its current
published floor before child access, refuses a federation that already has a
consensus directory with the distinct `FormationMetaAlreadyPublished` result,
and otherwise enters the same occurrence-bound guarded metadata submission
primitive as maintenance. Success means one guardian vote was submitted;
`fi-client` waits for exact consensus readback of all three fields.

There is no `ProposeGuardianFees` compatibility RPC and no DKG-transcript
fee-account envelope.
Formation endpoint proofs bind the directory directly to the final configured
seat endpoint keys, avoiding a second identity/account transcript in
`StartDkg` and `RestartDkg`.

## Later maintenance changes only the rate

After formation, `propose_guardian_fees` is a rate-change operation. It submits
only `fedi:guardian_fee_send_ppm` through `SetMetaField`. The generic validator
enforces the raw/value bounds and published floor before child access. The
formation-owned directory and recipient-list keys are absent from the generic
registry and are rejected if requested directly.

An explicit rate write is a new policy proposal even when its numeric value
already equals consensus, so it must satisfy the currently published floor.
Copying an older sub-floor rate unchanged while writing another key is not a
rate proposal and remains valid carry-forward behavior.

Every generic whole-object write that carries an existing fee policy revalidates
it before voting. It parses the stored canonical directory, verifies its FMan
attestations against the live federation config and requires the carried
account-keyed recipient list to equal the fixed recipient split. The
stored directory cannot re-prove endpoint ownership: its API endpoint
proofs were admission-time evidence and are intentionally not consensus data.

That boundary leaves a precise residual collusion model. A threshold of hostile
guardians already controls federation consensus and can adopt hostile metadata.
They can also admit a formation directory that misattributes an excluded,
non-colluding guardian by supplying threshold-valid endpoint evidence for the
seats they control. They cannot forge the excluded guardian's endpoint proof,
so including that honest endpoint under a false FMan identity is rejected.
Misattributing a guardian that joins the hostile threshold adds no independent
funds-control power: that threshold already controls the federation. Operators
still detect a live policy that stops paying them; FMan reports the policy but
does not automatically withdraw service.

## Whole-object safety and recovery

Both formation and maintenance commit to an occurrence-bound
`MetaConsensusBase`, covering the meta revision and exact raw bytes. A stale
base yields `MetaConsensusChanged`. One process-local target pin prevents two
different whole-object targets from being admitted for the same occurrence and
yields `MetaTargetConflict` for the loser. The FI serializes writers, rereads
consensus, and rebases exact replay after staleness. A fresh read proving the
target reached consensus takes precedence over late stale responses.

Before its first formation proposal, the FI durably records the complete target:
its canonical-directory readback prediction, paired attestation/proof entries,
FI account, derived recipient list, and initial rate. An interrupted formation replays those values exactly instead of
refetching capabilities or re-evaluating a changed default. Exact consensus
readback checkpoints the target as confirmed. Later formed-state reconciliation
still requires the immutable directory and recipients, but accepts an updated
rate and therefore needs no formation-only account provider.

The complete raw metadata object is capped at 1,048,576 bytes before parse or
fan-out on both sides. FMan preserves unrelated fields, canonicalizes the target,
and submits with the seat's guardian authentication. The upstream meta module
is the only source; config metadata is never used for guardian fees.

## Reporting

FMan stores no separate fee agreement. `SeatStatus` and `GuardianFees` derive
the current rate, expected account, share, and policy match from live consensus
metadata. An unset policy, malformed policy, and policy excluding this guardian
remain distinct observations. Revenue disagreement is reported for an operator
decision rather than causing automatic shutdown.
