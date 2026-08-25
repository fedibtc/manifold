# A guardian that fails after it starts running keeps displaying its last-known health

- **Status:** fixed — `397bf011` (W2.3)
- **Tier:** testimony (single role) — mechanism coordinator-verified, cost narrative corrected
- **Level:** policy (two documents require continuous health polling; both upgrade this rather
  than excuse it)
- **Found by:** coroner
- **Where:**
  - `operator-ui/apps/fleet-manager/src/shared/utils/seatStatus.ts:37-45`
    (`isSeatReportNonTerminal` returns `false` once `phase === 'running'`)
  - `operator-ui/apps/fleet-manager/src/features/seats/api/hooks/use-seat-reports/useSeatReports.ts:49-55`
  - `operator-ui/apps/fleet-manager/src/features/seats/api/hooks/use-seat-status/useSeatStatus.ts:17-20`

**What happens:** Both seat pollers stop entirely when a seat's report reaches a terminal
*phase*. `running` is treated as terminal, on the stated ground that a settled seat "is done
changing on its own". But `phase` and `health` are separate axes of the same `SeatReport`, and
the chip an operator reads is driven by `health` (`describeSeatReport`). Phase settles once;
health changes for the rest of the seat's life. From the moment a seat starts running, its
health is never fetched on a timer again.

**The result:** A guardian that dies after formation continues to render its last fetched
health. **Correction to the finding as filed:** react-query's `refetchOnWindowFocus` is left at
its default `true` with `staleTime: 15_000`, so tabbing away and back to the dashboard *does*
refetch and reveal the true health. The reported "eleven days of Healthy" therefore requires the
page to stay open without focus changes — a wall display, a dedicated monitoring tab, or one
long uninterrupted session. That is a narrower regime than the coroner claimed, and it is still
the regime a fleet dashboard is built for: the failure lands exactly where nobody is clicking,
and a monitoring surface whose freshness depends on alt-tabbing is not a monitoring surface.

**Failed defense:** Cost — per-seat health is an N+1 with no aggregate verb, and stopping at a
terminal state is the largest available saving
(`operator-ui/docs/daemon-aggregate-verbs.md`). It fails because saving and correctness are not
in tension here: the module already owns `pollingIntervals.ts` and already accepts 30 s and 60 s
fleet-wide cadences. A settled seat needs a *slower* cadence, not *none*; `false` is a category
error rather than a tuning choice. The cost argument is also weakest where it is invoked — an
all-running fleet at one call per 30 s per seat is the same order as `ListSeats` itself. And it
inverts the value: forming seats are watched by a present operator, running seats are watched by
nobody but the machine.

**Fix direction:** Split the two questions. Formation completeness governs the *fast* cadence; a
settled active seat falls back to a slow health cadence (`LIST_POLL_MS`, jittered, with the
existing backoff); only `state === 'decommissioned'` stops polling outright, since that is the
one report that genuinely cannot change. The long-term fix is the daemon ask already written up
— live health on `ListSeats` — and the slow-cadence fallback is what the dashboard owes until
it exists.

**Test gap:** every existing artefact asserts that `running` + `unavailable` *renders*; none
asserts that it is ever *fetched*. The test that would have caught this asserts the
`refetchInterval` for a `running` report is a number, not `false`.

## Fix (`397bf011`)

`isSeatReportNonTerminal` is gone. `seatStatus.ts` now answers the two questions separately —
`isSeatForming` (fast cadence) and `isSeatReportFinal` (decommissioned, the one report that
cannot change) — and one policy in
`features/seats/api/hooks/use-seat-status/seatStatusQuery.ts` reads both: `SEAT_FORMATION_POLL_MS`
while forming, `LIST_POLL_MS` once settled, jittered, with the existing per-seat backoff, and
`false` only for a decommissioned report.

Covered by, and observed red against the restored terminal-phase check:

- `use-seat-status/__tests__/seatStatusQuery.test.ts` — "should keep a running seat on a numeric
  interval rather than stopping" → red with `expected 'boolean' to be 'number'`.
- `use-seat-status/__tests__/useSeatStatus.test.tsx` — "should keep polling a running seat at the
  slow health cadence" → red with `expected "adminCall" to be called 2 times, but got 1 times`.
- `use-seat-reports/__tests__/useSeatReports.test.tsx` — "should slow to the health cadence once a
  seat is running, not stop" and "should back off a settled seat whose health reads start failing,
  without giving up" → both red the same way.

**Still open:** the `@live` rung-M2 spec that stops a guardian and asserts the chip changes with no
focus event belongs to W0.2, which has not landed. The unit tier is what this commit proves.
