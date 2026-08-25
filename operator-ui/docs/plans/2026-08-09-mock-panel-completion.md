# Plan: complete the dev mock panel (spec §4)

Branch: `feat/msw-mock-migration`
Base: `77a049c7` (Plan B complete)
Date: 2026-08-09
Design: [`docs/msw-mock-migration.md`](../msw-mock-migration.md) §4

## Objective

Close two open items from Plan B's final review:

1. **Acceptance criterion "Dev panel has both tabs"** — the panel is a flat
   scenario list today. Spec §4 wants a Global tab and a This-page tab.
2. **The Plan C BLOCKER** — `/__control`'s `patch`, `errors` and `state` routes
   have no browser equivalent, so `latencyMs`, `forcedErrors`, `phase`,
   `bootMode` and `authMode` are reachable only by booting express. In
   particular `bootMode: 'restore'` has no browser path at all, which makes
   FLIP's restore console unreachable in mock mode.

## Non-goals

- Form-fixture click-to-copy chips (spec §9 / Phase 8).
- Deleting express (Phase 9).
- Any other Plan C carry item (shared-package dedupe, `?error=` cast, README,
  msw pin, `E2E_TARGET=daemon` run).

## Decisions taken during design

**The per-page verb list is derived at runtime, not from a hand-written map.**
Spec §4 says the This-page tab shows "the verbs this page calls". A static
route→verbs table is prose about what a page queries: nothing type-checks it,
and it lies silently the day a page adds a query. Instead MSW records each verb
it serves, stamped with the route key that was showing at the time, and the tab
lists what was actually observed. Self-maintaining; the cost is that the list is
empty until the page's first fetch resolves, covered by an explicit
"Listening for this page's calls…" state.

**Error values are a select in both apps.** FLIP already selects a
`ServiceErrorCode`. FMan's express panel had a free-text message field; it
becomes a small canned list so one `VerbErrorList` component serves both tabs in
both apps. Arbitrary FMan messages stay reachable via
`window.__mockControl.setError(verb, message)`.

**The raw-MockState textarea is dropped; the path/value patcher is kept.** The
patcher is the escape hatch for anything the typed knobs miss. A whole-world JSON
editor is redundant now that the world is a localStorage blob a developer can
edit in devtools directly.

## Findings that shape the work

**`persist()` does not notify.** `scenario-store.ts` defines `persist: write`.
Control knobs write through `patchState` → `persist()`, so nothing re-renders and
`MockPanelMount`'s `invalidateQueries` never fires — changing latency or phase
would appear to do nothing. The store needs a separate notify path, kept separate
so that mutating *verbs* keep today's write-only behaviour and no e2e moves.

**FLIP mounts the panel below its boot gate.** `AppShell.tsx` renders
`<MockPanelMount />`, and `BootGate.tsx` renders `RestoreConsolePage` *instead
of* `<Outlet />` in restore mode. The moment the new `bootMode` control does its
job the panel disappears with no way back. FLIP needs the `RootLayout` treatment
FMan already got in `77a049c7`. This is a precondition of shipping the control,
not a drive-by.

**`GET /health` does not consult `forcedErrors`.** Only dispatched admin methods
are injectable, so only dispatched methods are recorded in the verb log.

**Both apps already export `adminMethods = Object.keys(verbs)`** from
`world/verbs.ts`, derived from the dispatch table. That is the Global tab's verb
list — it cannot drift from what is routed.

## Architecture

```
  packages/mock-devtools  (generic — knows nothing of authMode/phase/bootMode)
  ┌──────────────────────────────────────────────────────────────┐
  │ MockPanel            launcher + tab bar + Reset              │
  │  ├ PageTab           ScenarioList(filtered) + VerbErrorList  │
  │  │                                        (observed verbs)   │
  │  └ GlobalTab         ScenarioList(all) + ControlField[]      │
  │                      + VerbErrorList(all) + StatePatcher     │
  └────────────▲─────────────────────────────────────────────────┘
               │ props: routeKey, catalog, config, verbLog
  ┌────────────┴─────────────────────────────────────────────────┐
  │ apps/<app>/src/mocks/                                        │
  │   routes.ts        pathname → RouteKey (ordered patterns)    │
  │   verb-log.ts      createVerbLog(() => routeToKey(pathname)) │
  │   panel-config.ts  controls / errors / patch → patchState    │
  └──────────────────────────────────────────────────────────────┘
```

The package stays app-agnostic by taking descriptors rather than fields. FMan
supplies `latencyMs` + `authMode`; FLIP supplies `latencyMs` + `phase` +
`bootMode`.

**Production safety is unchanged.** `MockPanelMount` calls `useLocation()`
itself and passes `pathname` *into* the lazy-bound panel; `routes.ts`,
`verb-log.ts` and `panel-config.ts` are imported only inside the lazy factory,
behind the existing module-scope `mocksEnabled` gate. No new top-level
`@/mocks` import.

## Tasks

### Task 1 — store: separate notify from persist

`packages/mock-devtools/src/scenario-store.ts`, `types.ts`.

- Add a monotonic `revision`, bumped by the existing internal `notify()`.
- Expose `notify(): void` (fire listeners, no write) and `getRevision(): number`
  on `ScenarioStore`. `persist()` stays write-only.
- Add `use-mock-revision/useMockRevision.ts` — `useSyncExternalStore` over
  `getRevision`, so panel fields that read live world values re-render.
  `useScenario`'s snapshot is the scenario *string*, which does not change when
  a control does.

Verify: existing 27 mock-devtools tests stay green; new tests cover that
`persist()` does not notify and `notify()` bumps the revision.

### Task 2 — verb log

`packages/mock-devtools/src/verb-log.ts`, `use-verb-log/useVerbLog.ts`.

```ts
createVerbLog(getRouteKey: () => string): VerbLog
  record(verb)              // stamps the current route key
  list(routeKey)            // insertion order, deduped, stable reference
  clear(routeKey)
  subscribe(listener)       // fires only when a verb is NEW for that key
```

Two constraints, both load-bearing:

- **Stamp the route key rather than clearing on navigation.** Clearing in an
  effect races the page's own fetches. Stamping removes the race and keeps the
  list when you walk back to a page.
- **`list()` must return a stable reference** for `useSyncExternalStore`. Back it
  with `Map<routeKey, string[]>`, replace the array only on insert, and return a
  single frozen `EMPTY` for unknown keys.
- **`subscribe` fires only on a genuinely new verb.** Polling verbs
  (`use-authorization-watch`, seat reports) would otherwise re-render the panel
  on a timer.

Verify: unit tests for dedupe, reference stability, per-key isolation, and that
a repeat `record` does not notify.

### Task 3 — panel components

`packages/mock-devtools/src/`. One component per kebab folder with a colocated
module and `__tests__`, per the repo's folder rules.

| Folder | Component |
| --- | --- |
| `mock-panel/` | shell: launcher, tab bar, active tab, Reset (exists — rewritten) |
| `page-tab/` | filtered `ScenarioList` + observed `VerbErrorList` |
| `global-tab/` | full `ScenarioList` + `ControlField[]` + full `VerbErrorList` + `StatePatcher` |
| `scenario-list/` | `entries` + `activeName` → `ScenarioToggle` rows |
| `verb-error-list/` | verb rows, each a code select; `emptyLabel` for the listening state |
| `control-field/` | one knob (number or select), uncontrolled + Apply |
| `state-patcher/` | path + JSON value + Apply |
| `scenario-toggle/` | unchanged |

Tab behaviour:

- Label reads `This page: <RouteKey>`.
- Default tab is This-page when the route key resolves and at least one scenario
  affects it; otherwise Global. This is also the graceful fallback for FMan's
  `setup`/`auth` and FLIP's `restore-console`, which are gate-rendered and have
  no pathname of their own.
- The verb section's empty state is a spinner plus "Listening for this page's
  calls…" — one state, no timeout constant. It stays true however long it sits
  there, so a page that fetches nothing does not read as a hung panel.

Styling per `.claude/rules/tailwind-css.md`: no Tailwind strings in TSX, bare
`@apply` in colocated modules, preset tokens only.

Verify: `mock-devtools` unit suite; `check_against_system` on the new UI.

### Task 4 — app wiring, both apps

Per app, under `src/mocks/`:

- `routes.ts` — `routeToKey(pathname)`. Ordered patterns so `/seats/:id` →
  `seat-detail` wins over `/seats` → `seats`.
  - FMan: `/` overview · `/seats` seats · `/seats/:id` seat-detail · `/wallet`
    wallet · `/wallet/:fedId/withdraw` wallet-withdraw · `/offer` offer ·
    `/backup` backup · `/backup/phrase` backup-phrase
  - FLIP: `/` overview · `/setup` setup · `/funds` funds · `/allocations`
    allocations · `/advertisement` advertisement · `/settings` settings
- `verb-log.ts` — `createVerbLog(() => routeToKey(window.location.pathname))`.
- `panel-config.ts` — controls, error injection (`adminMethods` + code list),
  and `patch` wired to `patchState`.
- `handlers.ts` — one `verbLog.record(method)` line after the verb resolves.
- `state.ts` — `patchState` / `setState` / `tick` call `persist()` **and**
  `notify()`. Verb dispatch keeps `persist()` alone.
- `start.ts` — `window.__mockControl` gains `patch`, `setError`, `setState`
  (and FLIP's `tick`). Additive; no e2e call site changes.
- `MockPanelMount.tsx` — `useLocation()` at mount level, `pathname` passed into
  the lazy-bound panel.

FLIP only: new `src/app/components/root-layout/RootLayout.tsx` (`Outlet` +
`MockPanelMount`) as the router's top-level element; remove `MockPanelMount`
from `AppShell.tsx`.

Verify: both unit suites, five typechecks, `dist/` mock-marker grep, FMan e2e
35/35, FLIP e2e 18/18 — all unchanged.

### Task 5 — browser verification

- FLIP: set `bootMode: restore` from the Global tab → restore console renders →
  **panel is still present** → set back to `normal`.
- FMan: inject an error on `ListSeats` → the Seats page shows it with no reload.
- Both: walk the routes and confirm the This-page tab relabels, refilters, and
  populates its verb list from live traffic.

## Found during execution

**The React Compiler defeats every memo-based fix.** The panel renders from a world
that is mutated in place, so it is not a pure function of its props — the
compiler's central assumption. Measured, not assumed, with four probe variants:

| Shape | Result |
| --- | --- |
| `useMemo(() => obj.read(), [obj, rev])` | **stale** — `rev` is dropped, it is not read inside |
| `useMemo(() => ({ rev, v: obj.read() }), [rev])` | **stale** — the inner `obj.read()` is cached on its own |
| `read()` behind a stable `useCallback` | **stale** — a call on a stable callee is cached |
| `readAt(rev)` — revision as an *argument* | works, at the cost of a parameter nobody reads |
| `'use no memo'` | works |

`'use no memo'` on `PageTab` and `GlobalTab` is the answer, and the opt-out is
**permanent** rather than the TODO the repo's React Compiler rules describe.
The alternative — an immutable mock world — is a larger change than this panel,
and memoizing a dev-only drawer buys nothing.

**`mock-devtools` was not running the compiler in tests.** Both apps build with
`babel-plugin-react-compiler` via their vite config, but the package's
`vitest.config.ts` had no React plugin, so its tests exercised a component that
re-renders far more eagerly than the shipped one — they passed against the
broken code. The plugin is now wired into that config; the four regression tests
fail without the directive and pass with it.

**`active()` must return a fresh object.** The world's `forcedErrors` is mutated
in place, so handing it back directly gives every reader an identity that never
changes. Pinned in the `ErrorInjection` doc comment and done in both apps.

**FMan's Seats page shows its empty state, not an error, when `ListSeats` fails.**
Pre-existing UI behaviour that the new injection control merely makes easy to
reach. Not changed here.

## Acceptance

Browser verification (Task 5) was run in real Chrome on 2026-08-09 — results and
defects in [`2026-08-09-mock-panel-browser-qa.md`](./2026-08-09-mock-panel-browser-qa.md).

- [x] Panel has both tabs; the This-page tab filters scenarios on every route in
      both apps. *(Walked in-browser on FLIP setup/overview/funds/advertisement/
      allocations and FMan overview/seats/seat-detail/wallet/backup; the
      remaining route keys are covered by `mocks/__tests__/routes.test.ts`.)*
- [x] Every `/__control` capability except the raw-state textarea is reachable
      from the browser, by hand (panel) and by script (`__mockControl`).
      *(Caveat: FLIP's `phase` knob writes state but nothing reads it — QA
      dead end 2.)*
- [x] `bootMode: 'restore'` is reachable and the panel survives it.
- [x] Control changes refresh the screen with no manual reload. *(Except when
      MSW has silently stopped intercepting and express on :8787 is answering —
      QA dead end 1, the one blocking issue.)*
- [ ] No new mock markers in either `dist/`.
- [ ] Unit suites, typechecks and both e2e suites green and unchanged.
