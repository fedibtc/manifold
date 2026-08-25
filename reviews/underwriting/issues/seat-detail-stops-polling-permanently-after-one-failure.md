# Loading a seat's detail page during a blip leaves it permanently stuck on an error

- **Status:** fixed — `397bf011` (W3.4)
- **Tier:** checked + blinded convergence (2 roles)
- **Level:** code
- **Found by:** scanner, checker, ops-drill
- **Where:**
  - `operator-ui/apps/fleet-manager/src/features/seats/api/hooks/use-seat-status/useSeatStatus.ts:14-22`
  - `operator-ui/apps/fleet-manager/src/features/seats/api/hooks/use-seat-reports/useSeatReports.ts:45-59`
  - `operator-ui/apps/fleet-manager/src/pages/seat-detail/SeatDetailPage.tsx:22-33`

**What happens:** Two hooks write the same cache key with two different policies. The list hook
runs through the fan-out limiter, applies jittered per-seat backoff, and deliberately keeps
polling a seat whose report never arrived — with a written argument for why stopping would be
wrong. The detail hook has no limiter, no backoff, and returns `false` whenever `data` is
undefined: precisely the case its sibling argues must keep trying. After query-core's four
attempts, polling ends for good and the error screen offers only "Back to seats".

**The result:** A direct load or reload of `/seats/:id` during a transient failure leaves a
permanent error with no way forward but navigating away. Window refocus does recover it, but
nothing on screen says so. Meanwhile the same seat's row in the list keeps backing off and
retrying under the same key, so the two screens disagree about one seat.

**Failed defense:** "Different screens, different needs." Granted for the limiter — a detail
page fetching one seat does not need fan-out control. Not granted for the quiescent branch:
that is an oversight, and the module's own sibling hook says so in prose.

**Fix direction:** One hook, one policy, parameterized (`useSeatStatus(seatId, { fanOut })`),
with the list mapping over it — one cache key with one set of options. Give the detail page the
same Retry control the boot screen already has.

## Fix (`397bf011`)

`seatStatusQueryOptions(seatId, { fanOut })` in
`features/seats/api/hooks/use-seat-status/seatStatusQuery.ts` owns the key, the cadences and the
backoff. `useSeatStatus` is one line over it; `useSeatReports` maps it through `useQueries`. The
parameter sits on the options rather than on the hook because a hook cannot be called from a loop
over a list whose length changes — `fanOut` is the only thing a caller may vary, and it is the one
difference the issue grants. The detail error screen now says polling continues and offers Retry.

Covered by, and observed red against the restored pre-fix detail hook:

- `pages/seat-detail/__tests__/SeatDetailPage.test.tsx` — "should keep polling after a failed first
  read and recover with no reload" → red with `Unable to find an element with the text: fed1abc`.
- `use-seat-status/__tests__/useSeatStatus.test.tsx` — "should share one cache entry and one
  polling policy with the seats list" → red with
  `expected [ [Function _temp], …(1) ] to deeply equal [ …(2) ]`.
- `pages/seat-detail/__tests__/SeatDetailPage.test.tsx` — "should retry the read when the operator
  asks" → red with `Unable to find an accessible element with the role "button" and name "Retry"`
  once the control is removed.
