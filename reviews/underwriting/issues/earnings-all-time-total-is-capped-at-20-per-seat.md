# "Earned, all time" silently stops counting after 20 fee remittances per seat

- **Status:** fixed — daemon half `f108c486`, client half on
  `fix/fman-ui-lifetime-earnings`. The `@live` rung-M3 acceptance is still open;
  see the last section.
- **Tier:** blinded convergence (2 roles) + coordinator-verified at both ends
- **Level:** code
- **Found by:** courier, ops-drill
- **Where:**
  - `operator-ui/apps/fleet-manager/src/features/overview/api/hooks/use-guardian-fees/useGuardianFees.ts:45`
    (sends `limit: null`)
  - `crates/fman/core/src/admin.rs:381` (`limit.unwrap_or(20)`)
  - `operator-ui/apps/fleet-manager/src/features/overview/utils/deriveEarnings.ts:100-112`
  - `operator-ui/apps/fleet-manager/src/pages/overview/OverviewPage.tsx:70` (the label)

**What happens:** The earnings rollup asks each seat for its guardian-fee remittances with
`limit: null`. The daemon reads that as "no preference" and applies its own default of 20
(`limit.unwrap_or(20)`), returning only the newest 20 remittances for that seat. `deriveEarnings`
sums whatever came back into `totalMsat`, and the Overview renders it under the label
"Earned, all time".

**The result:** A money figure that is correct for a new fleet and quietly wrong forever after.
Once any seat has earned its 21st remittance, its older earnings drop out of the total, and the
number shown as a lifetime figure begins to *decrease* relative to reality with every further
payment. Nothing on screen marks it as windowed. An operator reconciling against their own
records finds a total that has no explanation and no visible rule.

**Failed defense:** "`limit: null` obviously means unlimited." It does not — the daemon
resolves it to 20, and the client never checks. This is the one place the module's own
discipline was not applied: it has a rigorously argued rule that unknown *amounts* must never
render as a number (`format.ts`, `federationBalance.ts`, `useOverviewEarnings.ts:36-43` count
unreadable seats rather than treating them as zero), and then prints a knowingly partial sum as
a lifetime total.

**Fix direction:** Decide the window explicitly and say it on screen. Either request a real
lifetime figure — which needs a daemon-side aggregate or an explicit large limit, and the
fleet-level fee summary is already written up in `operator-ui/docs/daemon-aggregate-verbs.md` —
or label the tile for what it is ("Earned, last 20 per seat"). Passing `limit: null` and
labelling the result "all time" is the one option that cannot be right.

## What has been done (daemon)

W1.3's daemon half is implemented on `feat/fman-lifetime-earnings`. The
`GuardianFees` response now carries `lifetime_remitted_msat`: everything payers
have ever remitted to that seat, swept funds included. It is read as its own
scalar — the walk over the account's full history lives in
`fman_core::guardian_fee::total_remitted`, and the windowed list stays exactly
what its doc-comment says it is.

Computed over the full history rather than kept as a counter, deliberately: the
stability-pool server maintains the balances beside it but exposes no lifetime
deposit aggregate, and nothing inside the daemon observes a remittance as it
lands, so a counter would have no event to increment from and would have to be
rebuilt by this same walk on any restore from mnemonic. The reasoning is on
`total_remitted` itself.

`crates/fman/core/tests/guardian_fee_history.rs` drives 21 remittances through
the walk and asserts the total is 231,000 msat rather than the newest-twenty
230,000 msat, plus a history longer than one 128-entry page. Observed red before
the fix.

## What has been done (client)

`deriveEarnings` reads `lifetime_remitted_msat` as the scalar it is and adds it
across the fleet's seats. `remittances` no longer produces a money total — it
feeds the timeline only, which is what its doc-comment always said it was. The
tile the Overview labels "Earned, all time" is therefore seat sales plus that
scalar, and the label is now true. `useGuardianFees` still sends `limit: null`,
which is correct once nothing aggregates the list: it bounds a display window,
and the comment on the hook now says so.

Two tests cover it, both observed red against a `deriveEarnings` reverted to
summing the window:

- `deriveEarnings.test.ts` — "should total guardian fees from the lifetime
  figure, not the returned window". The fixture's lifetime figure (41,500,000
  msat) and its window (16,000,000 msat) are deliberately different numbers, so
  a total taken from the list cannot pass. Red: `expected 16000000 to be
  41500000`.
- `OverviewPage.test.tsx` — "should read the all-time tile from the lifetime
  figure, not the remittance window". Red: the tile rendered `66,000 sats`
  against the true `91,500 sats`.

**The seat-sales half is a different shape and needs no change.** It totals the
`ListSeats` answer, and `ListSeats` takes no limit — `admin.rs:349` returns
`fleet.seat_summaries()` whole. It is the fleet, not a page of it.

**Still open — the `@live` acceptance.** The rung-M3 spec this issue's task asks
for — create more than 20 remittances against a real daemon and assert the
displayed total equals the true sum — is not written. It depends on W0.2, which
does not exist yet. Nothing here has been observed against a real daemon end to
end; the evidence above is unit and component level plus the daemon-side test in
`crates/fman/core/tests/guardian_fee_history.rs`.
