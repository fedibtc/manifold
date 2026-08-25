# Current argument

## Argument

**L1 (`code` + `schema`) — acceptance persists a hash the requester did not
choose.** `run_pipeline` refuses the request unless
`details.federation_config_hash` equals the invite preview's, so the value
`insert_allocation` stores is the previewed federation's own. The `schema` half
is narrower than it reads: the hash lives inside the `target_json` TEXT column
and no constraint checks it. What the schema enforces is one allocation per
federation, which makes that `INSERT OR IGNORE` the single writer of the
record the worker later reads.

**L2 (`code`) — the address the provider funds is minted by a client checked on
that same handle.** `FedimintStabilityPoolBackend::allocate_peg_in_address`
obtains one `ClientHandleArc`, calls `usable_target` on it, and returns early
unless that answers `Usable`; only then does it reach
`allocate_deposit_address_expert_only`. `usable_target` compares
`federation_config_hash(&client.config())` against the persisted hash and
requires `StabilityPoolClientModule` to resolve. Two things have to hold for
that answer to describe the client that mints.

*No substitution.* The handle is a local held across both, so no pool operation
can swap it: `evict_if_idle` skips a federation whose `Arc::strong_count`
exceeds one, and `evict` and `shutdown_all` call `Arc::try_unwrap` and only log
on failure, leaving a held instance running. The borrow checker helps —
`get_first_module` returns an instance borrowing the handle.

*No in-place mutation.* Under A3's second clause the held client's config has
one writer, unreachable for any config with `broadcast_public_keys: Some`, and
under A4 no config the worker can be holding has `None`. Both clauses are
needed: A3 alone leaves the writer reachable, and the reopen clause says nothing
about it.

**L3 (`code`) — the worker's per-tick `check_target` is a fast fail, not the
fence, and it is a blunt one after the first tick.** `process_stability_pool_item` calls it every tick before the peg-in
address, so a target that is unusable on the item's *first* funding tick costs
no `wallet_operations` row. That is the whole of its value, and it is narrower
than "before any row exists": on later ticks the row already exists, and
`fail_item` writes only `allocation_items`, so an `Unusable` answer at tick
N+k leaves a funding operation behind and sets the item `Failed` rather than
`ActionRequired`. `check_target` runs on every tick, including after the peg-in
is claimed, so the item it fails can be one whose provider value already sits
inside the target client — and `abandon_target_client_value`, the verb that
records value left behind, requires both `ActionRequired` and a claimed peg-in,
so it refuses that item twice over. Surfaced below.

Its answer describes the handle it opened and it drops that handle before
returning, so under A3 it cannot bind a later call. L2 binds the mint.

**L4 (`enum`) — the only address a stability item's funding step sends to is
the minted peg-in address.** `submit_funding_withdrawal` sends to
`FundingStep::address`, which for the stability path is
`item.step.peg_in_address`. The single production writer of that field is the
block in `process_stability_pool_item` that persists what
`allocate_peg_in_address` returned, and no path clears it, so the re-mint guard
cannot fire once a wallet row exists. The operator withdrawal verb also sends to
a caller-supplied address; it is outside this claim by the adversary model's
"cannot act as Admin", and is named so the enumeration is not read as covering
the daemon.

By L1, L2 and L4, the address any provider outflow reaches was minted by a
client whose config hash was `H` and which had the module. With A2 the claim
holds for the outflow it quantifies over.

## Residual windows

## Weakest links
