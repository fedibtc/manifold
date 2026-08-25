# MSW Migration — Plan A: foundation + FMan cutover

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run the FMan operator dashboard fully mocked in the browser via MSW, with a persisted scenario store and a dev control panel, so `pnpm fman:be` is no longer needed for FMan development or e2e.

**Architecture:** The FMan mock world (state, scenarios, verb handlers) moves out of `apps/fleet-manager/mock-server/` into `apps/fleet-manager/src/mocks/` as transport-agnostic code. Express keeps working by importing it during the migration window, so parity is structural rather than eyeballed. A new shared package `@operator-ui/mock-devtools` owns the scenario store, its localStorage persistence, and the control panel; MSW handlers read the store synchronously and write it back after mutating verbs.

**Tech Stack:** msw 2.15.0, Vite 6, React 19, TanStack Query 5, Vitest 2, Playwright 1.49, pnpm workspaces.

**Source spec:** [`operator-ui/docs/msw-mock-migration.md`](../msw-mock-migration.md). Plan A covers spec phases 1–4. Plans B (FLIP, phases 5–6) and C (per-page tab, form fixtures, teardown; phases 7–9) follow.

## Global Constraints

- All paths are relative to `operator-ui/` unless stated otherwise. Work on branch `feat/msw-mock-migration`.
- **Do not modify** `adminCall.ts`, `authenticate.ts`, `errors.ts`, `queryClient.ts`, or any hook/component under `src/features` or `src/pages`. Nothing above `fetch` changes.
- **Do not delete** `apps/fleet-manager/mock-server/` in this plan. It is deleted in Plan C, after FLIP has migrated too.
- Component rules (`operator-ui/CLAUDE.md`): arrow-function consts with named exports, no `React.FC`, a declared `XProps` interface per component, no inline logic in JSX, one React unit per file, kebab-case component folders with a PascalCase file and a `__tests__/` sibling.
- Styling: no Tailwind strings in TSX. `className` holds only `styles.*` refs from a colocated `.module.css` using bare top-level `@apply`.
- Tests: Vitest, every `it` description starts with `should`. Restore any overridden global in `afterEach`.
- The store schema version constant is `STORE_VERSION = 1`. localStorage keys are `operator-ui:dev:mocks:fman` and `operator-ui:dev:mocks:flip`.
- Pre-commit hook: this checkout lacks `typos`/`shellcheck`/`treefmt` on PATH. If the hook fails on a file you did not touch, commit with `--no-verify` and say so; never edit an unrelated file to satisfy it.
- `pnpm --filter fman test` must pass at the end of every task that touches app code. `E2E_APP=fman pnpm test:e2e` is the regression gate for tasks 1, 2, 5 and 6.

---

### Task 1: Bootstrap MSW in FMan and prove one interception

**Files:**
- Modify: `apps/fleet-manager/package.json` (add `msw` devDependency)
- Create: `apps/fleet-manager/public/mockServiceWorker.js` (generated, do not hand-edit)
- Create: `apps/fleet-manager/src/mocks/browser.ts`
- Create: `apps/fleet-manager/src/mocks/start.ts`
- Modify: `apps/fleet-manager/src/app/index.tsx`
- Modify: `agent-toolkit.json` (add the mocks dir to the `app` boundary layer)
- Test: `e2e/fman/msw-bootstrap.spec.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `startMocks(): Promise<void>` from `@/mocks/start`; `worker` from `@/mocks/browser`; a global `window.__mockControl` object whose only member so far is `active: true`. Tasks 5 and 6 extend `__mockControl`.

- [ ] **Step 1: Add msw and generate the service worker**

```bash
pnpm --filter fman add -D msw@^2.15.0
pnpm --filter fman exec msw init public --save
```

`msw init` writes `apps/fleet-manager/public/mockServiceWorker.js` and records the path in `package.json` under a `msw.workerDirectory` key. Commit the generated file — it must be served at the app origin.

- [ ] **Step 2: Write the failing e2e test**

Create `e2e/fman/msw-bootstrap.spec.ts`:

```ts
import { expect, test } from '@playwright/test';

// Tracer bullet for the MSW layer: proves the worker starts before the app
// renders, so no request can escape to the network unmocked.
test('should expose the mock control surface once MSW has started', async ({ page }) => {
  await page.goto('/');
  await expect
    .poll(() => page.evaluate(() => Boolean((window as { __mockControl?: unknown }).__mockControl)))
    .toBe(true);
});
```

- [ ] **Step 3: Run it to verify it fails**

Run: `E2E_APP=fman pnpm test:e2e -- msw-bootstrap`
Expected: FAIL — the poll never becomes `true` because `__mockControl` is undefined.

- [ ] **Step 4: Create the worker module**

Create `apps/fleet-manager/src/mocks/browser.ts`:

```ts
import { setupWorker } from 'msw/browser';

// Handlers arrive in Task 5. An empty worker still proves the registration
// path: anything unmatched falls through to the vite proxy, so the app keeps
// working against express while the two transports overlap.
export const worker = setupWorker();
```

- [ ] **Step 5: Create the dev-only start helper**

Create `apps/fleet-manager/src/mocks/start.ts`:

```ts
declare global {
  interface Window {
    __mockControl?: { active: boolean };
  }
}

export const startMocks = async (): Promise<void> => {
  const { worker } = await import('@/mocks/browser');
  await worker.start({
    // A missed endpoint should be loud. Vite's own asset and HMR requests are
    // same-origin, so 'warn' rather than 'error' keeps the console usable.
    onUnhandledRequest: 'warn',
    serviceWorker: { url: '/mockServiceWorker.js' }
  });
  window.__mockControl = { active: true };
};
```

- [ ] **Step 6: Start the worker before the app renders**

In `apps/fleet-manager/src/app/index.tsx`, replace the final `createRoot(...).render(...)` call with:

```tsx
const root = document.getElementById('root');
if (!root) throw new Error('missing #root element');

const render = () => {
  createRoot(root).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </StrictMode>
  );
};

// The worker must be listening before the first query fires, so rendering is
// deferred until it is. The dynamic import behind a statically-analysable guard
// is what keeps the whole mock subtree out of production bundles.
if (import.meta.env.DEV && import.meta.env.VITE_MOCKS !== 'off') {
  const { startMocks } = await import('@/mocks/start');
  await startMocks();
}

render();
```

Top-level `await` requires the module to be ESM, which it is (`"type": "module"` plus Vite's ESM output).

- [ ] **Step 7: Register the mocks dir with the boundaries linter**

In `agent-toolkit.json`, extend the `app` layer so `src/mocks` is linted as app-level code — importable by the entry point, importable by nothing else:

```json
  "boundaries": {
    "shared": ["packages/*", "apps/*/src/shared"],
    "feature": ["apps/*/src/features/*"],
    "composition": ["apps/*/src/pages/*"],
    "app": ["apps/*/src/app", "apps/*/src/mocks"]
  },
```

- [ ] **Step 8: Run the test to verify it passes**

Run: `E2E_APP=fman pnpm test:e2e -- msw-bootstrap`
Expected: PASS.

Then confirm nothing regressed and the lint gates are clean:

```bash
E2E_APP=fman pnpm test:e2e
pnpm --filter fman typecheck
pnpm lint:boundaries
```

- [ ] **Step 9: Verify the production bundle is clean**

```bash
pnpm --filter fman build
grep -r "__mockControl\|mockServiceWorker" apps/fleet-manager/dist/assets/ || echo "CLEAN"
```

Expected: `CLEAN`. If anything matches, the dynamic import is being statically pulled in — check that no module imports `@/mocks/*` at the top level.

- [ ] **Step 10: Commit**

```bash
git add apps/fleet-manager/package.json apps/fleet-manager/public/mockServiceWorker.js \
        apps/fleet-manager/src/mocks apps/fleet-manager/src/app/index.tsx \
        agent-toolkit.json e2e/fman/msw-bootstrap.spec.ts pnpm-lock.yaml
git commit -m "feat(fman): start MSW in dev before the app renders"
```

---

### Task 2: Move the FMan mock world into the app, with express still driving it

**Files:**
- Create: `apps/fleet-manager/src/mocks/state.ts` (moved from `mock-server/src/state.ts`)
- Create: `apps/fleet-manager/src/mocks/scenarios.ts` (moved from `mock-server/src/scenarios.ts`)
- Create: `apps/fleet-manager/src/mocks/world/verbs.ts` (extracted from `mock-server/src/routes/admin.ts`)
- Modify: `apps/fleet-manager/mock-server/src/routes/admin.ts`
- Modify: `apps/fleet-manager/mock-server/src/index.ts`, `middleware.ts`, `routes/control.ts`, `routes/auth.ts` (import paths only)
- Delete: `apps/fleet-manager/mock-server/src/state.ts`, `apps/fleet-manager/mock-server/src/scenarios.ts`
- Modify: `apps/fleet-manager/mock-server/tsconfig.json` (allow reaching into the app's `src`)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - From `@/mocks/state`: `MockState`, `MockSeat`, `MockGuardianFees`, `PatchInput`, `getState()`, `setState(next)`, `patchState(patch)`, `resetState(name?)` — all unchanged from the express originals, plus a new `sessionActive: boolean` field on `MockState`.
  - From `@/mocks/scenarios`: `ScenarioName`, `ScenarioNote`, `scenarioNames`, `scenarioCatalog`, `hasScenario(name)`, `scenario(name)` — unchanged.
  - From `@/mocks/world/verbs`: `verbs: Record<string, Verb>`, `MUTATING_VERBS: ReadonlySet<string>`, `adminMethods: string[]`, `parseRequest(body)`, `dispatch(body)`.
  - `type Verb = (payload: unknown) => unknown` — throws `Error` on failure; the caller maps that to `{ Err: message }`.

This is a **pure move plus one extraction**. No behaviour changes. The regression gate is the existing e2e suite passing unchanged.

- [ ] **Step 1: Move state and scenarios verbatim**

```bash
git mv apps/fleet-manager/mock-server/src/state.ts apps/fleet-manager/src/mocks/state.ts
git mv apps/fleet-manager/mock-server/src/scenarios.ts apps/fleet-manager/src/mocks/scenarios.ts
```

In `src/mocks/state.ts` change the scenarios import to the app alias:

```ts
import { scenario } from '@/mocks/scenarios';
```

In `src/mocks/scenarios.ts`:

```ts
import type { MockGuardianFees, MockSeat, MockState } from '@/mocks/state';
```

- [ ] **Step 2: Add the session flag to MockState**

The express server keeps `sessionActive` in a module-level variable in `middleware.ts`, which means a browser refresh keeps you logged in. In the browser that variable dies on reload, so it moves into the persisted world instead.

In `src/mocks/state.ts`, add to the `MockState` interface, after `authMode`:

```ts
  /** Whether a password login has succeeded. Lives in state (not a module
   *  variable) so it persists with the world and a refresh does not bounce the
   *  operator back to the login screen. */
  sessionActive: boolean;
```

In `src/mocks/scenarios.ts`, add `sessionActive: false` to the object returned by `base()`, next to `authMode: 'password'`.

- [ ] **Step 3: Extract the verb map out of the express route**

Create `apps/fleet-manager/src/mocks/world/verbs.ts`. Move the whole body of `mock-server/src/routes/admin.ts` across **except** the `adminRouter` declaration and the final `adminRouter.post` handler. Change the state import to `@/mocks/state`, and export the dispatch surface:

```ts
export type Verb = (payload: unknown) => unknown;

// ... err/ok/seatById/feeLedger/listSeats/... all move here verbatim ...

export const verbs: Record<string, Verb> = { ...fleetHandlers, ...onboardingHandlers };

/** Verbs that change the world. The store persists only after these, so polling
 *  reads (SeatStatus, Onboarding) do not serialise the world on every tick. */
export const MUTATING_VERBS: ReadonlySet<string> = new Set([
  'DecommissionSeat',
  'SetPrice',
  'Withdraw',
  'CollectGuardianFees',
  'WithdrawGuardianFees',
  'OnboardAsNew',
  'OnboardFromBackup'
]);

export const adminMethods = Object.keys(verbs);

export const parseRequest = (body: unknown): { method: string; payload: unknown } => {
  if (typeof body === 'string') return { method: body, payload: undefined };
  const method = Object.keys(body as object)[0];
  return { method, payload: (body as Record<string, unknown>)[method] };
};

export const isOnboardingVerb = (method: string): boolean => method in onboardingHandlers;
```

- [ ] **Step 4: Add the shared dispatcher**

Both transports need identical gating (forced errors, the not-onboarded refusal, error-to-`Err` mapping). Put it in the same file so neither transport can drift:

```ts
import type { AdminResult } from '@operator-ui/types';
import { getState } from '@/mocks/state';

export const dispatch = (body: unknown): AdminResult<unknown> => {
  const { method, payload } = parseRequest(body);

  const forced = getState().forcedErrors[method];
  if (forced) return { Err: forced };

  const verb = verbs[method];
  if (!verb) return { Err: `unparsable admin request: unknown variant ${method}` };

  // An un-onboarded host has no fleet, so every other verb says so rather than
  // inventing an empty one (crates/fman/core/src/onboarding.rs::dispatch).
  if (!getState().onboarded && !isOnboardingVerb(method)) {
    return {
      Err: 'this Fleet Manager has not been onboarded yet: run `admin onboard new` or `admin onboard restore`'
    };
  }

  try {
    return { Ok: verb(payload) };
  } catch (error) {
    return { Err: error instanceof Error ? error.message : String(error) };
  }
};
```

- [ ] **Step 5: Reduce the express route to a transport shim**

`mock-server/src/routes/admin.ts` becomes only:

```ts
import { type Request, type Response, Router, type Router as RouterType } from 'express';
import { adminMethods, dispatch } from '../../src/mocks/world/verbs';

export const adminRouter: RouterType = Router();

export { adminMethods };

adminRouter.post('/', (req: Request, res: Response) => {
  res.json(dispatch(req.body));
});
```

Update the remaining mock-server imports of `./state` and `./scenarios` to `../../src/mocks/state` and `../../src/mocks/scenarios` (they appear in `index.ts`, `middleware.ts`, `routes/control.ts`, `routes/auth.ts`).

- [ ] **Step 6: Point express's session at state**

In `mock-server/src/middleware.ts`, delete the module-level `let sessionActive = false;` and rewrite the three functions that used it to read and write `getState().sessionActive` instead:

```ts
export const startSession = (): void => {
  getState().sessionActive = true;
};

export const clearSession = (): void => {
  getState().sessionActive = false;
};
```

and in `requireSession`, replace `sessionActive &&` with `getState().sessionActive &&`.

- [ ] **Step 7: Let the mock-server tsconfig see the app source**

In `apps/fleet-manager/mock-server/tsconfig.json`, add the alias so `@/mocks/*` resolves for `tsx` and `tsc`:

```json
  "compilerOptions": {
    "baseUrl": ".",
    "paths": { "@/*": ["../src/*"] }
  },
  "include": ["src", "../src/mocks"]
```

`tsx` honours tsconfig paths at runtime, so `pnpm fman:be` keeps working.

- [ ] **Step 8: Run the full regression gate**

```bash
pnpm --filter fman typecheck
pnpm --filter fman-mock-server typecheck
pnpm --filter fman test
E2E_APP=fman pnpm test:e2e
pnpm lint:boundaries
```

Expected: all pass, with **no spec file edited**. That is the whole point of this task — the move is invisible to the tests. If a spec needed changing, something moved that should not have.

- [ ] **Step 9: Commit**

```bash
git add -A apps/fleet-manager agent-toolkit.json
git commit -m "refactor(fman): lift the mock world out of the express server"
```

---

### Task 3: Scenario store with localStorage persistence

**Files:**
- Create: `packages/mock-devtools/package.json`
- Create: `packages/mock-devtools/tsconfig.json`
- Create: `packages/mock-devtools/vitest.config.ts`
- Create: `packages/mock-devtools/src/index.ts`
- Create: `packages/mock-devtools/src/types.ts`
- Create: `packages/mock-devtools/src/storage.ts`
- Create: `packages/mock-devtools/src/scenario-store.ts`
- Test: `packages/mock-devtools/src/__tests__/scenarioStore.test.ts`

**Interfaces:**
- Consumes: nothing. This package is generic over the world type `W` and knows nothing about FMan or FLIP.
- Produces, from `@operator-ui/mock-devtools`:
  - `interface StorageAdapter { load(key: string): string | null; save(key: string, value: string): void; clear(key: string): void }`
  - `localStorageAdapter: StorageAdapter`
  - `interface WorldSource<W> { appKey: string; defaultScenario: string; has(name: string): boolean; build(name: string): W }`
  - `interface ScenarioStore<W> { getWorld(): W; getScenario(): string; setScenario(name: string): void; reset(): void; persist(): void; subscribe(listener: () => void): () => void }`
  - `createScenarioStore<W>(source: WorldSource<W>, storage?: StorageAdapter): ScenarioStore<W>`
  - `STORE_VERSION: number`

- [ ] **Step 1: Scaffold the package**

Create `packages/mock-devtools/package.json` (mirrors `packages/shared-ui`, whose folder name and package name deliberately differ — here they match):

```json
{
  "name": "@operator-ui/mock-devtools",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "main": "./src/index.ts",
  "types": "./src/index.ts",
  "exports": { ".": "./src/index.ts" },
  "scripts": {
    "test": "vitest run",
    "typecheck": "tsc --noEmit"
  },
  "peerDependencies": {
    "react": "^19.0.0",
    "react-dom": "^19.0.0"
  },
  "devDependencies": {
    "@testing-library/jest-dom": "^6.6.3",
    "@testing-library/react": "^16.1.0",
    "@types/react": "^19.0.7",
    "@types/react-dom": "^19.0.3",
    "jsdom": "^25.0.1",
    "react": "^19.0.0",
    "react-dom": "^19.0.0",
    "vitest": "^2.1.8"
  }
}
```

Copy `packages/shared-ui/tsconfig.json` to `packages/mock-devtools/tsconfig.json` unchanged. Create `packages/mock-devtools/vitest.config.ts`:

```ts
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: { globals: true, environment: 'jsdom' }
});
```

Then `pnpm install` from `operator-ui/` to link the workspace package.

- [ ] **Step 2: Write the failing tests**

Create `packages/mock-devtools/src/__tests__/scenarioStore.test.ts`:

```ts
import type { StorageAdapter, WorldSource } from '../types';
import { createScenarioStore, STORE_VERSION } from '../scenario-store';

interface TestWorld {
  seats: string[];
  price: number | null;
}

const source: WorldSource<TestWorld> = {
  appKey: 'test',
  defaultScenario: 'empty',
  has: (name) => name === 'empty' || name === 'populated',
  build: (name) =>
    name === 'populated' ? { seats: ['seat-1'], price: 50 } : { seats: [], price: null }
};

const memoryStorage = (seed: Record<string, string> = {}): StorageAdapter & {
  contents: Record<string, string>;
} => {
  const contents = { ...seed };
  return {
    contents,
    load: (key) => contents[key] ?? null,
    save: (key, value) => {
      contents[key] = value;
    },
    clear: (key) => {
      delete contents[key];
    }
  };
};

const KEY = 'operator-ui:dev:mocks:test';

it('should build the default scenario when nothing is persisted', () => {
  const store = createScenarioStore(source, memoryStorage());

  expect(store.getScenario()).toBe('empty');
  expect(store.getWorld().seats).toEqual([]);
});

it('should rehydrate a persisted world including its mutations', () => {
  const storage = memoryStorage({
    [KEY]: JSON.stringify({
      v: STORE_VERSION,
      scenario: 'populated',
      world: { seats: ['seat-1', 'seat-mutated'], price: 99 }
    })
  });

  const store = createScenarioStore(source, storage);

  expect(store.getScenario()).toBe('populated');
  expect(store.getWorld().seats).toEqual(['seat-1', 'seat-mutated']);
  expect(store.getWorld().price).toBe(99);
});

it('should write the mutated world back when persist is called', () => {
  const storage = memoryStorage();
  const store = createScenarioStore(source, storage);

  store.getWorld().seats.push('seat-added');
  store.persist();

  expect(JSON.parse(storage.contents[KEY]).world.seats).toEqual(['seat-added']);
});

it('should discard a world persisted under a different store version', () => {
  const storage = memoryStorage({
    [KEY]: JSON.stringify({
      v: STORE_VERSION + 1,
      scenario: 'populated',
      world: { seats: ['stale'], price: 1 }
    })
  });

  const store = createScenarioStore(source, storage);

  expect(store.getScenario()).toBe('empty');
  expect(store.getWorld().seats).toEqual([]);
});

it('should discard a world whose scenario name is no longer known', () => {
  const storage = memoryStorage({
    [KEY]: JSON.stringify({
      v: STORE_VERSION,
      scenario: 'deleted-scenario',
      world: { seats: ['stale'], price: 1 }
    })
  });

  const store = createScenarioStore(source, storage);

  expect(store.getScenario()).toBe('empty');
});

it('should fall back to the default when the persisted blob is not valid JSON', () => {
  const store = createScenarioStore(source, memoryStorage({ [KEY]: '{not json' }));

  expect(store.getScenario()).toBe('empty');
});

it('should rebuild from the named scenario and drop earlier mutations on switch', () => {
  const storage = memoryStorage();
  const store = createScenarioStore(source, storage);
  store.getWorld().seats.push('seat-added');
  store.persist();

  store.setScenario('populated');

  expect(store.getWorld().seats).toEqual(['seat-1']);
  expect(JSON.parse(storage.contents[KEY]).scenario).toBe('populated');
});

it('should return to the default scenario on reset', () => {
  const store = createScenarioStore(source, memoryStorage());
  store.setScenario('populated');

  store.reset();

  expect(store.getScenario()).toBe('empty');
  expect(store.getWorld().seats).toEqual([]);
});

it('should notify subscribers when the scenario changes', () => {
  const store = createScenarioStore(source, memoryStorage());
  let calls = 0;
  const unsubscribe = store.subscribe(() => {
    calls += 1;
  });

  store.setScenario('populated');
  unsubscribe();
  store.setScenario('empty');

  expect(calls).toBe(1);
});
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `pnpm --filter @operator-ui/mock-devtools test`
Expected: FAIL — cannot resolve `../scenario-store`.

- [ ] **Step 4: Write the types**

Create `packages/mock-devtools/src/types.ts`:

```ts
export type RouteKey = string;

export interface ScenarioNote {
  /** What this scenario puts the mock world into. */
  desc: string;
  /** Which routes change when it is loaded — drives the per-page panel tab. */
  affects: RouteKey[];
}

export interface ScenarioCatalogEntry extends ScenarioNote {
  name: string;
}

export interface StorageAdapter {
  load(key: string): string | null;
  save(key: string, value: string): void;
  clear(key: string): void;
}

/** Everything the store needs to know about an app's world, so the store itself
 *  stays generic over FMan and FLIP. */
export interface WorldSource<W> {
  /** Suffix of the localStorage key: 'fman' | 'flip'. */
  appKey: string;
  defaultScenario: string;
  has(name: string): boolean;
  build(name: string): W;
}

export interface ScenarioStore<W> {
  getWorld(): W;
  getScenario(): string;
  setScenario(name: string): void;
  reset(): void;
  /** Write the current world back to storage. Called after mutating verbs. */
  persist(): void;
  subscribe(listener: () => void): () => void;
}
```

- [ ] **Step 5: Write the storage adapter**

Create `packages/mock-devtools/src/storage.ts`:

```ts
import type { StorageAdapter } from './types';

// Persistence is deliberately behind three functions: the worlds measured at
// ~10 KiB make localStorage the right backend today, and if a fixture set ever
// grows past it, swapping the backend touches this file and nothing else.
export const localStorageAdapter: StorageAdapter = {
  load: (key) => {
    try {
      return window.localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  save: (key, value) => {
    try {
      window.localStorage.setItem(key, value);
    } catch {
      // Quota or a privacy mode that blocks writes: the in-memory world still
      // works for this session, so a failed write must not break the app.
    }
  },
  clear: (key) => {
    try {
      window.localStorage.removeItem(key);
    } catch {
      // See save().
    }
  }
};
```

- [ ] **Step 6: Write the store**

Create `packages/mock-devtools/src/scenario-store.ts`:

```ts
import { localStorageAdapter } from './storage';
import type { ScenarioStore, StorageAdapter, WorldSource } from './types';

/** Bump when the shape of any app's MockState changes, so a colleague's stale
 *  blob is discarded instead of half-loading. */
export const STORE_VERSION = 1;

interface Persisted<W> {
  v: number;
  scenario: string;
  world: W;
}

export const createScenarioStore = <W>(
  source: WorldSource<W>,
  storage: StorageAdapter = localStorageAdapter
): ScenarioStore<W> => {
  const key = `operator-ui:dev:mocks:${source.appKey}`;
  const listeners = new Set<() => void>();

  // The persisted world is not deep-validated. The version stamp plus a known
  // scenario name is the whole guard: a schema library for dev-only mock state
  // is not worth it when Reset is one click away.
  const restore = (): Persisted<W> | null => {
    const raw = storage.load(key);
    if (!raw) return null;
    try {
      const parsed = JSON.parse(raw) as Persisted<W>;
      if (parsed.v !== STORE_VERSION) return null;
      if (!source.has(parsed.scenario)) return null;
      return parsed;
    } catch {
      return null;
    }
  };

  const restored = restore();
  let scenario = restored?.scenario ?? source.defaultScenario;
  let world = restored ? restored.world : source.build(scenario);

  const write = () => {
    storage.save(key, JSON.stringify({ v: STORE_VERSION, scenario, world }));
  };

  const notify = () => {
    for (const listener of listeners) listener();
  };

  const load = (name: string) => {
    scenario = name;
    world = source.build(name);
    write();
    notify();
  };

  if (!restored) write();

  return {
    getWorld: () => world,
    getScenario: () => scenario,
    setScenario: (name) => {
      if (!source.has(name)) throw new Error(`unknown scenario: ${name}`);
      load(name);
    },
    reset: () => load(source.defaultScenario),
    persist: write,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    }
  };
};
```

Create `packages/mock-devtools/src/index.ts`:

```ts
export { createScenarioStore, STORE_VERSION } from './scenario-store';
export { localStorageAdapter } from './storage';
export type {
  RouteKey,
  ScenarioCatalogEntry,
  ScenarioNote,
  ScenarioStore,
  StorageAdapter,
  WorldSource
} from './types';
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `pnpm --filter @operator-ui/mock-devtools test`
Expected: 9 passed.

Then: `pnpm --filter @operator-ui/mock-devtools typecheck`

- [ ] **Step 8: Commit**

```bash
git add packages/mock-devtools pnpm-lock.yaml pnpm-workspace.yaml
git commit -m "feat(mock-devtools): add a persisting scenario store"
```

---

### Task 4: Wire FMan's world to the store and serve it from MSW

**Files:**
- Create: `apps/fleet-manager/src/mocks/store.ts`
- Create: `apps/fleet-manager/src/mocks/handlers.ts`
- Modify: `apps/fleet-manager/src/mocks/state.ts` (read the world from the store)
- Modify: `apps/fleet-manager/src/mocks/browser.ts`
- Modify: `apps/fleet-manager/package.json` (add the mock-devtools dependency)
- Test: `apps/fleet-manager/src/mocks/__tests__/handlers.test.ts`

**Interfaces:**
- Consumes: `createScenarioStore`, `WorldSource` from `@operator-ui/mock-devtools` (Task 3); `MockState`, `getState`, `setState` from `@/mocks/state` and `hasScenario`, `scenario`, `scenarioNames` from `@/mocks/scenarios` (Task 2); `dispatch`, `parseRequest`, `MUTATING_VERBS` from `@/mocks/world/verbs` (Task 2).
- Produces: `mockStore: ScenarioStore<MockState>` from `@/mocks/store`; `handlers: RequestHandler[]` from `@/mocks/handlers`.

- [ ] **Step 1: Add the dependency**

```bash
pnpm --filter fman add "@operator-ui/mock-devtools@workspace:*"
```

- [ ] **Step 2: Write the failing handler tests**

MSW's Node entry point lets the handlers be tested without a browser. Create `apps/fleet-manager/src/mocks/__tests__/handlers.test.ts`:

```ts
import { setupServer } from 'msw/node';
import { afterAll, afterEach, beforeAll } from 'vitest';
import { mockStore } from '@/mocks/store';
import { handlers } from '@/mocks/handlers';

const server = setupServer(...handlers);

beforeAll(() => server.listen({ onUnhandledRequest: 'error' }));
afterEach(() => {
  server.resetHandlers();
  mockStore.reset();
});
afterAll(() => server.close());

const admin = async (body: unknown) => {
  const response = await fetch('http://localhost/api/admin', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body)
  });
  return { status: response.status, body: await response.json() };
};

const login = (password = 'test-password') =>
  fetch('http://localhost/api/auth', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ password })
  });

it('should reject an admin call before the operator has logged in', async () => {
  const { status } = await admin('ListSeats');

  expect(status).toBe(401);
});

it('should answer a unit-variant verb sent as a bare string', async () => {
  await login();
  mockStore.setScenario('seats-mixed');

  const { body } = await admin('ListSeats');

  expect(body.Ok.seats).toHaveLength(4);
});

it('should answer a struct-variant verb sent as a single-key object', async () => {
  await login();
  mockStore.setScenario('seats-mixed');

  const { body } = await admin({ SeatStatus: { seat_id: 'seat-running-01' } });

  expect(body.Ok.report.phase).toBe('running');
});

it('should return Err rather than a non-200 status for an unknown verb', async () => {
  await login();

  const { status, body } = await admin('NoSuchVerb');

  expect(status).toBe(200);
  expect(body.Err).toContain('unknown variant NoSuchVerb');
});

it('should refuse fleet verbs while the host is not onboarded', async () => {
  await login();
  mockStore.setScenario('not-onboarded');

  const { body } = await admin('ListSeats');

  expect(body.Err).toContain('has not been onboarded yet');
});

it('should persist a mutation so a later read reflects it', async () => {
  await login();
  mockStore.setScenario('seats-mixed');

  await admin({ DecommissionSeat: { seat_id: 'seat-running-01' } });
  const { body } = await admin({ SeatStatus: { seat_id: 'seat-running-01' } });

  expect(body.Ok.decommissioned).toBe(true);
});

it('should reject a login with the wrong password', async () => {
  const response = await login('wrong');

  expect(response.status).toBe(401);
});
```

**Import-cycle note:** `state.ts` imports `mockStore` from `store.ts`, which imports
`scenario`/`hasScenario` from `scenarios.ts`, which imports back from `state.ts`.
That last edge is `import type` only, so it erases at compile time and there is no
runtime cycle. Keep it type-only — turning it into a value import would break module
initialisation.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `pnpm --filter fman test -- handlers`
Expected: FAIL — cannot resolve `@/mocks/store`.

- [ ] **Step 4: Create the FMan store binding**

Create `apps/fleet-manager/src/mocks/store.ts`:

```ts
import { createScenarioStore, type WorldSource } from '@operator-ui/mock-devtools';
import { hasScenario, scenario } from '@/mocks/scenarios';
import type { MockState } from '@/mocks/state';

const source: WorldSource<MockState> = {
  appKey: 'fman',
  defaultScenario: 'fresh-fleet',
  has: hasScenario,
  build: scenario
};

export const mockStore = createScenarioStore(source);
```

- [ ] **Step 5: Point state.ts at the store**

`src/mocks/state.ts` currently owns a module-level `current` world. The store owns it now, so replace the `current`/`getState`/`setState`/`resetState` block with:

```ts
import { mockStore } from '@/mocks/store';

export const getState = (): MockState => mockStore.getWorld();

export const setState = (next: MockState): void => {
  Object.assign(mockStore.getWorld(), next);
  mockStore.persist();
};

export const resetState = (name?: string): void => {
  if (name === undefined) mockStore.reset();
  else mockStore.setScenario(name);
};
```

`setState` assigns into the existing object rather than swapping the reference, because the store hands out a stable world object that callers may already hold.

Delete the now-unused `DEFAULT_SCENARIO` const and the `scenario` import if nothing else uses them. Keep `patchState` and `setByPath` exactly as they are, but add `mockStore.persist()` as the last line of `patchState`.

- [ ] **Step 6: Write the MSW handlers**

Create `apps/fleet-manager/src/mocks/handlers.ts`:

```ts
import { http, HttpResponse, type RequestHandler } from 'msw';
import { mockStore } from '@/mocks/store';
import { getState } from '@/mocks/state';
import { dispatch, MUTATING_VERBS, parseRequest } from '@/mocks/world/verbs';

// Mirrors crates/fman/core/src/admin_http.rs. The real adapter names the cookie
// randomly per process; a fixed name is fine for a dev mock and easier to debug.
const SESSION_COOKIE_NAME = 'fman_mock_session';
const SESSION_COOKIE_VALUE = 'mock-session-token';

const delay = async (): Promise<void> => {
  const { latencyMs } = getState();
  if (latencyMs > 0) await new Promise((resolve) => setTimeout(resolve, latencyMs));
};

// Real behavior (fedimint_ui_common::auth::require_auth): trusted-proxy mode
// does no local auth; password mode answers a bare 401 with no JSON body.
const isAuthorized = (): boolean => {
  const { authMode, sessionActive } = getState();
  return authMode === 'trusted_proxy' || sessionActive;
};

export const handlers: RequestHandler[] = [
  // Trusted-proxy mode mounts no /api/auth route at all in the real adapter.
  http.post('*/api/auth', async ({ request }) => {
    const state = getState();
    if (state.authMode === 'trusted_proxy') return new HttpResponse(null, { status: 404 });

    const { password } = (await request.json()) as { password?: string };
    if (password !== state.password) return new HttpResponse(null, { status: 401 });

    state.sessionActive = true;
    mockStore.persist();
    // Emitted for realism; the persisted flag above is the source of truth,
    // because a browser cannot hold an HttpOnly cookie the way the daemon sets one.
    return new HttpResponse(null, {
      status: 204,
      headers: {
        'set-cookie': `${SESSION_COOKIE_NAME}=${SESSION_COOKIE_VALUE}; HttpOnly; SameSite=Lax; Path=/`
      }
    });
  }),

  // One route, dispatched on the body: AdminRequest is externally tagged, so a
  // unit variant is a bare string and a struct variant a single-key object.
  http.post('*/api/admin', async ({ request }) => {
    if (!isAuthorized()) return new HttpResponse(null, { status: 401 });

    await delay();

    const body = await request.json();
    const result = dispatch(body);

    const { method } = parseRequest(body);
    if (MUTATING_VERBS.has(method)) mockStore.persist();

    return HttpResponse.json(result, { headers: { 'cache-control': 'no-store' } });
  })
];
```

- [ ] **Step 7: Register the handlers with the worker**

`apps/fleet-manager/src/mocks/browser.ts` becomes:

```ts
import { setupWorker } from 'msw/browser';
import { handlers } from '@/mocks/handlers';

export const worker = setupWorker(...handlers);
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `pnpm --filter fman test -- handlers`
Expected: 7 passed.

Then the whole app suite and typecheck:

```bash
pnpm --filter fman test
pnpm --filter fman typecheck
pnpm --filter fman-mock-server typecheck
```

- [ ] **Step 9: Verify against a running app**

```bash
pnpm --filter fman dev
```

Open `http://localhost:5174` with **no** `fman:be` process running. Expected: the login screen appears, `test-password` gets you in, Overview renders the `fresh-fleet` empty states, and the browser console shows MSW's activation notice with no unhandled-request warnings.

- [ ] **Step 10: Commit**

```bash
git add apps/fleet-manager pnpm-lock.yaml
git commit -m "feat(fman): serve the operator API from MSW handlers"
```

---

### Task 5: Expose the control surface and move Playwright onto it

**Files:**
- Modify: `apps/fleet-manager/src/mocks/start.ts`
- Rewrite: `e2e/fman/support/mock.ts`
- Modify: 11 spec files under `e2e/fman/` (mechanical `request` → `page`)
- Modify: `playwright.config.ts`
- Modify: `dev/fman-stack/up.sh`

**Interfaces:**
- Consumes: `mockStore` from `@/mocks/store` (Task 4).
- Produces: `window.__mockControl = { active, setScenario(name), reset(), getScenario() }`; `resetScenario(page, name): Promise<void>` from `e2e/fman/support/mock`.

- [ ] **Step 1: Extend the control surface**

Rewrite `apps/fleet-manager/src/mocks/start.ts`:

```ts
import { mockStore } from '@/mocks/store';

export interface MockControl {
  active: boolean;
  getScenario: () => string;
  setScenario: (name: string) => void;
  reset: () => void;
}

declare global {
  interface Window {
    __mockControl?: MockControl;
  }
}

export const startMocks = async (): Promise<void> => {
  const { worker } = await import('@/mocks/browser');
  await worker.start({
    onUnhandledRequest: 'warn',
    serviceWorker: { url: '/mockServiceWorker.js' }
  });

  // The only scripted entry point into mock state. Defined here and nowhere
  // else, so it cannot exist in a production build or in daemon mode.
  window.__mockControl = {
    active: true,
    getScenario: () => mockStore.getScenario(),
    setScenario: (name) => mockStore.setScenario(name),
    reset: () => mockStore.reset()
  };
};
```

- [ ] **Step 2: Rewrite the Playwright helper**

Replace `e2e/fman/support/mock.ts` entirely:

```ts
import type { Page } from '@playwright/test';

const STORE_KEY = 'operator-ui:dev:mocks:fman';

// Two paths, because 20+ specs switch scenario after navigating and expect the
// change to take effect the way the old express control route did.
export const resetScenario = async (page: Page, name: string): Promise<void> => {
  if (page.url() === 'about:blank') {
    // Not navigated yet: seed storage before the app boots, so the very first
    // query already sees the right world.
    await page.addInitScript(
      ([key, scenario]) => window.localStorage.setItem(key, JSON.stringify({ seed: scenario })),
      [STORE_KEY, name]
    );
    return;
  }

  await page.evaluate((scenario) => {
    const control = window.__mockControl;
    if (!control) throw new Error('mock control surface is not available');
    control.setScenario(scenario);
  }, name);
};
```

The pre-navigation branch writes a `{ seed }` marker rather than a full world, because the spec's world shape is not knowable from the test. Handle it in the store: in `packages/mock-devtools/src/scenario-store.ts`, inside `restore()`, before the version check:

```ts
      // A test seeded a scenario name before the app booted; build it fresh.
      if (typeof (parsed as { seed?: unknown }).seed === 'string') {
        const seeded = (parsed as unknown as { seed: string }).seed;
        return source.has(seeded) ? { v: STORE_VERSION, scenario: seeded, world: source.build(seeded) } : null;
      }
```

Add a test for it in `packages/mock-devtools/src/__tests__/scenarioStore.test.ts`:

```ts
it('should build a fresh world from a seed marker written before boot', () => {
  const store = createScenarioStore(
    source,
    memoryStorage({ [KEY]: JSON.stringify({ seed: 'populated' }) })
  );

  expect(store.getScenario()).toBe('populated');
  expect(store.getWorld().seats).toEqual(['seat-1']);
});
```

- [ ] **Step 3: Run the store tests**

Run: `pnpm --filter @operator-ui/mock-devtools test`
Expected: 10 passed.

- [ ] **Step 4: Swap the call sites**

```bash
cd operator-ui
grep -rl "resetScenario(request" e2e/fman | xargs sed -i '' 's/resetScenario(request,/resetScenario(page,/g'
```

Then fix the destructuring by hand: any `async ({ request })` or `async ({ page, request })` whose body no longer references `request` must drop it. Typecheck finds them:

Run: `pnpm exec tsc --noEmit -p tsconfig.json` (or `pnpm --filter fman typecheck` if e2e is covered there)
Expected: no unused-binding or missing-name errors.

- [ ] **Step 5: Stop booting the fman express server for e2e**

In `playwright.config.ts`, delete the first entry of `fmanMockWebServer` (the `fman-mock-server` block) so only the Vite entry remains:

```ts
const fmanMockWebServer = [
  {
    command: 'pnpm --filter fman dev',
    url: 'http://localhost:5174',
    reuseExistingServer: !process.env.CI,
    timeout: 60_000
  }
];
```

The readiness probe was `:8788/__control/scenarios`, which no longer exists; the Vite URL is now the only probe.

In the same file, add the kill switch to the daemon entry so MSW cannot hijack live traffic:

```ts
const fmanDaemonWebServer = [
  {
    command: 'pnpm --filter fman dev',
    env: { VITE_MOCKS: 'off' },
    url: 'http://localhost:5174',
    reuseExistingServer: !process.env.CI,
    timeout: 60_000
  }
];
```

- [ ] **Step 6: Add the same kill switch to the local daemon stack**

In `dev/fman-stack/up.sh`, line 106, add `VITE_MOCKS=off` to the exec env:

```bash
exec env FMAN_ADMIN_PROXY_TARGET="http://$ADMIN_ADDR" VITE_MOCKS=off pnpm --filter fman dev
```

Without this the stack script boots a dev server that mocks the very daemon it just started — a failure that looks like a working system.

- [ ] **Step 7: Run the full e2e suite with no express server**

```bash
pkill -f fman-mock-server || true
E2E_APP=fman pnpm test:e2e
```

Expected: the whole fman suite passes, with `fman-mock-server` never started.

- [ ] **Step 8: Commit**

```bash
git add apps/fleet-manager/src/mocks/start.ts packages/mock-devtools e2e/fman playwright.config.ts dev/fman-stack/up.sh
git commit -m "feat(fman): drive e2e scenarios through the in-browser mock control"
```

---

### Task 6: The Global tab of the dev mock panel

**Files:**
- Create: `packages/mock-devtools/src/use-scenario/useScenario.ts`
- Create: `packages/mock-devtools/src/mock-panel/MockPanel.tsx`
- Create: `packages/mock-devtools/src/mock-panel/MockPanel.module.css`
- Create: `packages/mock-devtools/src/mock-panel/__tests__/MockPanel.test.tsx`
- Modify: `packages/mock-devtools/src/index.ts`
- Modify: `apps/fleet-manager/src/mocks/scenarios.ts` (`affects` becomes `RouteKey[]`)
- Create: `apps/fleet-manager/src/app/components/mock-panel-mount/MockPanelMount.tsx`
- Create: `apps/fleet-manager/src/app/components/mock-panel-mount/__tests__/MockPanelMount.test.tsx`
- Modify: `apps/fleet-manager/src/app/components/app-shell/AppShell.tsx`

**Interfaces:**
- Consumes: `ScenarioStore`, `ScenarioCatalogEntry` from `@operator-ui/mock-devtools` (Task 3); `mockStore` (Task 4); `scenarioCatalog` (Task 2).
- Produces: `useScenario(store)` returning `{ scenario, setScenario, reset }`; `<MockPanel store catalog />`; `<MockPanelMount />`.

Only the Global tab ships here. The per-page tab is Plan C, once both apps have route maps.

- [ ] **Step 1: Convert `affects` to structured route keys**

In `apps/fleet-manager/src/mocks/scenarios.ts`, change the `ScenarioNote` import to come from the package and rewrite each `affects` prose string as an array. The `Record<ScenarioName, ScenarioNote>` typing stays, so an undocumented scenario is still a type error:

```ts
import type { ScenarioNote } from '@operator-ui/mock-devtools';

const notes: Record<ScenarioName, ScenarioNote> = {
  'fresh-fleet': {
    desc: 'Default. Onboarded and authorized, but nothing sold yet: no seats, no payment federations, no price.',
    affects: ['overview', 'seats', 'wallet', 'offer']
  },
  'not-onboarded': {
    desc: 'Host has never been onboarded. Only the onboarding verbs answer; everything else refuses.',
    affects: ['setup']
  },
  'awaiting-authorization': {
    desc: 'Onboarded, but no holder has authorized it yet — the QR step is still waiting.',
    affects: ['setup', 'backup']
  },
  'seats-empty': {
    desc: 'Still no seats, but one receivable federation at a zero balance and a price set.',
    affects: ['seats', 'wallet']
  },
  'seats-mixed': {
    desc: 'Four seats covering every phase: running, DKG in progress, code generated, decommissioned. The two pre-formation seats have no fee account yet.',
    affects: ['seats', 'seat-detail', 'overview']
  },
  'seat-unavailable': {
    desc: 'One running seat reporting unavailable health.',
    affects: ['seats', 'overview']
  },
  'wallet-not-receivable': {
    desc: 'Payment federation cannot receive.',
    affects: ['wallet', 'overview']
  },
  'offer-without-payments': {
    desc: 'A paid offer with no payment federation — nothing can ever be bought.',
    affects: ['offer', 'overview']
  },
  earnings: {
    desc: 'Two paid running seats with guardian-fee remittances across several days, one already-spent claim, and a wallet-only leftover federation.',
    affects: ['overview', 'wallet', 'seat-detail']
  }
};
```

Delete the local `ScenarioNote` interface declaration now that it comes from the package.

- [ ] **Step 2: Write the failing panel test**

Create `packages/mock-devtools/src/mock-panel/__tests__/MockPanel.test.tsx`:

```tsx
import { fireEvent, render, screen } from '@testing-library/react';
import { createScenarioStore } from '../../scenario-store';
import type { StorageAdapter, WorldSource } from '../../types';
import { MockPanel } from '../MockPanel';

interface TestWorld {
  seats: string[];
}

const source: WorldSource<TestWorld> = {
  appKey: 'test',
  defaultScenario: 'empty',
  has: (name) => name === 'empty' || name === 'populated',
  build: (name) => ({ seats: name === 'populated' ? ['seat-1'] : [] })
};

const memoryStorage = (): StorageAdapter => {
  const contents: Record<string, string> = {};
  return {
    load: (key) => contents[key] ?? null,
    save: (key, value) => {
      contents[key] = value;
    },
    clear: (key) => {
      delete contents[key];
    }
  };
};

const catalog = [
  { name: 'empty', desc: 'Nothing sold yet.', affects: ['overview'] },
  { name: 'populated', desc: 'One running seat.', affects: ['seats'] }
];

const renderPanel = () => {
  const store = createScenarioStore(source, memoryStorage());
  render(<MockPanel store={store} catalog={catalog} />);
  return store;
};

it('should stay collapsed until opened', () => {
  renderPanel();

  expect(screen.queryByText('One running seat.')).not.toBeInTheDocument();
});

it('should list every scenario with its description once opened', () => {
  renderPanel();

  fireEvent.click(screen.getByRole('button', { name: /mock controls/i }));

  expect(screen.getByText('Nothing sold yet.')).toBeInTheDocument();
  expect(screen.getByText('One running seat.')).toBeInTheDocument();
});

it('should load the scenario the operator picks', () => {
  const store = renderPanel();
  fireEvent.click(screen.getByRole('button', { name: /mock controls/i }));

  fireEvent.click(screen.getByRole('button', { name: /populated/i }));

  expect(store.getScenario()).toBe('populated');
});

it('should mark the active scenario', () => {
  renderPanel();
  fireEvent.click(screen.getByRole('button', { name: /mock controls/i }));

  expect(screen.getByRole('button', { name: /empty/i })).toHaveAttribute('aria-current', 'true');
});

it('should return to the default scenario on reset', () => {
  const store = renderPanel();
  fireEvent.click(screen.getByRole('button', { name: /mock controls/i }));
  fireEvent.click(screen.getByRole('button', { name: /populated/i }));

  fireEvent.click(screen.getByRole('button', { name: /reset mocks/i }));

  expect(store.getScenario()).toBe('empty');
});
```

- [ ] **Step 3: Run it to verify it fails**

Run: `pnpm --filter @operator-ui/mock-devtools test -- MockPanel`
Expected: FAIL — cannot resolve `../MockPanel`.

- [ ] **Step 4: Write the store hook**

Create `packages/mock-devtools/src/use-scenario/useScenario.ts`:

```ts
import { useCallback, useSyncExternalStore } from 'react';
import type { ScenarioStore } from '../types';

export interface ScenarioControls {
  scenario: string;
  setScenario: (name: string) => void;
  reset: () => void;
}

export const useScenario = <W>(store: ScenarioStore<W>): ScenarioControls => {
  const subscribe = useCallback((listener: () => void) => store.subscribe(listener), [store]);
  const scenario = useSyncExternalStore(subscribe, () => store.getScenario());

  const setScenario = useCallback((name: string) => store.setScenario(name), [store]);
  const reset = useCallback(() => store.reset(), [store]);

  return { scenario, setScenario, reset };
};
```

- [ ] **Step 5: Write the panel**

Create `packages/mock-devtools/src/mock-panel/MockPanel.tsx`:

```tsx
import { useState } from 'react';
import type { ScenarioCatalogEntry, ScenarioStore } from '../types';
import { useScenario } from '../use-scenario/useScenario';
import styles from './MockPanel.module.css';

export interface MockPanelProps<W> {
  store: ScenarioStore<W>;
  catalog: ScenarioCatalogEntry[];
}

export const MockPanel = <W,>({ store, catalog }: MockPanelProps<W>) => {
  const { scenario, setScenario, reset } = useScenario(store);
  const [isOpen, setIsOpen] = useState(false);

  const handleToggle = () => setIsOpen((open) => !open);

  return (
    <aside className={styles.panel}>
      <button type="button" className={styles.toggle} onClick={handleToggle}>
        Mock controls
      </button>
      {isOpen ? (
        <div className={styles.body}>
          <ul className={styles.list}>
            {catalog.map((entry) => (
              <ScenarioButton
                key={entry.name}
                entry={entry}
                isActive={entry.name === scenario}
                onSelect={setScenario}
              />
            ))}
          </ul>
          <button type="button" className={styles.reset} onClick={reset}>
            Reset mocks
          </button>
        </div>
      ) : null}
    </aside>
  );
};
```

The per-item handler lives in its own component rather than an arrow inside
`.map`, so "no inline logic in JSX" holds without exception. Create
`packages/mock-devtools/src/scenario-button/ScenarioButton.tsx`:

```tsx
import type { ScenarioCatalogEntry } from '../types';
import styles from './ScenarioButton.module.css';

export interface ScenarioButtonProps {
  entry: ScenarioCatalogEntry;
  isActive: boolean;
  onSelect: (name: string) => void;
}

export const ScenarioButton = ({ entry, isActive, onSelect }: ScenarioButtonProps) => {
  const handleClick = () => onSelect(entry.name);

  return (
    <li>
      <button
        type="button"
        className={styles.scenario}
        aria-current={isActive ? 'true' : undefined}
        onClick={handleClick}
      >
        {entry.name}
      </button>
      <p className={styles.desc}>{entry.desc}</p>
    </li>
  );
};
```

Move the `.scenario`, `.scenario[aria-current='true']` and `.desc` rules out of
`MockPanel.module.css` into a sibling `ScenarioButton.module.css`, and import
`ScenarioButton` at the top of `MockPanel.tsx`.

Add `packages/mock-devtools/src/scenario-button/__tests__/ScenarioButton.test.tsx`:

```tsx
import { fireEvent, render, screen } from '@testing-library/react';
import { ScenarioButton } from '../ScenarioButton';

const entry = { name: 'populated', desc: 'One running seat.', affects: ['seats'] };

it('should pass its scenario name to the select handler', () => {
  const selected: string[] = [];
  render(
    <ul>
      <ScenarioButton entry={entry} isActive={false} onSelect={(name) => selected.push(name)} />
    </ul>
  );

  fireEvent.click(screen.getByRole('button', { name: /populated/i }));

  expect(selected).toEqual(['populated']);
});

it('should mark itself current when active', () => {
  render(
    <ul>
      <ScenarioButton entry={entry} isActive onSelect={() => undefined} />
    </ul>
  );

  expect(screen.getByRole('button', { name: /populated/i })).toHaveAttribute(
    'aria-current',
    'true'
  );
});
```

Create `packages/mock-devtools/src/mock-panel/MockPanel.module.css`:

```css
.panel {
  @apply fixed bottom-4 right-4 z-50 max-w-sm text-left;
}

.toggle {
  @apply rounded-card bg-ink px-3 py-2 text-sm text-white shadow-lg;
}

.body {
  @apply mt-2 max-h-96 overflow-y-auto rounded-card bg-surface p-3 shadow-lg;
}

.list {
  @apply flex flex-col gap-3;
}

.scenario {
  @apply w-full rounded-card px-2 py-1 text-left text-sm font-medium text-ink;
}

.scenario[aria-current='true'] {
  @apply bg-ink text-white;
}

.desc {
  @apply px-2 text-xs text-ink;
}

.reset {
  @apply mt-3 w-full rounded-card border px-2 py-1 text-sm text-ink;
}
```

Export both from `packages/mock-devtools/src/index.ts`:

```ts
export { MockPanel } from './mock-panel/MockPanel';
export type { MockPanelProps } from './mock-panel/MockPanel';
export { useScenario } from './use-scenario/useScenario';
export type { ScenarioControls } from './use-scenario/useScenario';
```

- [ ] **Step 6: Run the panel tests to verify they pass**

Run: `pnpm --filter @operator-ui/mock-devtools test`
Expected: 15 passed (10 store + 5 panel).

- [ ] **Step 7: Mount it in FMan, dev-only**

Create `apps/fleet-manager/src/app/components/mock-panel-mount/MockPanelMount.tsx`:

```tsx
import { lazy, Suspense } from 'react';
import { mockStore } from '@/mocks/store';
import { scenarioCatalog } from '@/mocks/scenarios';

// Lazy so the panel is a separate chunk the production build never references.
const MockPanel = lazy(async () => {
  const { MockPanel: Panel } = await import('@operator-ui/mock-devtools');
  return { default: Panel };
});

export const MockPanelMount = () => {
  if (!import.meta.env.DEV || import.meta.env.VITE_MOCKS === 'off') return null;

  return (
    <Suspense fallback={null}>
      <MockPanel store={mockStore} catalog={scenarioCatalog} />
    </Suspense>
  );
};
```

Create `apps/fleet-manager/src/app/components/mock-panel-mount/__tests__/MockPanelMount.test.tsx`:

```tsx
import { render, screen } from '@testing-library/react';
import { MockPanelMount } from '../MockPanelMount';

it('should render nothing when mocks are switched off', () => {
  const original = import.meta.env.VITE_MOCKS;
  import.meta.env.VITE_MOCKS = 'off';

  try {
    render(<MockPanelMount />);
    expect(screen.queryByRole('button', { name: /mock controls/i })).not.toBeInTheDocument();
  } finally {
    import.meta.env.VITE_MOCKS = original;
  }
});
```

In `apps/fleet-manager/src/app/components/app-shell/AppShell.tsx`, render `<MockPanelMount />` as the last child of the shell's root element.

- [ ] **Step 8: Run everything**

```bash
pnpm --filter fman test
pnpm --filter fman typecheck
pnpm --filter @operator-ui/mock-devtools test
pnpm lint:boundaries
pnpm lint
E2E_APP=fman pnpm test:e2e
```

- [ ] **Step 9: Verify the production bundle again**

```bash
pnpm --filter fman build
grep -r "Mock controls\|__mockControl\|seat-running-01" apps/fleet-manager/dist/assets/ || echo "CLEAN"
```

Expected: `CLEAN`. This is the check that catches a panel accidentally pulled into the main chunk by a static import.

- [ ] **Step 10: Verify in the browser**

```bash
pnpm --filter fman dev
```

With no `fman:be` running: open `http://localhost:5174`, log in, open **Mock controls**, switch to `seats-mixed`, confirm the Seats page shows four seats. Decommission one, **refresh the page**, and confirm it is still decommissioned — that is the persistence requirement working end to end. Then hit **Reset mocks** and confirm it returns to `fresh-fleet`.

- [ ] **Step 11: Commit**

```bash
git add packages/mock-devtools apps/fleet-manager
git commit -m "feat(fman): add the dev mock control panel"
```

---

## Definition of done for Plan A

- [ ] FMan runs fully mocked with Vite alone; `pnpm fman:be` is not needed.
- [ ] All 12 admin verbs plus `POST /api/auth` answer from MSW.
- [ ] All 9 FMan scenarios reproduce their express behaviour.
- [ ] A mutation survives a browser refresh; scenario switch and Reset both discard it.
- [ ] A stale or version-mismatched blob falls back to `fresh-fleet` without crashing.
- [ ] `E2E_APP=fman pnpm test:e2e` passes with no express server running.
- [ ] `E2E_TARGET=daemon` and `dev/fman-stack/up.sh` both set `VITE_MOCKS=off`.
- [ ] The FMan production bundle contains no mock code.
- [ ] `apps/fleet-manager/mock-server/` still exists and still works (deleted in Plan C).

## Deferred to later plans

- **Scenario-switch cache busting** — spec §4.1, added after Plan A shipped. Switching
  scenario swaps the mock world underneath TanStack Query, which keeps serving its cache,
  so the visible screen shows the previous scenario until a manual reload. Verified in the
  browser at `362f04ad`. Fix: `MockPanelMount` subscribes to `mockStore` and calls
  `queryClient.invalidateQueries()` — app-side, because the package must not import the
  app's `queryClient`, and subscribing rather than hooking the button also covers the
  `window.__mockControl` path Playwright drives. Preserve `sessionActive` across the switch
  so invalidation does not refetch into a 401 and bounce the operator to login. Small and
  self-contained; take it first in Plan B, since FLIP will otherwise inherit the same gap.
- Plan B: FLIP handler extraction, MSW handlers, scenario notes for all 15 scenarios, flip e2e swap.
- Plan C: the per-page panel tab and route maps for both apps; form fixtures; deleting both express servers, their scripts and dependencies, the `/__control` vite proxies and the `dev/menu` entry; enabling parallel e2e workers; the CI prod-bundle check.
