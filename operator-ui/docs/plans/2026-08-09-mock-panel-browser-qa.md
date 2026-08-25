# Browser QA: dev mock panel (FLIP + FMan)

Date: 2026-08-09
Branch: `feat/msw-mock-migration`
Method: real Chrome (Claude-in-Chrome extension), both Vite dev servers
(`.claude/launch.json` → `flip-ui` :5173, `fman-ui` :5174), MSW in-browser.
Executes Task 5 ("browser verification") of
[`2026-08-09-mock-panel-completion.md`](./2026-08-09-mock-panel-completion.md).

Every scenario was applied by **clicking the panel**, not by scripting, and the
result read back off the rendered page. Where a control has a measurable effect
(latency) it was measured rather than eyeballed.

## Result

All 24 scenarios render, and every panel capability works. Six defects found,
one of them significant. The plan's acceptance list passes except where noted
under [Dead ends](#dead-ends).

### Scenarios — 24/24

**FLIP (15/15)** — `setup-fresh`, `setup-pending`, `all-clear`, `funds-critical`,
`funds-warning`, `ad-stale`, `ad-withdrawn`, `ad-failed`, `ad-relays-mixed`,
`health-degraded`, `wallet-ops-broadcast-cancelled`, `wallet-ops-review`,
`allocations-mixed`, `allocations-action-required`, `allocations-cancelled`.

Each matched its documented `desc`, including the subtle ones: `funds-warning`
shows the amber banner while the balance stays healthy (UI reads
`replenishment`, not the raw numbers); `ad-relays-mixed` renders
`Failed · relay handshake rejected`; `wallet-ops-review` humanizes to
`in doubt` / `manual review required`; `ad-withdrawn` renders `—`, never `0`.

**FMan (9/9)** — `fresh-fleet`, `not-onboarded`, `awaiting-authorization`,
`seats-empty`, `seats-mixed`, `seat-unavailable`, `wallet-not-receivable`,
`offer-without-payments`, `earnings`.

`awaiting-authorization` renders but is indistinguishable from `fresh-fleet` —
see dead end 3.

### Panel capabilities

| Capability | FLIP | FMan | Note |
| --- | --- | --- | --- |
| Both tabs, per-page filtering | ✅ | ✅ | e.g. Setup "2 of 15", Overview "6 of 9" |
| Route relabel on navigation | ✅ | ✅ | incl. `/seats/:id` → "Seat detail" over "Seats" |
| Verb log populates from live traffic | ✅ | ✅ | clear → fetch → row reappears, no remount |
| Latency | ✅ 1500 → 1916 ms | ✅ 1200 → 1216 ms | measured |
| Error injection + "Clear errors" | ✅ | ✅ | header shows "N injected error(s)" |
| Patch state + JSON validation | ✅ | ✅ | bad value → "value must be JSON" |
| Copy debug state | ✅ | ✅ | button reads "Copied" |
| Reset mocks | ✅ | ✅ | scenario, latency, patches, errors all cleared |
| Panel survives a gate | ✅ restore console | ✅ setup wizard | the `77a049c7` fix, both apps |
| `bootMode: restore` | ✅ | n/a | **the plan's headline item — works** |
| `authMode: trusted_proxy` | n/a | ✅ | sign-in gate skipped after reload |
| State persists across reload | ✅ | ✅ | survived an unplanned server restart |

`bootMode: restore` renders the restore console with the panel still mounted and
switches back to `normal` — the capability that had no browser path at all
before this work.

## Dead ends

### 1. MSW silently stops intercepting; Express answers instead — **high**

The most important finding. Mid-session the FLIP panel appeared to break:
`__mockControl.getState()` reported the new world, the panel header re-rendered,
but the app never changed. The requests were being served by the **legacy Express
mock server on :8787**, reached through the Vite proxy:

```
x-powered-by: Express        // ← served by express, not MSW
served: not_configured       // stale world
__mockControl world: ready   // the world the panel is editing
```

`navigator.serviceWorker.controller` was still set and `__mockControl.active` was
still `true`, so nothing in the app or the panel indicated a problem. A page
reload restores interception.

Why it is bad: `vite.config.ts` proxies `/admin`, `/health` and `/__control` to
`http://localhost:8787`. When MSW stops intercepting, the request does not fail —
it is answered by a *different mock world*. The panel then edits a world nobody
is reading, which reads as "the panel is broken" rather than "mocking is off".

Suspected cause: Chrome evicting the MSW service worker after idle; on revival
the worker has no registered client and passes requests through. Not confirmed —
the fix below does not depend on the cause.

**Fix, in order of value**
1. Make the fallthrough loud: when `import.meta.env.VITE_MOCKS !== 'off'`, drop
   `/admin` and `/health` from `server.proxy` so an unintercepted call fails
   instead of being answered by express. This alone converts a silent wrong
   answer into an obvious one.
2. Show interception state in the panel header (a one-line probe: issue a
   request and check for a response header MSW sets). Cheap, and it makes the
   condition self-diagnosing.
3. Re-run the MSW handshake on `visibilitychange` / `serviceWorker.oncontrollerchange`.

This also sharpens the existing "stale-mock :8787" note: the hazard is not a
stale server, it is a *silent substitution*.

### 2. FLIP's `Daemon phase` control does nothing — **medium**

`MockState.phase` is written by `patchState` and read only by the panel's own
`read()`. No handler, verb or health response consults it. `grep -rn phase
apps/liquidity-provider/src` confirms: the remaining hits are unrelated local UI
state (save phase, wizard phase).

The comment calls it "daemon phase; gates deferred routes", but the gate does not
exist — `deferredRoutes` in `shared/api/errors.ts` is an **empty set** by design
("Empty until the next method is gated"), so `RouteDeferredError` is unreachable
too. Selecting 9, 10 or 11 changes state and nothing else.

**Fix**: either drop the control until a route is actually gated, or make phase
mean something — have `handlers.ts` return `unavailable` for a verb set below a
threshold phase, and add those verbs to `deferredRoutes` so the "not available
yet" screen becomes reachable. The second option is the one that buys test
coverage.

### 3. FMan `awaiting-authorization` is unobservable — **medium**

The scenario builds `onboarded: true` with
`onboarding.nostr.state = 'waiting_for_authorization'`, and its notes claim
`affects: ['setup', 'backup']`. In the browser it renders a normal healthy
Overview, and its Backup screen is **byte-identical** to `fresh-fleet`'s.

The nostr authorization state is only rendered by the onboarding wizard's
Authorization step, and that wizard is gate-rendered *only when
`onboarded: false`* — which this scenario is not. So the one thing the scenario
exists to show cannot be reached.

**Fix**: set `onboarded: false` in the builder (so the wizard renders and can
land on the Authorization step), or correct `affects` and the description to say
what it actually demonstrates.

### 4. A failed query renders as "you have nothing" — **medium**

Force `ListSeats` to fail, then load `/seats` cold under `seats-mixed`
(4 seats). The page renders:

> No seats yet. Seats are created by Federation Initiators after they pay for a
> plan…

An operator cannot tell a broken daemon from an empty fleet. The plan predicted
this; the browser confirms it. Two aggravating details found here:

- With cached data already present, injecting the error produces **no visible
  change at all** — the stale rows stay and the error is swallowed.
- From the cold error state, clearing the error did not refresh the screen in
  place within ~8 s; it recovered on navigating away and back. (Seen once; in the
  warm case clearing recovers immediately. Worth confirming before fixing.)

**Fix**: surface `isError` on the seats query distinctly from an empty list.
Pre-existing UI behaviour, out of scope for the panel work, but the panel is now
the cheapest way to reproduce it.

### 5. Patcher placeholder is FMan-specific in both apps — **low**

`StatePatcher.tsx` hardcodes `placeholder="seats.0.report.health"` and
`'"unavailable"'`. FLIP has no `seats` in its world, so its panel suggests a path
that cannot work. **Fix**: move the example pair into `PanelConfig` next to
`patch`.

### 6. Stale copy in FMan's seats empty state — **low**

"See **Plans** for the current offer" — Plans was removed in the MVP revision and
is not in the nav. **Fix**: point at the offer/price surface that replaced it.

### 7. Panel scroll resets to top on route navigation — **low**

Navigating relabels the panel and resets its scroll, so a control you were using
below the fold jumps away. Applying a *scenario* does **not** reset scroll —
verified directly (`scrollTop` held at 800 across a scenario change), correcting
an initial misread.

## Not covered

Stated so the gap is not mistaken for a pass:

- FLIP `/settings` and FMan `/wallet/:fedId/withdraw`, `/offer`, `/backup/phrase`
  were not walked individually in the browser; their route keys are covered by
  `apps/*/src/mocks/__tests__/routes.test.ts`.
- FLIP's `tick()` (deterministic republish) not exercised.
- Playwright suites not re-run in this session.

## Suggested order of work

1. **Dead end 1** — the only one that makes correct code look broken. Ship fix
   (1) at minimum; it is a few lines in `vite.config.ts`.
2. **Dead end 3** and **6** — one-line scenario/copy corrections.
3. **Dead end 2** — decide whether phase earns its control; remove or wire.
4. **Dead end 5**, **7** — panel polish.
5. **Dead end 4** — real UI defect, own ticket, outside the panel's scope.

## Environment note

Both Vite servers were already running at session start from an earlier terminal
and died mid-session (both at once, including the app not under test). They were
restarted from `.claude/launch.json` and behaved normally afterwards. Unrelated
to the panel; recorded only because it explains a gap in the session timeline.
