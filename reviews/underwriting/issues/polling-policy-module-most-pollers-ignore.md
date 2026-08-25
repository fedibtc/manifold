# The polling-policy module says every poller must use it; most do not, and the transport owns no timeout

- **Status:** open
- **Tier:** checked (systemic)
- **Level:** code, at all sites
- **Found by:** scanner, checker (widened the site list), ops-drill
- **Where:**
  - policy: `.../shared/api/pollingIntervals.ts` ("All hooks that poll must pull their interval
    from here")
  - obeying: `useOnboarding`, `useSeatReports`, `useGuardianFees`
  - not obeying: `useAuthorizationWatch.ts:7` (3 s flat, relay-touching), `useSeatStatus.ts:19`
    (5 s flat then silence), `useSeats.ts:11`, `usePaymentFederations.ts:11` (30 s flat, no
    jitter, no decay)
  - retry axis unowned: `useSeats`, `usePaymentFederations`, `useOffer`, `useSeatStatus`,
    `useSeatReports` all keep the default `retry: 3`
  - `.../shared/api/authenticate.ts:17-22` — the only `fetch` in the app with no
    `AbortSignal.timeout`
  - `.../shared/api/adminCall.ts:41-47` — never forwards react-query's `signal`

**What happens:** A module was written to own polling cadence, jitter, decay and backoff, and it
states that requirement in its own header. Four of seven pollers ignore it and hardcode a flat
interval. The retry axis is unowned nearly everywhere, so "one poll" is up to four calls at five
sites. Two transport decisions are missing entirely: the sign-in `fetch` has no timeout, and
`adminCall` never reads the `AbortSignal` react-query hands it, so `#abortSignalConsumed` stays
false and query cancellation is a no-op.

**The result:** Cadence is unowned in practice, which is what let the 3-second relay watch and
the stop-forever seat detail both ship. The unowned `retry: 3` is also the mechanism that turns a
brief blip into a blank Overview. A black-holed sign-in spins until the browser gives up, on the
one screen with no other route forward. Unmounting the Seats page mid-fan-out leaves every
in-flight call running to completion and holding its slot — self-limiting at 15 s, but unowned.

**Failed defense:** "The policy module is available to hooks that want it." A policy module half
the callers ignore is not a policy, it is a utility with an inaccurate header — and the header is
what a future author reads before adding the eighth poller.

**Fix direction:** A poller declares a policy — `{ baseMs, healthyMs, ceilingMs, retry }` — and
cannot poll without one. The transport owns timeout and cancellation in one place: give
`authenticate` the same `AbortSignal.timeout` as `adminCall`, and forward the query `signal`
through `adminCall` (a two-line change).
