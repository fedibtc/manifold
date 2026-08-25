# A daemon blip empties the earnings dashboard and it stays empty until the daemon returns

- **Status:** fixed — `37dd6f70` (`fix/fman-ui-query-disposition`)
- **Tier:** checked + blinded convergence (3 roles)
- **Level:** code
- **Found by:** scanner, checker, coroner, ops-drill
- **Where:**
  - `operator-ui/apps/fleet-manager/src/pages/overview/OverviewPage.tsx:25-47`
  - correct sibling for contrast: `operator-ui/apps/fleet-manager/src/pages/wallet/WalletPage.tsx:26-57`

**What happens:** The Overview renders a banner *instead of* its content whenever `isError` is
true. React-query retains `data` through a failed refresh — the error action keeps the previous
data and only sets `status: 'error'` — so the page discards data it still holds. Recovery is
worse than losing one render: query-core clears `error` and resets `status` only when
`data === undefined`, so once the page has flipped, every subsequent retry leaves it in `error`.
The page stays blank for the **entire** outage and only a success restores it.

**The result:** During any daemon restart or outage the operator's earnings dashboard is blank
rather than stale. It takes four failed attempts to get there (the queries keep react-query's
default `retry: 3`, about 7 s of retry delays), which a daemon restart supplies comfortably. The
boot gate does not cover this once `Onboarding` has cached data. Two pages fed by the same query
teach two different lessons about one outage: the Wallet shows its balances under a staleness
banner and the Overview shows nothing.

**Failed defense:** "An error banner is honest." Honest about the refresh, dishonest about the
data it deletes from the screen. Stale earnings with a marked staleness are strictly more
informative than no earnings.

**Fix direction:** One disposition, applied everywhere — no answer → loading; no answer plus
failure → failure with retry; answer plus failed refresh → the answer under a staleness banner.
`WalletPage` is already the reference implementation. See
[`no-disposition-for-answered-then-failed`](no-disposition-for-answered-then-failed.md) for the
systemic version.

## Fix — `37dd6f70`

`OverviewPage` no longer branches on `isError`. Its three fleet-wide reads — `ListSeats`,
`ListPaymentFederations` and `ShowPlans` — go through the shared `useQueryDisposition` primitive,
and the page body renders inside `QuerySurface`. A failure while those reads hold answers is now
`stale`: the figures stay on screen under the existing `StaleDataBanner`, dated by the oldest
answer behind them. A failure with no answer at all is `failed` and gains a **Try again** control,
which the page did not have before.

The regression test — `should keep the earnings figures under a staleness marker for a whole
outage` — models the fault rather than a single failure: it loads figures, takes the daemon down,
and drives `refetchQueries` under react-query's default `retry: 3`. It asserts the balance read was
attempted exactly 4 times per refresh (12 across three refreshes) and that the wallet balance and
the headline stay on screen under the staleness banner throughout, including after the later
refreshes that the pre-fix page could never recover from.

Observed red against the pre-fix page: the test failed at `Showing last-known data`, and the dumped
DOM was the heading plus a lone error banner reading "daemon restarting" — no tiles, no balance,
nothing else. The pre-existing case `should show an error banner, never the healthy banner, when a
query fails` also went red on its new **Try again** assertion.
