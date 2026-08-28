# Current argument

## Argument

**L1 (`claim`) — every newly created allocation has an exact-federation
endorsement admission.** This is the imported conclusion of
`unendorsed-federation-allocation.md`.

**L2 (`claim`) — sharing and races cannot multiply that allocation.** This is the
imported conclusion of `duplicate-federation-allocation.md`.

**L3 (`enum` + `claim`) — the capability's bounded durable effect is exactly
the two supported source slots.** The imported cardinality conclusion allows at
most one item per source. Regenerating `plan_allocation` and the public
`SourceType` cases yields only `gateway` and `stability_pool`, with each case
adding at most one planned row. Combining that exhaustive case split with L1
and L2 gives the stated durable-row bound.

## Residual windows

- A valid endorsement authorizes any holder, indefinitely, until an admission
  lookup observes its FMan badge revoked. This is the accepted bearer-capability
  tradeoff, not a gap in the composition.
- Work already accepted is not cancelled on later revocation.
- The bound is on allocation and item rows, not external financial effects;
  source-specific crash recovery and submission guarantees require their own
  records.
- The claim does not prove that an item completes with attributable target-side
  liquidity;
  [CLAIM-unrelated-gateway-balance-completes-item](../CLAIM-unrelated-gateway-balance-completes-item.md)
  covers that property separately.

## Weakest links

1. **L1–L2 (`claim`)** — both imported properties must hold and remain current.
2. **L3 (`enum`)** — adding a source type changes the bound.
