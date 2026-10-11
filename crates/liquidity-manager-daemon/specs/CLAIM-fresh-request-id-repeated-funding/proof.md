# Current argument

## Argument

The uniqueness constraints and restore comparison prevent two simultaneous live
allocations for one semantic request, but official release deletes the allocation
and item rows without retaining a historical semantic-request identity. The
counterexample is recorded in
[`falsification-release-erases-identity.md`](falsification-release-erases-identity.md).
Therefore the claim is false even though the release trace performs no funding
effect.

## Weakest links

Repair requires either retaining a durable historical identity or an explicit
semantic decision that authenticated release ends the claim's quantifier.
