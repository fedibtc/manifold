# Released allocation erases semantic-request identity

At source change `moqkvurn`, accept one valid signed request, cancel it before
any funding operation, then use the documented authenticated
`release_federation_allocation` Admin operation. The release path verifies that
the allocation holds no reserving item, unsettled operation, or fulfilled value,
then deletes its allocation and item rows. It retains no historical semantic
request identity.

Replaying the same still-valid signed request therefore finds no existing
allocation and commits a new independent allocation. All immediate assumptions
remain granted: callers use documented authenticated interfaces, storage behaves
correctly, and one daemon owns the data root. This is a source-derived
counterexample; the ordinary library suite separately exercises the cancellation,
deletion, and subsequent takeover mechanisms.
