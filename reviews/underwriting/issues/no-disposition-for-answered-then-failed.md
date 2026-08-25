# Six screens invented six different answers to "we have data and the refresh just failed"

> There were **seven**. The seventh (`BackupPage`) was found by the enforcement rule, not by the
> scan — see the remainder section.

- **Status:** fixed — primitive and first two sites in `37dd6f70`
  (`fix/fman-ui-query-disposition`); remaining sites, a seventh the scan missed, and the rule in
  `c1cf7b1e` + `24f178da` (`fix/fman-ui-disposition-remaining`). One site is deliberately left
  unconverted with a stated reason, below.
- **Tier:** checked (systemic)
- **Level:** code, at all sites
- **Found by:** scanner (5 sites), checker (added the 6th)
- **Where:**
  - `.../features/seats/hooks/use-seat-rows/useSeatRows.ts:31-36` + `pages/seats/SeatsPage.tsx:9`
    — claims an empty fleet
  - `.../pages/overview/OverviewPage.tsx:38` — discards the data it still holds
  - `.../features/offer/hooks/use-offer-form/useOfferForm.ts:41` — disables the Save control and
    replaces the form's error line with a load error
  - `.../pages/seat-detail/SeatDetailPage.tsx:14-33` — terminal error, no retry
  - `.../pages/wallet/WalletPage.tsx:26-57` — correct, but with no retry and an undated banner
  - `.../AuthorizationPanel.tsx:20-25,60-64` — correct, and the only site rendering data and a
    refresh failure at once
  - `.../pages/backup/BackupPage.tsx:20` — **missed by the scan**; found by the rule. Deletes the
    operator's identity keys for the whole outage, with no retry

**What happens:** React-query retains data through a failed refresh, so every screen must decide
what to show when it has an answer and the last attempt failed. There is no shared answer, so
each site improvised. Two got it right, four did not, and two of the four state something false
about the fleet rather than merely looking wrong.

**The result:** One daemon outage produces four different behaviours across one dashboard, and
the operator cannot learn a rule from any of them. This is the shared root of
[`overview-blanks-for-the-whole-outage`](overview-blanks-for-the-whole-outage.md),
[`seats-page-claims-an-empty-fleet-when-the-read-failed`](seats-page-claims-an-empty-fleet-when-the-read-failed.md)
and [`seat-detail-stops-polling-permanently-after-one-failure`](seat-detail-stops-polling-permanently-after-one-failure.md);
fixing them one at a time leaves the next screen free to invent a fifth answer.

**Failed defense:** "Each screen knows its own needs." The module disproves this itself — it has
a rigorously argued, centrally enforced policy for unknown *amounts* (never render zero for a
balance you do not know) and nothing at all for unknown *collections*. The same reasoning applies
and was simply never lifted.

**Fix direction:** One `useQueryDisposition` / `QuerySurface` mapping `{data, isPending, isError}`
onto four states — loading, failed-with-retry, stale-with-banner, content — and every screen
renders through it. `WalletPage` is the existing reference for the stale-with-banner case.

## Partial fix — `37dd6f70`

The primitive exists and the two sites that stated something false are converted.

- `.../shared/query/use-query-disposition/useQueryDisposition.ts` — `readQueryDisposition` maps a
  screen's whole read set onto `loading` / `failed` / `stale` / `content`. It is only `content`
  once **every** read has answered, any one failure marks the surface, and a set that holds no
  answer at all is `failed` rather than `stale` even when a sibling read answered. A `stale`
  surface is dated by its **oldest** answer — a mixed-age screen is only as fresh as its stalest
  part. `useQueryDisposition` adds a `retry` that forces an attempt on every read behind it.
- `.../shared/components/query-surface/QuerySurface.tsx` — the one rendering of those four, reusing
  the existing `StaleDataBanner` from `@operator-ui/common-ui`. A screen wraps whatever it says
  about the fleet in this, so one outage teaches the operator one lesson everywhere.

**Converted (2 of 6):**

- `useSeatRows` + `SeatsPage` — see
  [`seats-page-claims-an-empty-fleet-when-the-read-failed`](seats-page-claims-an-empty-fleet-when-the-read-failed.md)
- `OverviewPage` — see
  [`overview-blanks-for-the-whole-outage`](overview-blanks-for-the-whole-outage.md)

## Remainder — `c1cf7b1e`

**The scan undercounted. There were seven sites, not six.** `pages/backup/BackupPage.tsx:20` gated
the operator's own identity keys on `onboarding.isError` and deleted them for the whole outage, with
no retry. It surfaced only when the rule below was scoped to `pages/**` and landed red on it.

**Converted:**

- `features/offer/hooks/use-offer-form/useOfferForm.ts` — the one with the real hazard. `canSubmit`
  now reads the disposition, so `content` and `stale` both permit a write: the operator is
  overwriting a price they were shown, and a failed *background* poll does not change that. The read
  failure has left the form's error line entirely — validation and a refused write keep it to
  themselves, which is the second channel the earlier pass was protecting. `OfferPage` renders the
  form through `QuerySurface`, so a first-read failure offers a retry instead of a blank field whose
  emptiness would have read as "not selling".
- `pages/seat-detail/SeatDetailPage.tsx` — W3.4 (`397bf011`) had already fixed the polling and added
  a retry; the branch still missing was "we hold a report and the last poll failed", which replaced
  the invite code and health with an error screen. Only the surface changed here; the polling is
  untouched.
- `pages/wallet/WalletPage.tsx` — converted after all. The earlier judgement ("cosmetic") was wrong
  on two counts: the page had **no retry** in its failure branch, and its bespoke
  "Showing last-known data" banner carried **no timestamp**, where `StaleDataBanner` dates the
  answer. Converting also drops a duplicated `PageHeader` render across three branches.
- `pages/backup/BackupPage.tsx` — the seventh site. Holding the whole block behind one answer also
  removes the per-field `'—'` fallbacks, which said "unknown" in the one case where the daemon had
  actually answered.

**Not converted, and why:**

- `shared/components/authorization-panel/AuthorizationPanel.tsx` — left deliberately, and it stays
  correct. Three reasons, in order of weight:
  1. It is **not a screen**. It takes `{data, isLoading, error}` as props, holds no query, and has
     no `refetch` to give `QuerySurface`'s `onRetry`.
  2. Both callers (`AuthorizationPage`, `SetupAuthorization`) already own a **"Check now"** control
     directly beneath it. `QuerySurface` would put a second, competing retry button immediately
     above that one.
  3. Its file comment states the panel "renders state, never actions", *because* the two callers
     need different controls over the same state. Putting a button inside it contradicts the reason
     the component is shaped the way it is.

  Its stale rendering is also richer than the shared one: it keeps the QR, the key and the copy
  control visible and adds a specific "this state could not be refreshed" line. The cost of leaving
  it is that one surface in the app words staleness differently — recorded here rather than fixed,
  because the alternative degrades it.

## The rule — `24f178da`

`packages/biome-plugins/screen-query-disposition.grit`, registered in `biome.json` and scoped to
`apps/fleet-manager/src/pages/**/*.tsx` minus `__tests__`. Any `$read.isError` in a screen is an
error naming the file, the line, and the replacement.

Biome rather than `check-structure.mjs`: check-structure is the folder-layout contract and reads
files as text, whereas this is a claim about code. Biome already parses these files, already runs in
CI as a listed gate, and prints the offending expression under a code frame.

**Two exclusions, both deliberate, both a real cost:**

- **Feature components are out of scope.** A *mutation's* `isError` is an action that failed, not a
  read to dispose of, and there is no syntactic way to tell the two apart — banning it would fire on
  every Save button in the app. **The residual gap is honest: a future screen can push the branch
  down into a feature hook and escape the rule.** That is exactly where `useOfferForm` hid it. The
  rule catches the screen; nothing yet catches the hook.
- **`apps/liquidity-provider` is out of scope.** FLIP has no copy of the primitive, and a rule may
  only require what exists. Five FLIP pages branch on raw `isError` today and are untouched by this.

`packages/biome-plugins/test-screen-disposition.sh` (in `pnpm plugin:test`) asserts six placements
and lifts the `includes` glob from the real `biome.json` rather than restating it — a glob anchored
at `apps/…` instead of `**/apps/…` matches no file, so the plugin loads, reports nothing, and looks
green. That trap was hit while writing this rule.
