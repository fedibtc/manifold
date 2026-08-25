# Ask: fleet-level rollup verbs for the FMan operator API

Written for the FMan daemon owners. This records a client-side cost the
dashboard cannot remove on its own. Nothing here is implemented; it is a
request, and the dashboard works without it.

## The shape of the problem

Two dashboard screens need one fleet-wide answer, and the admin API offers only
per-seat ones.

| Screen | Wants | Has to use | Cost |
|---|---|---|---|
| Overview | one earnings rollup | `GuardianFees { seat_id }` per live seat | seats × 1 per 60 s |
| Seats | each row's phase and health | `SeatStatus { seat_id }` per live row | seats × 1 per 5 s while forming |

So load scales as dashboards × seats × time. A 50-seat fleet on the Seats page
during formation is 600 calls per minute per open dashboard. The daemon opens the
same per-seat state repeatedly to answer a single human question.

The bound that was supposed to contain this — "fleets are small" — is an
operator-supplied `u32`. Nothing enforces it.

## What the dashboard already does about it

These are mitigations, not fixes. They bound the burst; they do not remove the
N+1:

- **A concurrency limit** (`shared/api/requestLimit.ts`). At most
  `SEAT_FAN_OUT_LIMIT` calls of one kind are in flight. Nothing is dropped — a
  call over the bound waits its turn — so no total is ever quietly narrowed.
- **Jitter** on the poll intervals, so a whole fleet does not fire on one tick
  and restarted dashboards do not stay synchronised.
- **Per-seat backoff** while a seat keeps failing, decaying toward the healthy
  cadence and resetting on the first success.

## The ask

Any one of these removes the fan-out. They are listed in the order we would
find most useful, but the choice belongs to the daemon:

1. **A fleet-level fee/earnings summary** — one verb returning the rollup
   Overview draws, carrying per-seat unreadability metadata so the screen can
   still say *which* seats it could not read rather than folding them into a
   total. Overview does not need per-seat fee detail; it needs the sum plus the
   truth about its completeness.
2. **Live reports on `ListSeats`** — the Seats page needs each row's phase and
   health at list time. If `ListSeats` carried the report it already has to look
   up per seat, the second round of calls disappears.
3. **One snapshot/watch verb** — FMan's internal watch channels may already
   support this. A snapshot plus a change stream would replace both polls and
   remove the cadence tuning from the client entirely.

## What we need back, whichever is chosen

- **Unreadability must survive the aggregation.** The daemon's wallet projection
  reports null affected values together with closed `query_errors`, and permits
  `drain_state: drained` only after complete successful reads. A rollup verb
  must keep those distinctions per constituent, or the dashboard is forced
  back into inventing a number.
- **No silent partial totals.** If the summary is incomplete, say so in the
  response. The screens render an incomplete total as "—", never as a number.
