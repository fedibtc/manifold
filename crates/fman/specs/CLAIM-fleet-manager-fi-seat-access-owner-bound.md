# CLAIM-fleet-manager-fi-seat-access-owner-bound: FI seat access is owner-bound

For every daemon invocation of an FI-authenticated RPC verb whose
`SignedRequest` verification succeeds, let K be the outer public key that
verified the exact direction, verb, and payload. The invocation never obtains
privileged local access to a registered seat S unless S's durable creation row
has `seats.fi_id = K` before that access.

Privileged local access includes a seat authority escaping the ownership gate;
constructing, selecting, or retaining it for verb behavior; returning its
stored `CreateSeat` commitment; or using it to access the seat's facts, runtime
state, durable or DKG rows, API credential, supervisor, process, or data
directory. Resolving the request's named seat solely for the ownership
comparison and aggregate capacity-liveness reads are excluded. An absent seat
or owner mismatch produces semantic `UnknownSeat` before seat-specific behavior.

The property covers every current FI-signed verb, fresh and replayed
`CreateSeat`, arbitrary valid inputs, known victim IDs, crash and restart, and
concurrent FI or trusted operator activity. Captured valid envelopes remain
attributed to their actual signer. Ordinary public Fedimint traffic is not
local seat access.

## Status

Unverified.

## Assumptions

- [CLAIM-fleet-manager-selects-only-owned-seat-authority](CLAIM-fleet-manager-selects-only-owned-seat-authority.md)
- [CLAIM-fleet-manager-confines-seat-local-authority](CLAIM-fleet-manager-confines-seat-local-authority.md)
