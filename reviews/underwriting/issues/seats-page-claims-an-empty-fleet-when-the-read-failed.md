# The Seats page tells an operator their fleet is empty whenever the seat list fails to load

- **Status:** fixed — `37dd6f70` (`fix/fman-ui-query-disposition`)
- **Tier:** checked + blinded convergence (2 roles)
- **Level:** code
- **Where:**
  - `operator-ui/apps/fleet-manager/src/features/seats/hooks/use-seat-rows/useSeatRows.ts:20,31-36`
  - `operator-ui/apps/fleet-manager/src/pages/seats/SeatsPage.tsx:9-21`
- **Found by:** scanner, checker, ops-drill

**What happens:** The row model drops `isPending` and `isError` and exposes only the rows. The
page branches on `isEmpty` first, so one branch serves three different facts: "the daemon
answered and you have no seats", "the daemon has not answered yet", and "the read failed". In
all three the operator is told, in a full sentence, that they have no seats yet and that this is
normal.

**The result:** An operator whose seat list is failing reads a positive, reassuring claim about
their fleet. The failure classes that land here are wider than they look: only `AuthError` is
promoted to the boot gate, so a 403, any 5xx, a protocol error and a transport error all render
"No seats yet". The pending case is every single visit to `/seats`. No test covers a failing
`ListSeats` — the page's test mocks only a resolved value.

**Failed defense:** "Rare, and the boot gate covers a dead daemon." It does not — the gate
promotes only 401. And the module already bans exactly this lie for money: it will not render a
balance it does not know, and counts unreadable seats rather than treating them as zero. The
same rule was simply never applied to inventory.

**Fix direction:** The row model carries the query state and the page distinguishes the three
cases. `WalletPage.tsx:23-25` already states the rule in a comment.

## Fix — `37dd6f70`

`useSeatRows` now narrows `isEmpty` to `seats.data !== undefined && allSeats.length === 0`, so the
empty claim needs an answer from the daemon before it may be made, and carries a `disposition`
from the shared `useQueryDisposition` primitive. `SeatsPage` renders its fleet through
`QuerySurface`, so an unanswered read shows "Loading…", a failed one shows the daemon's failure
with a **Try again** control, and a failed *refresh* keeps the seat table under a staleness
banner. Per-seat status stays out of the page disposition — a seat whose status call failed is
already carried by its own row and must not blank the list around it.

Covered by four unit tests in `pages/seats/__tests__/SeatsPage.test.tsx`, one per disposition.
Observed red against the pre-fix row model and page: all four failed, and the dumped DOM showed
"No seats yet. Seats are created by Federation Initiators…" on screen while `ListSeats` was
rejecting.

Still open from the parent issue: the `@live` spec that kills the daemon mid-session (needs the
Wave 0 live tier).
