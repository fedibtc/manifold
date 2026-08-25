# CLAIM-federation-capability-bounded-allocation: Federation capability bounded allocation

A bearer FMan endorsement can cause at most one durable FLIP allocation for the
federation it names, containing at most one gateway item and at most one
stability-pool item. No public request can create those durable rows without
first passing the endorsement admission checks at its creation point.

The adversary may share/replay an endorsement, use arbitrary requester keys,
make concurrent requests and retries, lose responses, and crash FLIP around any
persistent operation. This is deliberately not a requester-identity,
endorsement-expiry, or post-admission-revocation claim.

## Status

Unverified.

## Assumptions

The effective trusted base is the union of the imported records' A1–A3.
