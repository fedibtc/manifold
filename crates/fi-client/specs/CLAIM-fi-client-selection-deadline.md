# CLAIM-fi-client-selection-deadline: Selection distinguishes deadline exhaustion from pool shortfall

For an initial `FiClient` or `FmanSelectionQuery` selection preview, publisher
controlled cheap advertisements cannot make the API return
`FiError::InsufficientFmanSeats` after its absolute deadline before it starts
verification of enough retained, statically eligible, badge-valid honest
advertisements. The distinct replacement-preview API is outside this claim.

## Status

Unverified.

## Assumptions

- The runtime clock is monotonic and the absolute timer and explicit clock checks
  observe deadline expiry; synchronous work between yield points and executor
  starvation are outside this async guarantee.
- Open-write publishers control their advertised price and claimed issuer before
  later author and issuer binding verification.
- The concrete verifier accepts the honest advertisements and asynchronously
  rejects the adversarial advertisements used by the selection test.
