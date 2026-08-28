# CLAIM-unrelated-gateway-balance-completes-item: Unrelated gateway balance completes item

For every production transition that durably sets a gateway allocation item
`i` to `allocation_items.status = 'completed'`, the target-side increase used
to justify that transition is attributable to `i`: the configured gateway's
Fedimint client has claimed the Bitcoin output sent by `i`'s own persisted
`wallet_operations` row (the row with `operation_type = 'gateway_funding'` and
`item_id = i.item_id`) to that row's persisted address, rather than merely
having an aggregate balance high enough because some other operation or deposit
increased the same gateway/federation balance.

The mechanically enumerable domains are: (G1) every production writer of a
completed gateway item and every constructor/field of its completion evidence;
(G2) every production reader of the gateway's claimed-deposit evidence; (G3)
every production gateway federation-balance reader; (G4) every production
gateway deposit-address allocation and recheck call; (G5) every wallet status
writer by which gateway completion becomes eligible; and (G6) schema keys and
loaders governing concurrent gateway items for one federation. The exact bad
durable predicate is an `allocation_items` row with `source_type = 'gateway'`,
`status = 'completed'`, and gateway `completion_evidence_json`, for which the
balance delta satisfying the completion guard was caused wholly or partly by
target credit not derived from the output identified by that item's persisted
wallet operation and deposit address. “Attributable” therefore requires an
item-address/output-to-Fedimint-claim identity, not simultaneous truth of an
unrelated wallet txid and a federation-wide balance inequality.

The adversary may schedule multiple valid accepted requests for the same target
federation, ordinary operator or third-party deposits to gateway-controlled
target addresses, dependency delay, 10-second allocation ticks, 30-second
wallet/observation ticks, and worker interleavings at every `await`; it may crash
and restart FLIP or an honest dependency before or after any committed SQL
statement or external effect. The configured gateway, provider wallet,
Fedimint, and chain observer return honest results. Authenticated Admin/operator
setup and actions are trusted. The claim does not rely on a malicious Admin,
direct database edits, malicious configuration or forged backend responses, or
out-of-band provider-wallet activity.

**Live restore.** The daemon can replace its data directory without exiting
([ARCH-liquidity-manager](../specs/ARCH-liquidity-manager.md), "Startup and
readiness"), so a restore need not be preceded by a process boundary. This does
not widen the domain above. The runtime generation holding every handle on the
data dir — periodic workers, the database, the secret store, target-federation
clients — is torn down in full before any file moves, and its replacement is
built through the ordinary startup path including startup recovery, so no state
derived from the old data dir crosses the swap. The admitted executions are
therefore exactly those of a restore performed by stopping the process,
replacing the data dir, and starting it again, which this domain already
admits. The process-lifetime shell retains only boot arguments, the
data-directory lock, and the Admin API listener, none of which this argument
reads; holding the lock across the swap rather than releasing and retaking it
narrows the window in which a second process could hold it. One process-global
value does survive a swap that a restart would reset — the wallet operation-id
counter — and it only increases, feeding an id already made unique by its
nanosecond component, so a surviving counter is narrower than a reset one.

## Status

Unverified.

## Assumptions

1. **A1 — durable database execution.** A committed SQLite statement survives
   ordinary process crashes and has the effects described by its SQL; a declared
   unique index rejects a conflicting insert. Used in L1, L3, L5, and the
   counterexample.
2. **A2 — honest pinned dependency execution.** The configured gatewayd and
   Fedimint clients execute the pinned source semantics described in L2. A
   confirmed deposit to one allocated gateway/Fedimint address can eventually
   be claimed into that gateway client's primary-module balance, and different
   deposits may become visible at different times. Transport failure delays or
   fails a call rather than forging a successful response. Used in L2, L4, and
   the counterexample.
3. **A3 — ordinary scheduling and funds.** Independently valid requests can be
   accepted when the honest observed provider-wallet capacity covers their
   reservations. A spawned periodic task can run again after a transient
   dependency delay; processes can crash at an `await` boundary and later
   restart with committed state. No fairness bound orders Fedimint deposit
   claims against FLIP's separate chain-settlement observation. Used in L3-L5
   and the counterexample.

No axiom assumes that a federation aggregate balance identifies the deposit
which changed it; that is the property being checked.
