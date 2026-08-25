# MSW Migration — Plan B: FLIP cutover

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run the FLIP operator dashboard fully mocked in the browser via MSW, so `pnpm flip:be` is no longer needed for FLIP development or e2e — and harden the shared machinery first, now that it has a second consumer.

**Architecture:** Same shape Plan A proved for FMan. The FLIP mock world moves out of `apps/liquidity-provider/mock-server/` into `apps/liquidity-provider/src/mocks/`, Express keeps importing it during the migration window so parity is structural, and `@operator-ui/mock-devtools` supplies the store, persistence and panel unchanged. The one genuinely new chunk of work is extracting FLIP's Express-coupled `(req, res)` handlers into pure functions — FMan's were already pure.

**Tech Stack:** msw 2.15.0, Vite 6, React 19, TanStack Query 5, Vitest 2, Playwright 1.49, pnpm workspaces.

**Source spec:** [`operator-ui/docs/msw-mock-migration.md`](../msw-mock-migration.md) (phases 5–6, plus §4.1). Plan A (`2026-08-08-msw-migration-plan-a.md`) is complete and merged into the branch. Plan C removes both Express servers.

## Global Constraints

- All paths relative to `operator-ui/`. Branch `feat/msw-mock-migration`, already checked out.
- **Do not modify** `apps/liquidity-provider/src/shared/api/{adminCall,errors,queryClient,tokenStore,restoreMode}.ts`, or anything under `src/features` or `src/pages` in either app. Nothing above `fetch` changes.
- **Do not delete** either `mock-server/` directory. Both die in Plan C.
- **Do not regress FMan.** It is fully migrated and green: 213 unit, 35 e2e. Every task re-runs both.
- Components: arrow-function consts, named exports, declared `XProps` interface, no `React.FC`, no inline logic in JSX, one React unit per file, kebab folders with `__tests__/`.
- Styling: no Tailwind strings in TSX; colocated `.module.css` with bare top-level `@apply`; tokens from `packages/shared-ui/tailwind-preset.cjs` only.
- Tests: Vitest, every `it` starts with `should`. Restore overridden globals/env in `afterEach`.
- **Node 25 quirk:** a built-in Web Storage global shadows jsdom's `window.localStorage` and its `setItem` is not a function. Any vitest test touching localStorage must `vi.stubGlobal('localStorage', <in-memory Storage>)` and restore with `vi.unstubAllGlobals()`. Pattern: `apps/fleet-manager/src/mocks/__tests__/handlers.test.ts`.
- **Pre-commit hook:** this checkout lacks `typos`/`shellcheck`/`treefmt` and the hook fails on a pre-existing unrelated file (`crates/fman/claims/availability/disk-fault-kills-directory-recovery.md`). Commit with `--no-verify` and say so. Never edit an unrelated file to satisfy it.
- **Ports:** ensure 5173/5174/8787/8788 are free before any e2e run. A stale server produces misleading ECONNREFUSED failures — this cost real time in Plan A.

---

### Task 1: Harden the shared package for a second consumer

**Files:**
- Modify: `packages/mock-devtools/src/types.ts`
- Modify: `packages/mock-devtools/src/scenario-store.ts`
- Modify: `packages/mock-devtools/src/storage.ts`
- Modify: `packages/mock-devtools/src/index.ts`
- Modify: `packages/mock-devtools/src/__tests__/scenarioStore.test.ts`
- Modify: `apps/fleet-manager/src/mocks/store.ts`
- Modify: `apps/fleet-manager/src/mocks/__tests__/handlers.test.ts`
- Modify: `e2e/fman/support/mock.ts`

**Interfaces:**
- Consumes: the existing `createScenarioStore`, `WorldSource`, `ScenarioStore`, `StorageAdapter` from Plan A.
- Produces:
  - `storeKey(appKey: string): string` — exported from `@operator-ui/mock-devtools`, returns `` `operator-ui:dev:mocks:${appKey}` ``.
  - `WorldSource<W>` gains optional `carryOver?(previous: W, next: W): void`.
  - `StorageAdapter.clear` is **removed**.

Three findings from Plan A's whole-branch review, all of which get worse once FLIP duplicates the machinery.

- [ ] **Step 1: Write the failing tests**

Append to `packages/mock-devtools/src/__tests__/scenarioStore.test.ts`:

```ts
it('should build the storage key from the app key', () => {
  expect(storeKey('flip')).toBe('operator-ui:dev:mocks:flip');
});

it('should carry state across a scenario switch when the source asks for it', () => {
  const carrying: WorldSource<TestWorld & { session: boolean }> = {
    appKey: 'test',
    defaultScenario: 'empty',
    has: (name) => name === 'empty' || name === 'populated',
    build: (name) => ({ seats: name === 'populated' ? ['seat-1'] : [], session: false }),
    carryOver: (previous, next) => {
      next.session = previous.session;
    }
  };
  const store = createScenarioStore(carrying, memoryStorage());
  store.getWorld().session = true;

  store.setScenario('populated');

  expect(store.getWorld().session).toBe(true);
  expect(store.getWorld().seats).toEqual(['seat-1']);
});

it('should not swallow a scenario builder that throws', () => {
  const exploding: WorldSource<TestWorld> = {
    appKey: 'test',
    defaultScenario: 'empty',
    has: () => true,
    build: (name) => {
      if (name === 'broken') throw new Error('builder is wrong');
      return { seats: [] };
    }
  };
  const store = createScenarioStore(exploding, memoryStorage());

  expect(() => store.setScenario('broken')).toThrow('builder is wrong');
});
```

Add `storeKey` to the file's imports from `../scenario-store`, and `WorldSource` is already imported from `../types`.

- [ ] **Step 2: Run to verify they fail**

Run: `pnpm --filter @operator-ui/mock-devtools test`
Expected: FAIL — `storeKey` is not exported, `carryOver` is not a known property.

- [ ] **Step 3: Add `storeKey` and `carryOver`**

In `packages/mock-devtools/src/types.ts`, add to `WorldSource<W>` after `build`:

```ts
  /** Copy state that must survive a scenario switch from the outgoing world into
   *  the freshly built one. For dev-session artifacts — an authenticated session,
   *  a bootstrap token — that describe the mock rather than the scenario. */
  carryOver?(previous: W, next: W): void;
```

and delete the `clear(key: string): void;` line from `StorageAdapter`.

In `packages/mock-devtools/src/scenario-store.ts`, export the key builder and use it:

```ts
export const storeKey = (appKey: string): string => `operator-ui:dev:mocks:${appKey}`;
```

Replace the inline ``const key = `operator-ui:dev:mocks:${source.appKey}`;`` with `const key = storeKey(source.appKey);`.

- [ ] **Step 4: Narrow the restore catch and wire carryOver**

Still in `scenario-store.ts`, restructure `restore()` so only `JSON.parse` is guarded — a builder that throws must surface, not silently become the default scenario:

```ts
  const restore = (): Persisted<W> | null => {
    const raw = storage.load(key);
    if (!raw) return null;

    let parsed: Persisted<W> & { seed?: string };
    try {
      parsed = JSON.parse(raw) as Persisted<W> & { seed?: string };
    } catch {
      return null;
    }

    // A test seeded a scenario name before the app booted; build it fresh.
    if (typeof parsed.seed === 'string') {
      return source.has(parsed.seed)
        ? { v: STORE_VERSION, scenario: parsed.seed, world: source.build(parsed.seed) }
        : null;
    }

    if (parsed.v !== STORE_VERSION) return null;
    if (!source.has(parsed.scenario)) return null;
    return parsed;
  };
```

Then teach `load()` to carry state over:

```ts
  const load = (name: string) => {
    const previous = world;
    scenario = name;
    world = source.build(name);
    source.carryOver?.(previous, world);
    write();
    notify();
  };
```

- [ ] **Step 5: Drop the dead `clear`**

In `packages/mock-devtools/src/storage.ts`, delete the `clear` implementation from `localStorageAdapter`. Nothing calls it — `reset()` goes through `load()` → `write()`. Update `src/index.ts` exports if `clear` is named there.

Every in-memory `StorageAdapter` in the test files also drops its `clear` property, or TypeScript will flag an excess property. Those live in `packages/mock-devtools/src/__tests__/scenarioStore.test.ts` and `packages/mock-devtools/src/mock-panel/__tests__/MockPanel.test.tsx`.

- [ ] **Step 6: Use `storeKey` at the duplicated call sites**

In `apps/fleet-manager/src/mocks/__tests__/handlers.test.ts`, replace the two hardcoded `'operator-ui:dev:mocks:fman'` strings with `storeKey('fman')`, importing it from `@operator-ui/mock-devtools`.

In `e2e/fman/support/mock.ts`, replace `const STORE_KEY = 'operator-ui:dev:mocks:fman';` with an import:

```ts
import { storeKey } from '@operator-ui/mock-devtools';

const STORE_KEY = storeKey('fman');
```

If the e2e file cannot resolve the workspace import (there is no tsconfig covering `e2e/`), leave the literal and say so in your report rather than adding build plumbing for it.

- [ ] **Step 7: Carry the FMan session across a scenario switch**

In `apps/fleet-manager/src/mocks/store.ts`, add `carryOver` to the source:

```ts
const source: WorldSource<MockState> = {
  appKey: 'fman',
  defaultScenario: 'fresh-fleet',
  has: hasScenario,
  build: scenario,
  // Being logged out on every scenario switch is friction the Express panel
  // never had, because it lived on its own page. The session describes the
  // mock's auth, not the state of the fleet. Spec §4.1.
  carryOver: (previous, next) => {
    next.sessionActive = previous.sessionActive;
  }
};
```

- [ ] **Step 8: Run everything**

```bash
pnpm --filter @operator-ui/mock-devtools test
pnpm --filter @operator-ui/mock-devtools typecheck
pnpm --filter fman test
pnpm --filter fman typecheck
pnpm --filter fman-mock-server typecheck
E2E_APP=fman pnpm test:e2e
pnpm test:e2e
```

Expected: mock-devtools 26, fman 213, FMan e2e 35, FLIP e2e 18. All typechecks clean.

Note the FMan e2e specs set a scenario **before** signing in, so carrying the session over does not change what they exercise — they start with no session either way.

- [ ] **Step 9: Commit**

```bash
git add packages/mock-devtools apps/fleet-manager/src/mocks e2e/fman/support
git commit --no-verify -m "refactor(mock-devtools): harden the store before FLIP consumes it"
```

---

### Task 2: Invalidate the query cache when the scenario changes

**Files:**
- Modify: `apps/fleet-manager/src/app/components/mock-panel-mount/MockPanelMount.tsx`
- Test: `apps/fleet-manager/src/app/components/mock-panel-mount/__tests__/MockPanelMount.test.tsx`

**Interfaces:**
- Consumes: `mockStore` from `@/mocks/store`, `MockPanel` from `@operator-ui/mock-devtools/panel`, `useQueryClient` from `@tanstack/react-query`.
- Produces: nothing new. Behaviour only.

Spec §4.1. Switching scenario swaps the world underneath TanStack Query, which keeps serving its cache, so the visible screen shows the previous scenario until a manual reload. Verified in a browser at `362f04ad`.

- [ ] **Step 1: Write the failing test**

Add to `apps/fleet-manager/src/app/components/mock-panel-mount/__tests__/MockPanelMount.test.tsx`:

```tsx
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, waitFor } from '@testing-library/react';
import { afterEach, vi } from 'vitest';
import { mockStore } from '@/mocks/store';
import { MockPanelMount } from '../MockPanelMount';

afterEach(() => {
  vi.unstubAllEnvs();
  mockStore.reset();
});

it('should invalidate cached queries when the scenario changes', async () => {
  const queryClient = new QueryClient();
  const invalidate = vi.spyOn(queryClient, 'invalidateQueries');
  render(
    <QueryClientProvider client={queryClient}>
      <MockPanelMount />
    </QueryClientProvider>
  );

  mockStore.setScenario('seats-mixed');

  await waitFor(() => expect(invalidate).toHaveBeenCalled());
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `pnpm --filter fman test -- MockPanelMount`
Expected: FAIL — `invalidateQueries` is never called.

- [ ] **Step 3: Subscribe to the store**

Rewrite `MockPanelMount.tsx`, keeping the module-scope `mocksEnabled` gate exactly as it is — that gate is what keeps mock code out of the production bundle, and moving it inside the component reintroduces the leak Plan A fixed:

```tsx
import { useQueryClient } from '@tanstack/react-query';
import { lazy, Suspense, useEffect } from 'react';

// `import.meta.env.DEV` is a build-time constant, so in a production build this
// folds to `false` and the branch below — including the `@/mocks/*` imports it
// reaches — is dead code Rollup never puts in the chunk graph. A runtime guard
// inside the component is not enough. See `@/app/index.tsx` for the same shape.
const mocksEnabled = import.meta.env.DEV && import.meta.env.VITE_MOCKS !== 'off';

const MockPanel = mocksEnabled
  ? lazy(async () => {
      const [{ MockPanel: Panel }, { mockStore }, { scenarioCatalog }] = await Promise.all([
        import('@operator-ui/mock-devtools/panel'),
        import('@/mocks/store'),
        import('@/mocks/scenarios')
      ]);

      const BoundMockPanel = () => <Panel store={mockStore} catalog={scenarioCatalog} />;
      return { default: BoundMockPanel };
    })
  : null;

export const MockPanelMount = () => {
  const queryClient = useQueryClient();

  // Subscribe to the store rather than the panel's button, so a scenario set
  // through `window.__mockControl` — the surface Playwright drives — refreshes
  // the screen too. Spec §4.1.
  useEffect(() => {
    if (!mocksEnabled) return;

    let unsubscribe = () => undefined as void;
    void import('@/mocks/store').then(({ mockStore }) => {
      unsubscribe = mockStore.subscribe(() => {
        void queryClient.invalidateQueries();
      });
    });

    return () => unsubscribe();
  }, [queryClient]);

  if (!MockPanel) return null;

  return (
    <Suspense fallback={null}>
      <MockPanel />
    </Suspense>
  );
};
```

- [ ] **Step 4: Run to verify it passes**

Run: `pnpm --filter fman test -- MockPanelMount`
Expected: PASS.

- [ ] **Step 5: Verify the production bundle is still clean**

```bash
pnpm --filter fman build
ls apps/fleet-manager/dist/
grep -rn "Mock controls\|__mockControl\|seat-running-01\|mock-devtools\|fresh-fleet" apps/fleet-manager/dist/
```

Expected: `dist/` holds only `assets/` and `index.html`; the grep returns nothing. **This is the check that matters most in this task** — a static import of `@/mocks/store` at the top of the file would pass the tests and leak the whole mock world into production.

- [ ] **Step 6: Verify in a browser**

```bash
pnpm --filter fman dev
```

Open `http://localhost:5174` with no `fman:be` running. Sign in with `test-password`. Open **Mock controls**, switch to `seats-mixed`, and confirm the Seats page updates **without a reload**, and that you are **not** bounced to the login screen (Task 1's `carryOver`).

- [ ] **Step 7: Run the suites and commit**

```bash
pnpm --filter fman test
E2E_APP=fman pnpm test:e2e
git add apps/fleet-manager/src/app/components/mock-panel-mount
git commit --no-verify -m "fix(fman): refresh the screen when the mock scenario changes"
```

---

### Task 3: Move the FLIP mock world into the app

**Files:**
- Create (moved): `apps/liquidity-provider/src/mocks/state.ts`, `scenarios.ts`, `logic.ts`
- Create: `apps/liquidity-provider/src/mocks/world/verbs.ts`
- Create: `apps/liquidity-provider/src/mocks/world/health.ts` (moved `withRestoreMarker`)
- Modify: `apps/liquidity-provider/mock-server/src/index.ts`, `middleware.ts`, `routes/control.ts`, `routes/health.ts`, `routes/admin.ts`, `routes/admin/*.ts`
- Modify: `apps/liquidity-provider/mock-server/tsconfig.json`
- Modify: `agent-toolkit.json` (already lists `apps/*/src/mocks` — verify, do not duplicate)

**Interfaces:**
- Consumes: nothing from Tasks 1–2.
- Produces:
  - From `@/mocks/state`: `MockState`, `PatchInput`, `getState()`, `setState(next)`, `patchState(patch)`, `resetState(name?)`, `tick()`.
  - From `@/mocks/scenarios`: `hasScenario(name)`, `scenario(name)`, `scenarioNames`.
  - From `@/mocks/world/verbs`: `type Verb = (payload: unknown) => unknown`, `verbs: Record<string, Verb>`, `MUTATING_VERBS: ReadonlySet<string>`, `adminMethods: string[]`, `dispatch(method: string, payload: unknown): unknown`.
  - `withRestoreMarker(health, bootMode)` from `@/mocks/world/health` — moved unchanged out of `mock-server/src/routes/admin/health.ts`.
  - `dispatch` **throws** a `ServiceErrorLike` for failure; the transport maps it to a status. Define both in `verbs.ts`:
    ```ts
    import type { ServiceErrorCode } from '@operator-ui/types';

    export interface ServiceErrorLike {
      code: ServiceErrorCode;
      message: string;
    }

    export const isServiceErrorLike = (value: unknown): value is ServiceErrorLike =>
      typeof value === 'object' &&
      value !== null &&
      typeof (value as ServiceErrorLike).code === 'string' &&
      typeof (value as ServiceErrorLike).message === 'string';
    ```

This is the genuinely new work. FMan's handlers were already `(payload) => unknown`; FLIP's are `(req: Request, res: Response) => void` writing through `res.json()`. **This is a pure extraction — behaviour must not change.** The regression gate is FLIP's e2e suite passing with no spec edited.

- [ ] **Step 1: Move state, scenarios and logic verbatim**

```bash
git mv apps/liquidity-provider/mock-server/src/state.ts apps/liquidity-provider/src/mocks/state.ts
git mv apps/liquidity-provider/mock-server/src/scenarios.ts apps/liquidity-provider/src/mocks/scenarios.ts
git mv apps/liquidity-provider/mock-server/src/logic.ts apps/liquidity-provider/src/mocks/logic.ts
```

Update their internal imports to the `@/` alias (`@/mocks/scenarios`, `@/mocks/state`, `@/mocks/logic`). `logic.ts` is already pure — it is a list of setup-validation `Rule`s and needs no change beyond its import path.

- [ ] **Step 2: Extract the verb handlers**

Create `apps/liquidity-provider/src/mocks/world/verbs.ts`. For each handler in `mock-server/src/routes/admin.ts` and `mock-server/src/routes/admin/{allocations,attestations,backup,funds,health}.ts`, convert the signature:

```ts
// before (express)
const getFunds = (res: Response): void => {
  res.json(getState().funds);
};

// after (transport-agnostic)
const getFunds: Verb = () => getState().funds;
```

Handlers that read the request body take it as `payload`:

```ts
// before
const applySetupConfig = (req: Request, res: Response): void => {
  const config = req.body as SetupConfig;
  ...
  res.json(body);
};

// after
const applySetupConfig: Verb = (payload) => {
  const config = payload as SetupConfig;
  ...
  return body;
};
```

Handlers that send an error become throws:

```ts
// before
sendServiceError(res, 'failed_precondition', 'setup is not ready');
return;

// after
throw { code: 'failed_precondition', message: 'setup is not ready' } satisfies ServiceErrorLike;
```

Keep every message string byte-identical. A changed message is a behaviour change and the e2e suite may assert on it.

- [ ] **Step 3: Build the verb map and the mutating set**

At the bottom of `verbs.ts`:

```ts
export const verbs: Record<string, Verb> = {
  get_setup_state: getSetupState,
  validate_setup: validateSetup,
  apply_setup_config: applySetupConfig,
  get_provider_config: getProviderConfig,
  update_provider_config: updateProviderConfig,
  get_advertisement_state: getAdvertisementState,
  get_funds: getFunds,
  get_health: getHealth,
  list_wallet_operations: listWalletOperations,
  create_deposit_address: createDepositAddress,
  request_withdrawal: requestWithdrawal,
  republish_advertisement: republishAdvertisement,
  withdraw_advertisement: withdrawAdvertisement,
  refresh_relays: refreshRelays,
  list_allocations: listAllocations,
  get_allocation: getAllocation,
  retry_funding_step: retryFundingStep,
  cancel_allocation: cancelAllocation,
  attestation_install: attestationInstall,
  attestation_list: attestationList,
  attestation_remove: attestationRemove,
  create_backup: createBackup,
  inspect_backup: inspectBackup,
  restore_backup: restoreBackup
};

/** Verbs that change the world. The store persists only after these, so polling
 *  reads do not serialise the world on every tick. */
export const MUTATING_VERBS: ReadonlySet<string> = new Set([
  'apply_setup_config',
  'update_provider_config',
  'create_deposit_address',
  'request_withdrawal',
  'republish_advertisement',
  'withdraw_advertisement',
  'refresh_relays',
  'retry_funding_step',
  'cancel_allocation',
  'attestation_install',
  'attestation_remove',
  'restore_backup'
]);

export const adminMethods = Object.keys(verbs);

export const dispatch = (method: string, payload: unknown): unknown => {
  const verb = verbs[method];
  if (!verb) throw { code: 'not_found', message: `unknown method ${method}` } satisfies ServiceErrorLike;
  return verb(payload);
};
```

Cross-check the 24 keys against `mock-server/src/routes/admin.ts`'s `switch` before moving on. A missing key is a route that 404s at runtime with a green typecheck.

- [ ] **Step 4: Reduce the Express routes to a shim**

`mock-server/src/routes/admin.ts` keeps its `?error=` override and `DEFERRED_PHASE` gate — those are transport concerns that Task 4 will mirror in MSW — and delegates the rest:

```ts
adminRouter.post('/:method', (req, res) => {
  const method = req.params.method;

  const injected = resolveInjectedError(method, req.query);
  if (injected) {
    sendServiceError(res, injected);
    return;
  }

  const requiredPhase = DEFERRED_PHASE[method];
  if (requiredPhase !== undefined && getState().phase < requiredPhase) {
    sendServiceError(res, 'unavailable', `method deferred to phase ${requiredPhase}`);
    return;
  }

  try {
    res.json(dispatch(method, req.body));
  } catch (error) {
    if (isServiceErrorLike(error)) {
      sendServiceError(res, error.code, error.message);
      return;
    }
    throw error;
  }
});
```

Delete the per-method handler bodies and the `routes/admin/` subfolder files whose contents moved. Update `mock-server/src/routes/health.ts` to import `getState` from the app path.

- [ ] **Step 5: Point the mock-server tsconfig at the app source**

Add the alias so `@/mocks/*` resolves to `../src/mocks/*` for both `tsc --noEmit` and `tsx` at runtime, and exclude the vitest files so a Node-only typecheck does not pull them in:

```json
  "compilerOptions": {
    "baseUrl": ".",
    "paths": { "@/*": ["../src/*"] }
  },
  "include": ["src", "../src/mocks"],
  "exclude": ["../src/mocks/**/__tests__"]
```

Merge these into whatever the file already contains rather than overwriting it — read it first. `apps/fleet-manager/mock-server/tsconfig.json` is the working reference if the merge is unclear.

- [ ] **Step 6: Run the regression gate**

```bash
pnpm --filter flip typecheck
pnpm --filter flip-mock-server typecheck
pnpm --filter flip test
pnpm test:e2e
git status --short e2e/
```

Expected: all pass, FLIP e2e 18/18, and **`git status --short e2e/` shows nothing**. No spec file may change — that is what proves the extraction preserved behaviour.

Also confirm Express still boots: with 8787 free, run `npx tsx src/index.ts` from `apps/liquidity-provider/mock-server/`, wait ~10s, confirm it logs `flip-mock-server listening on :8787`, then `curl -s localhost:8787/health | head -c 200` returns JSON. Kill it.

- [ ] **Step 7: Commit**

```bash
git add -A apps/liquidity-provider
git commit --no-verify -m "refactor(flip): lift the mock world out of the express server"
```

---

### Task 4: Serve the FLIP admin API from MSW

**Files:**
- Modify: `apps/liquidity-provider/package.json` (add `msw`, `@operator-ui/mock-devtools`)
- Create: `apps/liquidity-provider/public/mockServiceWorker.js` (generated)
- Create: `apps/liquidity-provider/src/mocks/store.ts`, `browser.ts`, `handlers.ts`
- Modify: `apps/liquidity-provider/src/mocks/state.ts`
- Modify: `apps/liquidity-provider/src/app/App.tsx`
- Modify: `apps/liquidity-provider/vite.config.ts`
- Test: `apps/liquidity-provider/src/mocks/__tests__/handlers.test.ts`

**Interfaces:**
- Consumes: `createScenarioStore`, `storeKey`, `WorldSource` from `@operator-ui/mock-devtools` (Task 1); `dispatch`, `MUTATING_VERBS`, `isServiceErrorLike` from `@/mocks/world/verbs` (Task 3).
- Produces: `mockStore: ScenarioStore<MockState>` from `@/mocks/store`; `handlers: RequestHandler[]` from `@/mocks/handlers`; `worker` from `@/mocks/browser`.

**The critical risk, same as FMan's:** Express runs in Node where `window` is undefined, and it will now transitively import the store → `localStorageAdapter`. The adapter's try/catch is what stops that throwing. Do not "clean up" those catch blocks. Verify Express still boots before claiming done.

- [ ] **Step 1: Install and generate**

```bash
pnpm --filter flip add -D msw@^2.15.0
pnpm --filter flip add "@operator-ui/mock-devtools@workspace:*"
pnpm --filter flip exec msw init public --save
```

`pnpm-workspace.yaml` already allows msw's build script (`allowBuilds.msw: true`), added in Plan A.

- [ ] **Step 2: Write the failing handler tests**

Create `apps/liquidity-provider/src/mocks/__tests__/handlers.test.ts`:

```ts
import { setupServer } from 'msw/node';
import { afterAll, afterEach, beforeAll } from 'vitest';
import { handlers } from '@/mocks/handlers';
import { mockStore } from '@/mocks/store';

const server = setupServer(...handlers);

beforeAll(() => server.listen({ onUnhandledRequest: 'error' }));
afterEach(() => {
  server.resetHandlers();
  mockStore.reset();
});
afterAll(() => server.close());

const admin = async (method: string, body: unknown = null, token = 'e2e-token') => {
  const response = await fetch(`http://localhost/admin/v1/${method}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', authorization: `Bearer ${token}` },
    body: JSON.stringify(body)
  });
  const text = await response.text();
  return { status: response.status, body: text ? JSON.parse(text) : null };
};

it('should serve the unauthenticated health probe', async () => {
  const response = await fetch('http://localhost/health');

  expect(response.status).toBe(200);
  expect((await response.json()).components).toBeDefined();
});

it('should reject an admin call with no bearer token', async () => {
  const response = await fetch('http://localhost/admin/v1/get_funds', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: 'null'
  });

  expect(response.status).toBe(401);
});

it('should answer get_setup_state for the default scenario', async () => {
  const { status, body } = await admin('get_setup_state');

  expect(status).toBe(200);
  expect(body.status).toBe('not_configured');
});

it('should report a configured setup once the ready scenario is active', async () => {
  mockStore.setScenario('all-clear');

  const { body } = await admin('get_setup_state');

  expect(body.status).toBe('ready');
});

it('should answer with a service error for an unknown method', async () => {
  const { status, body } = await admin('no_such_method');

  expect(status).toBe(404);
  expect(body.code).toBe('not_found');
});

it('should honour the error query override', async () => {
  const response = await fetch('http://localhost/admin/v1/get_funds?error=unavailable', {
    method: 'POST',
    headers: { 'content-type': 'application/json', authorization: 'Bearer e2e-token' },
    body: 'null'
  });

  expect(response.status).toBe(503);
});

it('should persist a mutation to localStorage', async () => {
  mockStore.setScenario('all-clear');

  await admin('withdraw_advertisement');
  const raw = window.localStorage.getItem(storeKey('flip'));

  expect(JSON.parse(raw ?? '{}').world.advertisement.publicationStatus).toBe('withdrawn');
});
```

Add the Node-25 localStorage stub at the top of the file, copying the exact shape from `apps/fleet-manager/src/mocks/__tests__/handlers.test.ts`, and import `storeKey` from `@operator-ui/mock-devtools`.

- [ ] **Step 3: Run to verify they fail**

Run: `pnpm --filter flip test -- handlers`
Expected: FAIL — cannot resolve `@/mocks/handlers`.

- [ ] **Step 4: Create the store binding**

Create `apps/liquidity-provider/src/mocks/store.ts`:

```ts
import { createScenarioStore, type WorldSource } from '@operator-ui/mock-devtools';
import { hasScenario, scenario } from '@/mocks/scenarios';
import type { MockState } from '@/mocks/state';

const source: WorldSource<MockState> = {
  appKey: 'flip',
  defaultScenario: 'setup-fresh',
  has: hasScenario,
  build: scenario
};

export const mockStore = createScenarioStore(source);
```

FLIP's bearer token lives in an in-memory `tokenStore`, not in `MockState`, so there is nothing to `carryOver` here.

- [ ] **Step 5: Point state.ts at the store**

Replace the module-level `current` world in `apps/liquidity-provider/src/mocks/state.ts`:

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

Keep `patchState`, `setByPath` and `tick` as they are, but add `mockStore.persist()` as the last line of both `patchState` and `tick`.

- [ ] **Step 6: Write the handlers**

Create `apps/liquidity-provider/src/mocks/handlers.ts`:

```ts
import type { ServiceError, ServiceErrorCode } from '@operator-ui/types';
import { http, HttpResponse, type RequestHandler } from 'msw';
import { mockStore } from '@/mocks/store';
import { getState } from '@/mocks/state';
import { dispatch, isServiceErrorLike, MUTATING_VERBS } from '@/mocks/world/verbs';
import { withRestoreMarker } from '@/mocks/world/health';

const HTTP_STATUS: Record<ServiceErrorCode, number> = {
  invalid_argument: 400,
  failed_precondition: 400,
  permission_denied: 401,
  not_found: 404,
  unavailable: 503,
  internal: 500,
  unknown: 500
};

const serviceError = (code: ServiceErrorCode, message: string) =>
  HttpResponse.json({ code, message } satisfies ServiceError, { status: HTTP_STATUS[code] });

const delay = async (): Promise<void> => {
  const { latencyMs } = getState();
  if (latencyMs > 0) await new Promise((resolve) => setTimeout(resolve, latencyMs));
};

export const handlers: RequestHandler[] = [
  // Unauthenticated liveness probe. The SPA boot sequence reads
  // health.components before the operator has authenticated, so this must
  // serve the full GetHealthResponse, not a bare {status}.
  http.get('*/health', () => {
    const { health, bootMode } = getState();
    return HttpResponse.json(withRestoreMarker(health, bootMode));
  }),

  http.post('*/admin/v1/:method', async ({ request, params }) => {
    const method = String(params.method);

    const header = request.headers.get('authorization') ?? '';
    const token = header.startsWith('Bearer ') ? header.slice(7).trim() : '';
    if (!token) return serviceError('permission_denied', 'missing bearer token');

    const injected = new URL(request.url).searchParams.get('error');
    if (injected) {
      const code = injected === '503' ? 'unavailable' : (injected as ServiceErrorCode);
      return serviceError(code, 'route not available in mock');
    }

    const forced = getState().forcedErrors[method];
    if (forced) {
      const code = forced === '503' ? 'unavailable' : forced;
      return serviceError(code, 'route not available in mock');
    }

    await delay();

    try {
      const result = dispatch(method, await request.json());
      if (MUTATING_VERBS.has(method)) mockStore.persist();
      return HttpResponse.json(result);
    } catch (error) {
      if (isServiceErrorLike(error)) return serviceError(error.code, error.message);
      throw error;
    }
  })
];
```

`withRestoreMarker` currently lives in `mock-server/src/routes/admin/health.ts`. Move it to `apps/liquidity-provider/src/mocks/world/health.ts` as part of Task 3's extraction if you have not already, and update the Express import.

The `DEFERRED_PHASE` map is empty today (`mock-server/src/middleware.ts` says so explicitly). Port it only if it has entries; if it is still `{}`, note in your report that the gate has nothing to gate and was deliberately not carried over.

- [ ] **Step 7: Register the worker and boot it**

Create `apps/liquidity-provider/src/mocks/browser.ts`:

```ts
import { setupWorker } from 'msw/browser';
import { handlers } from '@/mocks/handlers';

export const worker = setupWorker(...handlers);
```

In `apps/liquidity-provider/src/app/App.tsx`, defer the render exactly the way `apps/fleet-manager/src/app/index.tsx` does — read that file first and mirror it:

```tsx
const render = () => {
  createRoot(root).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </StrictMode>
  );
};

if (import.meta.env.DEV && import.meta.env.VITE_MOCKS !== 'off') {
  const { startMocks } = await import('@/mocks/start');
  await startMocks();
}

render();
```

`@/mocks/start` arrives in Task 5. For this task, import `@/mocks/browser` and call `worker.start(...)` inline, then replace it in Task 5.

- [ ] **Step 8: Run everything**

```bash
pnpm --filter flip test
pnpm --filter flip typecheck
pnpm --filter flip-mock-server typecheck
```

Then confirm Express still boots (8787 free → `npx tsx src/index.ts` from the mock-server dir → logs `listening on :8787` → `curl -s localhost:8787/health` returns JSON → kill it).

Then run the app with **no** `flip:be`: `pnpm --filter flip dev`, open `http://localhost:5173`, and confirm the boot gate passes and the setup wizard appears for `setup-fresh`.

FLIP's e2e will be **red** at this commit, exactly as FMan's was at the equivalent point: MSW now serves `/admin/v1/*` from the browser, so the Express-based `resetScenario` no longer controls what the browser sees. Task 5 fixes it. Do not chase it here, and do not edit `e2e/`.

- [ ] **Step 9: Commit**

```bash
git add apps/liquidity-provider pnpm-lock.yaml
git commit --no-verify -m "feat(flip): serve the admin API from MSW handlers"
```

---

### Task 5: FLIP scenario notes, control surface, and the Playwright swap

**Files:**
- Modify: `apps/liquidity-provider/src/mocks/scenarios.ts`
- Create: `apps/liquidity-provider/src/mocks/start.ts`
- Modify: `apps/liquidity-provider/src/app/App.tsx`
- Rewrite: `e2e/support/mock.ts`
- Modify: 8 spec files under `e2e/` (18 call sites)
- Modify: `playwright.config.ts`
- Modify: `dev/flip-stack/up.sh`

**Interfaces:**
- Consumes: `mockStore` from `@/mocks/store` (Task 4); `storeKey`, `ScenarioNote` from `@operator-ui/mock-devtools` (Task 1).
- Produces: `scenarioCatalog` from `@/mocks/scenarios`; `startMocks()` from `@/mocks/start`; `window.__mockControl`; `resetScenario(page, name)` from `e2e/support/mock`.

- [ ] **Step 1: Author the scenario notes**

FLIP has no `notes` record at all — this is new content, not a port. Add to `apps/liquidity-provider/src/mocks/scenarios.ts`, mirroring FMan's shape so an undocumented scenario is a type error:

```ts
import type { ScenarioNote } from '@operator-ui/mock-devtools';

export type ScenarioName = keyof typeof builders;

const notes: Record<ScenarioName, ScenarioNote> = {
  'setup-fresh': {
    desc: 'Default. Nothing configured yet: the wizard is the only reachable screen.',
    affects: ['setup']
  },
  'setup-pending': {
    desc: 'A config has been applied and is awaiting validation.',
    affects: ['setup', 'settings']
  },
  'all-clear': {
    desc: 'Fully configured and published: healthy funds, a live advertisement, connected relays.',
    affects: ['overview', 'funds', 'advertisement', 'settings']
  },
  'funds-critical': {
    desc: 'Balance below the critical threshold.',
    affects: ['overview', 'funds']
  },
  'funds-warning': {
    desc: 'Balance below the low-balance warning but above critical.',
    affects: ['overview', 'funds']
  },
  'ad-stale': {
    desc: 'The advertisement has expired and needs republishing.',
    affects: ['overview', 'advertisement']
  },
  'ad-withdrawn': {
    desc: 'The advertisement has been withdrawn; the provider is not discoverable.',
    affects: ['overview', 'advertisement']
  },
  'ad-failed': {
    desc: 'Publication failed on every relay.',
    affects: ['overview', 'advertisement']
  },
  'ad-relays-mixed': {
    desc: 'Some relays accepted the advertisement and some rejected it.',
    affects: ['advertisement']
  },
  'health-degraded': {
    desc: 'One or more components report degraded health.',
    affects: ['overview']
  },
  'wallet-ops-broadcast-cancelled': {
    desc: 'A wallet operation was broadcast and then cancelled.',
    affects: ['funds']
  },
  'wallet-ops-review': {
    desc: 'A wallet operation is waiting on operator review.',
    affects: ['funds']
  },
  'allocations-action-required': {
    desc: 'An allocation is stalled and needs the operator to act.',
    affects: ['overview', 'allocations']
  },
  'allocations-cancelled': {
    desc: 'An allocation was cancelled.',
    affects: ['allocations']
  },
  'allocations-mixed': {
    desc: 'Allocations across every state at once.',
    affects: ['allocations']
  }
};

export const scenarioNames = Object.keys(builders) as ScenarioName[];

export const scenarioCatalog = scenarioNames.map((name) => ({ name, ...notes[name] }));
```

`builders` is currently typed `Record<string, () => MockState>`. Change it to a `satisfies Record<string, () => MockState>` so the keys stay literal and `ScenarioName` is a union — otherwise `notes` cannot be key-checked. Read FMan's `scenarios.ts` for the exact pattern.

**Verify each description against the builder it names** rather than trusting the list above — these were written from scenario names, and a description that lies is worse than none.

- [ ] **Step 2: Create the control surface**

Create `apps/liquidity-provider/src/mocks/start.ts`, mirroring `apps/fleet-manager/src/mocks/start.ts`:

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

  window.__mockControl = {
    active: true,
    getScenario: () => mockStore.getScenario(),
    setScenario: (name) => mockStore.setScenario(name),
    reset: () => mockStore.reset()
  };
};
```

Point `App.tsx`'s dynamic import at `@/mocks/start` and delete the inline `worker.start` from Task 4.

- [ ] **Step 3: Rewrite the Playwright helper**

Replace `e2e/support/mock.ts` entirely:

```ts
import type { Page } from '@playwright/test';

const STORE_KEY = 'operator-ui:dev:mocks:flip';

// Two paths, because specs switch scenario after navigating and expect the
// change to take effect the way the old express control route did.
export const resetScenario = async (page: Page, name: string): Promise<void> => {
  if (page.url() === 'about:blank') {
    // Not navigated yet: seed storage before the app boots. Only seed when the
    // key is absent — addInitScript re-runs on EVERY document in this page, and
    // re-seeding on a reload would rebuild the world and drop any mutation.
    await page.addInitScript(
      ([key, scenario]) => {
        if (!window.localStorage.getItem(key)) {
          window.localStorage.setItem(key, JSON.stringify({ seed: scenario }));
        }
      },
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

- [ ] **Step 4: Swap the 18 call sites**

```bash
grep -rl "resetScenario(request" e2e/*.spec.ts | xargs sed -i '' 's/resetScenario(request,/resetScenario(page,/g'
```

Then fix the destructuring by hand: any `async ({ request })` or `async ({ page, request })` whose body no longer uses `request` must drop it. `pnpm exec biome check e2e` flags unused bindings.

Note `e2e/support/wizard.ts::authenticate` fills the admin-token prompt — leave it alone. FLIP's token lives in an in-memory `tokenStore`, not in the mock world, so it is unaffected by scenario switching.

- [ ] **Step 5: Stop booting the FLIP Express server, and guard daemon mode**

In `playwright.config.ts`, reduce `mockWebServer` to the Vite entry only:

```ts
const mockWebServer = [
  {
    command: 'pnpm --filter flip dev',
    url: 'http://localhost:5173',
    reuseExistingServer: !process.env.CI,
    timeout: 60_000
  }
];
```

and guard the daemon entry, mirroring what `fmanDaemonWebServer` already does:

```ts
const daemonWebServer = [
  {
    command: 'pnpm --filter flip dev',
    env: { VITE_MOCKS: 'off' },
    url: 'http://localhost:5173',
    reuseExistingServer: false,
    timeout: 60_000
  }
];
```

`reuseExistingServer: false` matters: Playwright's reuse check is "does the URL respond", not "does the env match", so a mocked dev server already on 5173 would otherwise be silently reused for a daemon-target run.

In `dev/flip-stack/up.sh` line 114, add the kill switch:

```bash
exec env FLIP_ADMIN_PROXY_TARGET="http://$ADMIN_ADDR" VITE_MOCKS=off pnpm --filter flip dev
```

Also add `strictPort: true` to `apps/liquidity-provider/vite.config.ts`'s `server` block, so the stack script cannot silently land on 5174 while telling the operator to open 5173.

- [ ] **Step 6: Run the full gate**

```bash
pkill -f flip-mock-server || true
pnpm test:e2e
E2E_APP=fman pnpm test:e2e
pnpm --filter flip test
pnpm --filter flip typecheck
```

Expected: FLIP e2e 18/18 with `[WebServer]` showing only `vite --port 5173` — `flip-mock-server` must never start. FMan e2e still 35/35.

- [ ] **Step 7: Commit**

```bash
git add apps/liquidity-provider e2e playwright.config.ts dev/flip-stack/up.sh
git commit --no-verify -m "feat(flip): drive e2e scenarios through the in-browser mock control"
```

---

### Task 6: Mount the panel in FLIP

**Files:**
- Create: `apps/liquidity-provider/src/app/components/mock-panel-mount/MockPanelMount.tsx`
- Create: `apps/liquidity-provider/src/app/components/mock-panel-mount/__tests__/MockPanelMount.test.tsx`
- Modify: `apps/liquidity-provider/src/app/components/app-shell/AppShell.tsx`

**Interfaces:**
- Consumes: `MockPanel` from `@operator-ui/mock-devtools/panel`; `mockStore` from `@/mocks/store`; `scenarioCatalog` from `@/mocks/scenarios`; the invalidation pattern from Task 2.
- Produces: `<MockPanelMount />`.

- [ ] **Step 1: Write the failing test**

Create `apps/liquidity-provider/src/app/components/mock-panel-mount/__tests__/MockPanelMount.test.tsx`:

```tsx
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, vi } from 'vitest';
import { mockStore } from '@/mocks/store';
import { MockPanelMount } from '../MockPanelMount';

afterEach(() => {
  vi.unstubAllEnvs();
  vi.resetModules();
  mockStore.reset();
});

const renderMount = () => {
  const queryClient = new QueryClient();
  const invalidate = vi.spyOn(queryClient, 'invalidateQueries');
  render(
    <QueryClientProvider client={queryClient}>
      <MockPanelMount />
    </QueryClientProvider>
  );
  return invalidate;
};

it('should offer the mock controls in development', async () => {
  renderMount();

  await waitFor(() =>
    expect(screen.getByRole('button', { name: /mock controls/i })).toBeInTheDocument()
  );
});

it('should invalidate cached queries when the scenario changes', async () => {
  const invalidate = renderMount();

  mockStore.setScenario('all-clear');

  await waitFor(() => expect(invalidate).toHaveBeenCalled());
});
```

Note this covers the mocks-**enabled** branch, which Plan A's FMan equivalent never tested. Testing the "off" branch requires `vi.resetModules()` plus a dynamic re-import, because the gate is evaluated at module scope — see Plan A's FMan test for that shape if you add it.

- [ ] **Step 2: Run to verify it fails**

Run: `pnpm --filter flip test -- MockPanelMount`
Expected: FAIL — cannot resolve `../MockPanelMount`.

- [ ] **Step 3: Write the mount**

Create `apps/liquidity-provider/src/app/components/mock-panel-mount/MockPanelMount.tsx`:

```tsx
import { useQueryClient } from '@tanstack/react-query';
import { lazy, Suspense, useEffect } from 'react';

// `import.meta.env.DEV` is a build-time constant, so in a production build this
// folds to `false` and the branch below — including the `@/mocks/*` imports it
// reaches — is dead code Rollup never puts in the chunk graph. A runtime guard
// inside the component is NOT enough: `@/mocks/store` would already be bundled.
const mocksEnabled = import.meta.env.DEV && import.meta.env.VITE_MOCKS !== 'off';

const MockPanel = mocksEnabled
  ? lazy(async () => {
      const [{ MockPanel: Panel }, { mockStore }, { scenarioCatalog }] = await Promise.all([
        import('@operator-ui/mock-devtools/panel'),
        import('@/mocks/store'),
        import('@/mocks/scenarios')
      ]);

      const BoundMockPanel = () => <Panel store={mockStore} catalog={scenarioCatalog} />;
      return { default: BoundMockPanel };
    })
  : null;

export const MockPanelMount = () => {
  const queryClient = useQueryClient();

  // Subscribe to the store rather than the panel's button, so a scenario set
  // through `window.__mockControl` — the surface Playwright drives — refreshes
  // the screen too. Spec §4.1.
  useEffect(() => {
    if (!mocksEnabled) return;

    // `cancelled` is load-bearing, not defensive boilerplate. StrictMode
    // double-invokes effects — mount, cleanup, mount — before any promise
    // resolves, so without it the discarded first run still subscribes after
    // its own cleanup has been and gone, and that listener is never released.
    let cancelled = false;
    let unsubscribe: () => void = () => undefined;

    void import('@/mocks/store').then(({ mockStore }) => {
      if (cancelled) return;
      unsubscribe = mockStore.subscribe(() => {
        void queryClient.invalidateQueries();
      });
    });

    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, [queryClient]);

  if (!MockPanel) return null;

  return (
    <Suspense fallback={null}>
      <MockPanel />
    </Suspense>
  );
};
```

**Keep the `mocksEnabled` gate at module scope.** A runtime-only guard leaks the mock world into the production bundle — a defect Plan A already paid for once. Keep the runtime `if (!mocksEnabled) return;` inside the effect as well: without it, production would still subscribe even though the panel never renders.

Add a test for the unmount-during-pending-import path alongside the two above — render, unmount immediately, flush microtasks, then `setScenario` and assert `invalidateQueries` was **not** called. FMan's equivalent test is the reference.

- [ ] **Step 4: Mount it in the shell**

Render `<MockPanelMount />` as the last child of the root element in `apps/liquidity-provider/src/app/components/app-shell/AppShell.tsx`. Read the file first and match its structure; do not restructure it.

Note FLIP's setup wizard renders **inside** the shell (unlike FMan's, which gates above it), so the panel will be available during setup too. That is correct and useful — `setup-fresh` and `setup-pending` are the scenarios a developer most wants to switch between.

- [ ] **Step 5: Run to verify it passes**

Run: `pnpm --filter flip test -- MockPanelMount`
Expected: PASS.

- [ ] **Step 6: Verify the production bundle**

```bash
pnpm --filter flip build
ls apps/liquidity-provider/dist/
grep -rn "Mock controls\|__mockControl\|mock-devtools\|setup-fresh\|all-clear" apps/liquidity-provider/dist/
```

Expected: no `mockServiceWorker.js` in `dist/`, and the grep returns nothing.

**If `mockServiceWorker.js` is present**, port the fix FMan already has: an `apply: 'build'` Vite plugin with a `closeBundle` hook that removes the emitted file. Read `apps/fleet-manager/vite.config.ts` and mirror it.

- [ ] **Step 7: Verify in a browser**

```bash
pnpm --filter flip dev
```

With no `flip:be` running, open `http://localhost:5173`. Open **Mock controls**, switch to `all-clear`, and confirm the screen updates **without a reload**. Switch to `funds-critical` and confirm the Funds screen reflects it. Confirm **Reset mocks** appears once you are off `setup-fresh` and disappears after it runs.

- [ ] **Step 8: Run every gate and commit**

```bash
pnpm --filter flip test
pnpm --filter fman test
pnpm --filter @operator-ui/mock-devtools test
pnpm --filter flip typecheck
pnpm --filter fman typecheck
pnpm --filter flip-mock-server typecheck
pnpm --filter fman-mock-server typecheck
pnpm lint
pnpm test:e2e
E2E_APP=fman pnpm test:e2e
git add apps/liquidity-provider
git commit --no-verify -m "feat(flip): add the dev mock control panel"
```

---

## Definition of done for Plan B

- [ ] FLIP runs fully mocked with Vite alone; `pnpm flip:be` is not needed.
- [ ] All 24 admin methods plus `GET /health` answer from MSW.
- [ ] All 15 FLIP scenarios reproduce their Express behaviour, and each has a description verified against its builder.
- [ ] `pnpm test:e2e` passes with no Express server running.
- [ ] `E2E_APP=fman pnpm test:e2e` still passes — FMan is unregressed.
- [ ] `E2E_TARGET=daemon` and `dev/flip-stack/up.sh` both set `VITE_MOCKS=off`.
- [ ] Switching scenario refreshes the screen in place in both apps, with no reload.
- [ ] Neither production bundle contains mock code.
- [ ] Both `mock-server/` directories still exist and still work.

## Deferred to Plan C

- Delete both Express servers, `flip:be`/`fman:be`, the `express`/`tsx` deps, the `/__control` vite proxies and the `dev/menu` entries.
- Port `/__control`'s `patch`, `errors` and `state` routes onto `window.__mockControl`, or explicitly drop them. **Plan C cannot delete Express until this is decided** — `MockState.latencyMs` and `forcedErrors` are otherwise unreachable under MSW.
- The per-page panel tab and route maps for both apps.
- Form fixtures (spec §9).
- Enable parallel e2e workers; add the prod-bundle grep to CI.
- Add an `e2e/tsconfig.json` so the control surface is typechecked.
- Fix stale comments: `playwright.config.ts`'s `workers: 1` rationale, `vite.config.ts` proxy comments, the unused `/__control` proxy entries.
- Pin `msw` exactly so the generated worker's integrity checksum cannot go stale.
- Give `mocks` its own boundaries layer instead of folding it into `app`.
