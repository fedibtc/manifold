# CLAIM-request-material-substitutes-nondirectory-identity: Request material substitutes nondirectory identity

Request-carried FMan trust material cannot make an identity absent from the
previewed consensus seat directory count toward `all_trusted` or
`consensus_majority_trusted`, nor substitute one directory identity's signed
material for another. The requester may omit, replay, reorder, duplicate, or add
material, but cannot forge FMan, issuer, or federation-consensus signatures.

## Status

Unverified.

## Assumptions

- **A1 — consensus directory authenticity.** The previewed seat bindings describe
  the target federation's authoritative seats and `verify_for_federation` checks
  their complete correspondence.
- **A2 — signature/canonical verification soundness.** The domain verification
  routines accept only material signed by the exact expected FMan identity.
