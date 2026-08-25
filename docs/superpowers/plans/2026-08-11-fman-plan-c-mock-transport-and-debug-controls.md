# FMan Plan C — mock transport failures and the debug-control matrix

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a tester reach every stable recovery and authorization state added by Plans B1 and B2 from the dev control panel alone, without touching the dotted-path editor.

**Architecture:** The mock world gains four typed setup controls. Transport failures are implemented at the HTTP boundary in `handlers.ts`, not inside a verb, so a lost response is never confused with a daemon `{ Err: … }`. The recovery verb reads its counts from the world instead of returning a fixed `2 / 1`. Three authorization scenarios join the catalog.

**Tech Stack:** TypeScript, MSW, Vitest, `@operator-ui/mock-devtools`.

**Source of record:** `docs/superpowers/specs/2026-08-11-fman-recovery-authorization-design.md`, section D10.

**Depends on:** Plan A, Plan B1 and Plan B2. All three must already be on the branch — this plan drives the UI they build.

**Scope note:** this is test infrastructure, not operator-visible behaviour. Nothing here ships to an operator. It must not delay Plan B1 or B2.

## Global Constraints

- All work is under `operator-ui/apps/fleet-manager/src/mocks`. No production component, hook, page or util changes in this plan. No Rust change. No daemon change.
- **Do not modify `pnpm-lock.yaml`.** Do not add or remove any package dependency.
- **Do not edit anything under `packages/biome-plugins`, `tasks/`, or `.github/`.**
- **Do not change `@operator-ui/mock-devtools`.** `MockControl.kind` is `'number' | 'select'` only. Every control in this plan is a `select`. If a control seems to need a new kind, model it as a select instead.
- Absolute imports only: `@/` within the app, `@operator-ui/*` across packages.
- Vitest, `it("should …")`. Restore any overridden global in an `afterEach`.
- Every writer on the panel — a control's `write`, `errors.set`, `patch` — owns persisting the world and notifying the store. The panel does not do it for them, because the same writes arrive through `window.__mockControl` with no panel in the loop and must behave identically.
- A control's value must survive a scenario switch only if it describes the mock session rather than the scenario. `mockStore`'s `carryOver` currently carries `sessionActive` and nothing else. The four new controls describe the scenario, so they **reset** with it.
- Run the gates locally before each commit:
  ```bash
  bash operator-ui/scripts/harness-gate.sh typecheck && bash operator-ui/scripts/harness-gate.sh lint && bash operator-ui/scripts/harness-gate.sh style && bash operator-ui/scripts/harness-gate.sh structure && bash operator-ui/scripts/harness-gate.sh test
  ```

---

## File Structure

All paths relative to `operator-ui/apps/fleet-manager/src/`.

| File | Change |
|---|---|
| `mocks/state.ts` | Four new typed fields on `MockState`, plus a setter for the one-shot session expiry. |
| `mocks/scenarios.ts` | `base()` seeds the four fields; three authorization scenarios join the catalog. |
| `mocks/world/verbs.ts` | `onboardFromBackup` returns the configured counts; `expire on submit` is consumed here or at the boundary, never both. |
| `mocks/handlers.ts` | Transport failures before dispatch and after commit; the one-shot 401. |
| `mocks/panel-config.ts` | Four new selects; a longer recovery error list. |
| `mocks/__tests__/handlers.test.ts` | Coverage for each transport failure. |
| `mocks/__tests__/verbs.test.ts` (create if absent) | Coverage for the recovery count variants. |
| `mocks/__tests__/panel-config.test.ts` (create) | Coverage for control read/write and scenario reset. |

---

### Task 1: Typed setup controls in the mock world

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/mocks/state.ts`
- Modify: `operator-ui/apps/fleet-manager/src/mocks/scenarios.ts`

**Interfaces:**
- Consumes: nothing.
- Produces, exported from `@/mocks/state` and used by Tasks 2, 3 and 4:

  ```ts
  export type RestoreResultChoice = 'two-seats-one-formed' | 'two-seats-no-formed' | 'no-seats';
  export type RestoreTransport = 'normal' | 'fail-before-dispatch' | 'fail-after-commit';
  export type RestoreSession = 'active' | 'expire-on-submit';
  export type OnboardingTransport = 'normal' | 'network-failure';
  ```

  and four new required fields on `MockState`:

  ```ts
  restoreResult: RestoreResultChoice;
  restoreTransport: RestoreTransport;
  restoreSession: RestoreSession;
  onboardingTransport: OnboardingTransport;
  ```

  plus `export const RESTORE_COUNTS: Record<RestoreResultChoice, { seats: number; formed: number }>`.

- [ ] **Step 1: Extend `MockState`**

In `mocks/state.ts`, above the `MockState` interface:

```ts
/** What `OnboardFromBackup` reports having recovered. The daemon's own counts vary
 *  with the relay records behind the phrase; a fixed 2 / 1 could never produce the
 *  zero-seat screen. */
export type RestoreResultChoice = 'two-seats-one-formed' | 'two-seats-no-formed' | 'no-seats';

export const RESTORE_COUNTS: Record<RestoreResultChoice, { seats: number; formed: number }> = {
  'two-seats-one-formed': { seats: 2, formed: 1 },
  'two-seats-no-formed': { seats: 2, formed: 0 },
  'no-seats': { seats: 0, formed: 0 }
};

/** Where a recovery attempt loses its answer. Both failures happen at the HTTP
 *  boundary, never inside a verb: a lost response is not a daemon `{ Err }`, and
 *  the UI must be able to tell them apart. */
export type RestoreTransport = 'normal' | 'fail-before-dispatch' | 'fail-after-commit';

/** `expire-on-submit` applies to the NEXT OnboardFromBackup call only. Changing the
 *  control does not expire the session now, so the recovery form stays open until
 *  the tester submits it. */
export type RestoreSession = 'active' | 'expire-on-submit';

/** Fails the status check the unknown-result screen makes. Not a daemon `{ Err }`. */
export type OnboardingTransport = 'normal' | 'network-failure';
```

Add the four fields to `MockState`, next to `latencyMs` and `forcedErrors`:

```ts
  restoreResult: RestoreResultChoice;
  restoreTransport: RestoreTransport;
  restoreSession: RestoreSession;
  onboardingTransport: OnboardingTransport;
```

- [ ] **Step 2: Seed them in every scenario**

In `mocks/scenarios.ts`, extend `base()`. It already returns the `Pick<…>` shared by every
builder, so one edit covers all of them:

```ts
const base = (): Pick<
  MockState,
  | 'onboarded'
  | 'authMode'
  | 'sessionActive'
  | 'password'
  | 'latencyMs'
  | 'forcedErrors'
  | 'restoreResult'
  | 'restoreTransport'
  | 'restoreSession'
  | 'onboardingTransport'
> => ({
  onboarded: true,
  authMode: 'password',
  sessionActive: false,
  password: 'test-password',
  latencyMs: 0,
  forcedErrors: {},
  restoreResult: 'two-seats-one-formed',
  restoreTransport: 'normal',
  restoreSession: 'active',
  onboardingTransport: 'normal'
});
```

Because the values live in `base()` and `carryOver` does not copy them, switching
scenarios resets all four. That is intended: they describe the scenario, not the session.

- [ ] **Step 3: Typecheck**

```bash
cd operator-ui && pnpm --filter fman typecheck
```

Expected: PASS. If a builder constructs its world without `...base()`, add the four
fields there too rather than making them optional.

- [ ] **Step 4: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/mocks/state.ts operator-ui/apps/fleet-manager/src/mocks/scenarios.ts
git commit -m "feat(fman-ui): add typed setup controls to the mock world"
```

---

### Task 2: The recovery verb returns the configured counts

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/mocks/world/verbs.ts:131-149`
- Test: `operator-ui/apps/fleet-manager/src/mocks/__tests__/verbs.test.ts` (create if it does not exist)

**Interfaces:**
- Consumes: `RESTORE_COUNTS`, `getState` from `@/mocks/state` (Task 1).
- Produces: `OnboardFromBackup` answers `{ onboarded: 'restored', seats, formed }` from `RESTORE_COUNTS[state.restoreResult]`.

- [ ] **Step 1: Write the failing test**

Create or extend `mocks/__tests__/verbs.test.ts`:

```ts
import { afterEach, describe, expect, it } from 'vitest';
import { getState, resetState } from '@/mocks/state';
import { dispatch } from '@/mocks/world/verbs';

const restore = () =>
  dispatch({
    OnboardFromBackup: {
      mnemonic: 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about',
      acknowledge_original_host_is_gone: true
    }
  });

afterEach(() => {
  resetState('not-onboarded');
});

describe('OnboardFromBackup', () => {
  it('should report two seat records with one formed by default', () => {
    resetState('not-onboarded');

    expect(restore()).toEqual({ Ok: { onboarded: 'restored', seats: 2, formed: 1 } });
  });

  it('should report two seat records with none formed when configured', () => {
    resetState('not-onboarded');
    getState().restoreResult = 'two-seats-no-formed';

    expect(restore()).toEqual({ Ok: { onboarded: 'restored', seats: 2, formed: 0 } });
  });

  it('should report no seat records when configured', () => {
    resetState('not-onboarded');
    getState().restoreResult = 'no-seats';

    expect(restore()).toEqual({ Ok: { onboarded: 'restored', seats: 0, formed: 0 } });
  });

  it('should still refuse a phrase that is not twelve words', () => {
    resetState('not-onboarded');

    expect(
      dispatch({
        OnboardFromBackup: { mnemonic: 'too short', acknowledge_original_host_is_gone: true }
      })
    ).toEqual({ Err: 'invalid mnemonic' });
  });
});
```

- [ ] **Step 2: Run it and confirm the count cases fail**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/mocks/__tests__/verbs.test.ts
```

- [ ] **Step 3: Read the counts from the world**

In `mocks/world/verbs.ts`, add `RESTORE_COUNTS` to the `@/mocks/state` import and replace
the final two lines of `onboardFromBackup`:

```ts
  getState().onboarded = true;
  return { onboarded: 'restored', seats: 2, formed: 1 };
```

with:

```ts
  const state = getState();
  state.onboarded = true;
  // The daemon's counts come from the relay records behind the phrase, so they vary.
  // A fixed 2 / 1 could never produce the zero-seat screen a tester has to review.
  const { seats, formed } = RESTORE_COUNTS[state.restoreResult];
  return { onboarded: 'restored', seats, formed };
```

- [ ] **Step 4: Run it and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/mocks
```

- [ ] **Step 5: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/mocks/world/verbs.ts operator-ui/apps/fleet-manager/src/mocks/__tests__/verbs.test.ts
git commit -m "feat(fman-ui): let the mock recovery verb return each configured count"
```

---

### Task 3: Transport failures at the HTTP boundary

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/mocks/handlers.ts`
- Test: `operator-ui/apps/fleet-manager/src/mocks/__tests__/handlers.test.ts`

**Interfaces:**
- Consumes: `RestoreTransport`, `RestoreSession`, `OnboardingTransport`, `getState` from `@/mocks/state` (Task 1); `dispatch`, `parseRequest`, `MUTATING_VERBS` from `@/mocks/world/verbs`.
- Produces: no new export. The `POST */api/admin` handler grows three branches.

Semantics, from D10 — these three distinctions are the whole point of the task:

| Control | Effect |
|---|---|
| `restoreTransport: 'fail-before-dispatch'` | Return a **network failure**. Do not call `OnboardFromBackup`. The world is unchanged. |
| `restoreTransport: 'fail-after-commit'` | Call `OnboardFromBackup`, persist the changed world, **then** return a network failure. |
| `onboardingTransport: 'network-failure'` | Fail every `Onboarding` call at the transport. Never `{ Err: … }`. |
| `restoreSession: 'expire-on-submit'` | On the next `OnboardFromBackup` only: set `sessionActive` to false and return HTTP 401 **before dispatch**. Then fall back to `'active'`. |

In MSW, a transport failure is `HttpResponse.error()`. It is not a 500 and not a JSON
body — `adminCall` must raise `NetworkError`, which is exactly what the unknown-result
screen keys on.

- [ ] **Step 1: Write the failing tests**

Add to `mocks/__tests__/handlers.test.ts`, following whatever request helper the file
already defines:

```ts
it('should fail before dispatch without changing the world', async () => {
  resetState('not-onboarded');
  getState().sessionActive = true;
  getState().restoreTransport = 'fail-before-dispatch';

  await expect(postAdmin(restoreBody)).rejects.toBeTruthy();
  expect(getState().onboarded).toBe(false);
});

it('should commit and then lose the answer', async () => {
  resetState('not-onboarded');
  getState().sessionActive = true;
  getState().restoreTransport = 'fail-after-commit';

  await expect(postAdmin(restoreBody)).rejects.toBeTruthy();
  expect(getState().onboarded).toBe(true);
});

it('should fail the status check at the transport, not as a daemon error', async () => {
  resetState('fresh-fleet');
  getState().sessionActive = true;
  getState().onboardingTransport = 'network-failure';

  await expect(postAdmin('Onboarding')).rejects.toBeTruthy();
});

it('should expire the session on the next recovery submit only', async () => {
  resetState('not-onboarded');
  getState().sessionActive = true;
  getState().restoreSession = 'expire-on-submit';

  const response = await fetchAdmin(restoreBody);
  expect(response.status).toBe(401);
  expect(getState().onboarded).toBe(false);
  expect(getState().sessionActive).toBe(false);
  expect(getState().restoreSession).toBe('active');
});
```

`postAdmin` should be the helper that throws on a transport error; `fetchAdmin` the one
that returns the raw `Response`. If the file has only one of the two, add the other
beside it. `restoreBody` is the `{ OnboardFromBackup: { mnemonic, acknowledge_original_host_is_gone: true } }`
object with a twelve-word phrase.

- [ ] **Step 2: Run them and confirm they fail**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/mocks/__tests__/handlers.test.ts
```

- [ ] **Step 3: Add the branches**

Rewrite the `POST */api/admin` handler in `mocks/handlers.ts`:

```ts
  // One route, dispatched on the body: AdminRequest is externally tagged, so a
  // unit variant is a bare string and a struct variant a single-key object.
  http.post('*/api/admin', async ({ request }) => {
    const body = await request.json();
    const { method } = parseRequest(body);
    const state = getState();

    // Consumed before the auth check, so the tester's next submit is the one that
    // is refused rather than some later poll.
    if (method === 'OnboardFromBackup' && state.restoreSession === 'expire-on-submit') {
      state.sessionActive = false;
      state.restoreSession = 'active';
      mockStore.persist();
      return new HttpResponse(null, { status: 401 });
    }

    if (!isAuthorized()) return new HttpResponse(null, { status: 401 });

    await delay();

    // A transport failure is not a daemon refusal. HttpResponse.error() is what
    // makes `adminCall` raise NetworkError, which is what the unknown-result screen
    // keys on — a 500 or a JSON body would produce an AdminApiError instead.
    if (method === 'Onboarding' && state.onboardingTransport === 'network-failure') {
      return HttpResponse.error();
    }

    if (method === 'OnboardFromBackup' && state.restoreTransport === 'fail-before-dispatch') {
      return HttpResponse.error();
    }

    const result = dispatch(body);

    // Feeds the dev panel's per-page tab, which lists what a page actually
    // calls rather than what a hand-written map claims it calls.
    verbLog.record(method);
    if (MUTATING_VERBS.has(method)) mockStore.persist();

    // The daemon acted and the browser never heard. This is the state that
    // BE-FMAN-RECOVERY-002 exists to make recoverable.
    if (method === 'OnboardFromBackup' && state.restoreTransport === 'fail-after-commit') {
      return HttpResponse.error();
    }

    return HttpResponse.json(result, { headers: { 'cache-control': 'no-store' } });
  })
```

Note that `parseRequest` now runs before the auth check, so its call later in the handler
is removed. Keep the `mockStore.persist()` after `dispatch` where it is — the
`fail-after-commit` branch depends on the world already being written when the failure is
returned.

- [ ] **Step 4: Run them and confirm they pass**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/mocks
```

- [ ] **Step 5: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/mocks/handlers.ts operator-ui/apps/fleet-manager/src/mocks/__tests__/handlers.test.ts
git commit -m "feat(fman-ui): add mock transport failures before dispatch and after commit"
```

---

### Task 4: The panel controls and the recovery error list

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/mocks/panel-config.ts`
- Test: `operator-ui/apps/fleet-manager/src/mocks/__tests__/panel-config.test.ts` (create)

**Interfaces:**
- Consumes: the four types and `getState`, `patchState`, `setForcedError` from `@/mocks/state` (Task 1).
- Produces: `panelConfig.controls` grows from two entries to six.

Labels and option order come straight from D10's table:

| Control id | Label | Options, in this order |
|---|---|---|
| `restoreResult` | Restore result | `2 seats / 1 formed`, `2 seats / 0 formed`, `0 seats` |
| `restoreTransport` | Restore transport | `normal`, `fail before dispatch`, `fail after commit` |
| `restoreSession` | Restore session | `active`, `expire on submit` |
| `onboardingTransport` | Onboarding transport | `normal`, `network failure` |

The panel's `MockControl` reads and writes strings, so each control maps its display
labels onto the world's union values. Keep the mapping in one place per control.

- [ ] **Step 1: Write the failing test**

Create `mocks/__tests__/panel-config.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { panelConfig } from '@/mocks/panel-config';
import { getState, resetState } from '@/mocks/state';

const control = (id: string) => {
  const found = panelConfig.controls.find((entry) => entry.id === id);
  if (!found) throw new Error(`no control: ${id}`);
  return found;
};

describe('panelConfig setup controls', () => {
  it('should expose every stable recovery result', () => {
    expect(control('restoreResult').options).toEqual([
      '2 seats / 1 formed',
      '2 seats / 0 formed',
      '0 seats'
    ]);
  });

  it('should write the selected recovery result into the world', () => {
    resetState('not-onboarded');
    control('restoreResult').write('0 seats');

    expect(getState().restoreResult).toBe('no-seats');
  });

  it('should read the world back as its display label', () => {
    resetState('not-onboarded');
    getState().restoreTransport = 'fail-after-commit';

    expect(control('restoreTransport').read()).toBe('fail after commit');
  });

  it('should reset every setup control with the scenario', () => {
    resetState('not-onboarded');
    control('restoreTransport').write('fail before dispatch');
    control('restoreSession').write('expire on submit');

    resetState('fresh-fleet');

    expect(getState().restoreTransport).toBe('normal');
    expect(getState().restoreSession).toBe('active');
  });

  it('should offer the real classes of recovery refusal', () => {
    expect(panelConfig.errors.codes).toContain('invalid mnemonic');
    expect(panelConfig.errors.codes).toContain(
      'backup document version is newer than this build can read'
    );
  });
});
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/mocks/__tests__/panel-config.test.ts
```

- [ ] **Step 3: Add the controls**

In `mocks/panel-config.ts`, above `panelConfig`:

```ts
// The panel speaks display labels; the world speaks union values. Each map is the
// single place the two meet, so a renamed option cannot silently write a value the
// world does not have.
const RESTORE_RESULTS = {
  '2 seats / 1 formed': 'two-seats-one-formed',
  '2 seats / 0 formed': 'two-seats-no-formed',
  '0 seats': 'no-seats'
} as const satisfies Record<string, RestoreResultChoice>;

const RESTORE_TRANSPORTS = {
  normal: 'normal',
  'fail before dispatch': 'fail-before-dispatch',
  'fail after commit': 'fail-after-commit'
} as const satisfies Record<string, RestoreTransport>;

const RESTORE_SESSIONS = {
  active: 'active',
  'expire on submit': 'expire-on-submit'
} as const satisfies Record<string, RestoreSession>;

const ONBOARDING_TRANSPORTS = {
  normal: 'normal',
  'network failure': 'network-failure'
} as const satisfies Record<string, OnboardingTransport>;

const labelFor = <V extends string>(map: Record<string, V>, value: V): string =>
  Object.keys(map).find((label) => map[label] === value) ?? Object.keys(map)[0];
```

Then add four entries to `panelConfig.controls`, after `authMode`. Each follows this
shape — shown for `restoreResult`, repeated for the other three with their own map,
field and label:

```ts
    {
      id: 'restoreResult',
      label: 'Restore result',
      kind: 'select',
      options: Object.keys(RESTORE_RESULTS),
      read: () => labelFor(RESTORE_RESULTS, getState().restoreResult),
      write: (value) => patchState({ path: 'restoreResult', value: RESTORE_RESULTS[value] })
    },
```

`patchState` already persists and notifies, so the writer contract is satisfied.

- [ ] **Step 4: Extend the recovery error list**

Replace `ERROR_MESSAGES` with the real classes of message the UI must display, keeping
the existing generic entries:

```ts
// FMan's forced errors are the message the daemon would return, not a code —
// `dispatch` answers `{ Err: message }`. These are the classes a recovery can
// actually fail with; `BE-FMAN-RECOVERY-003` is what would let the UI select an
// action from a code instead of the prose. An arbitrary message is still reachable
// through `window.__mockControl.setError`.
const ERROR_MESSAGES = [
  'unknown seat',
  'daemon unavailable',
  'not onboarded',
  'internal error',
  'invalid mnemonic',
  'backup document version is newer than this build can read',
  'seat directory already exists: /var/lib/fman/seats/seat-running-01 — remove it and retry',
  'guardian archive missing for a formed seat: seat-running-01'
] as const;
```

- [ ] **Step 5: Run it and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/mocks
```

- [ ] **Step 6: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/mocks/panel-config.ts operator-ui/apps/fleet-manager/src/mocks/__tests__/panel-config.test.ts
git commit -m "feat(fman-ui): give the dev panel typed recovery and transport controls"
```

---

### Task 5: The authorization scenarios

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/mocks/scenarios.ts`

**Interfaces:**
- Consumes: `MOCK_HOLDER_PUBKEY`, `MOCK_SERVICE_NOSTR_PUBKEY` from `@/mocks/world/keys` (Plan A).
- Produces: two new scenario names, `authorization-observed` and `authorization-read-error`, alongside the existing `awaiting-authorization`.

From D10:

- `awaiting-authorization` shows the waiting panel and the Overview signpost. **Exists.**
- `authorization-observed` shows the holder list and the full hexadecimal service Nostr public key.
- `authorization-read-error` shows the read error while the app shell stays available.
- All three list `setup` and `authorization` in `affects`. Any that changes the Overview signpost also lists `overview`. A tester can therefore move the wizard from waiting to observed without leaving the setup tab.
- `not-onboarded` remains the entry point for new setup and for every recovery result, and keeps `affects: ['setup']`.

`authorization-read-error` is built with a forced error rather than a new world field —
`forcedErrors` is already part of `MockState` and every scenario seeds it through
`base()`.

- [ ] **Step 1: Add the two builders**

Inside the `builders` object, beside `awaiting-authorization`:

```ts
  'authorization-observed': () => ({
    ...base(),
    seats: [],
    paymentFederations: [],
    price: null,
    onboarding: authorized
  }),
  'authorization-read-error': () => ({
    ...base(),
    seats: [],
    paymentFederations: [],
    price: null,
    onboarding: onboarding(),
    // The relay read failed. The screen must show the error rather than claim the
    // fleet is waiting — those are different facts, and BE-FMAN-AUTH-001 is what
    // would let the daemon distinguish them itself.
    forcedErrors: { Onboarding: 'relay query failed: connection reset' }
  }),
```

- [ ] **Step 2: Document them**

`notes` is keyed off `builders`, so a missing entry is a type error. Add:

```ts
  'authorization-observed': {
    desc: 'A holder authorization is on the relay. The Authorization page lists the holder and the full service Nostr public key.',
    affects: ['setup', 'authorization', 'backup', 'overview']
  },
  'authorization-read-error': {
    desc: 'The Onboarding read fails. The authorization state is unknown, and the app shell stays available.',
    affects: ['setup', 'authorization', 'backup', 'overview']
  },
```

And widen the existing `awaiting-authorization` note to
`affects: ['setup', 'authorization', 'backup', 'overview']` if Plan B2 has not already
done so.

- [ ] **Step 3: Check the scenario catalog test**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/mocks
```

Any test asserting an exact scenario count or list needs the two new names.

- [ ] **Step 4: Verify the whole matrix by hand**

Start the app against mocks:

```bash
cd operator-ui && pnpm fman:fe
```

Then walk D10's table in the browser. Each row must be reachable from the panel alone,
with no use of the dotted-path editor:

| State to inspect | Panel setup | Expected UI |
|---|---|---|
| Recovery request pending | Latency, then submit | Disabled actions and visible progress |
| Recovery with seat records | `not-onboarded`; result `2 / 1`; transport normal | Success screen with the exact labels |
| Recovery with no formed record | `not-onboarded`; result `2 / 0`; transport normal | Success screen, zero formed records |
| Recovery with no seat record | `not-onboarded`; result `0`; transport normal | Zero-seat warning, Continue only |
| Daemon recovery refusal | Force an `OnboardFromBackup` error | Full daemon-error result with retry actions |
| Authentication refusal | Restore session `expire on submit` | Sign-in gate after submit; no retained phrase |
| Network failure before dispatch | Restore transport `fail before dispatch` | Unknown result; status check returns to the form |
| Network failure after commit | Restore transport `fail after commit` | Unknown result; status check continues without counts |
| Status check cannot connect | Any network restore failure plus Onboarding network failure | Global daemon-unavailable gate; reconnect routes from daemon state |
| Authorization waiting | `awaiting-authorization` | Full key, copy control, waiting text |
| Authorization loading | Latency, then open Authorization | Loading state without waiting text |
| Authorization observed | `authorization-observed` | Holder list and observed text |
| Authorization read error | `authorization-read-error` | Error text without a false waiting claim |
| Authorization refresh, daemon error | Open observed, then force an `Onboarding` `{Err}` | Last known key and status, plus a refresh warning |
| Authorization refresh, transport error, cached | Open observed, then Onboarding transport `network failure` | Last known key and status, plus a refresh warning |
| Authorization refresh, transport error, no cache | Onboarding transport `network failure` before the first response | The daemon-unavailable gate, not a waiting claim |

Record any row that cannot be reached and fix the control rather than the expectation.
The automatic two-second transition is **not** a persistent debug state — the fake-timer
component test from Plan B2 owns it, and the standalone Authorization page provides the
stable observed state for visual review.

- [ ] **Step 5: Run every gate**

```bash
bash operator-ui/scripts/harness-gate.sh typecheck
bash operator-ui/scripts/harness-gate.sh lint
bash operator-ui/scripts/harness-gate.sh style
bash operator-ui/scripts/harness-gate.sh structure
bash operator-ui/scripts/harness-gate.sh fallow
bash operator-ui/scripts/harness-gate.sh test
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/mocks
git commit -m "feat(fman-ui): add stable authorization scenarios to the mock catalog"
```

---

## Acceptance criteria

- [ ] `MockState` carries `restoreResult`, `restoreTransport`, `restoreSession` and `onboardingTransport`; all four are seeded by `base()` and reset with a scenario switch.
- [ ] `OnboardFromBackup` returns each of `2 / 1`, `2 / 0` and `0 / 0` from the control, and still refuses a phrase that is not twelve words.
- [ ] `fail before dispatch` returns a transport failure and leaves `onboarded` false.
- [ ] `fail after commit` sets `onboarded` true, persists, and then returns a transport failure.
- [ ] `Onboarding transport: network failure` fails at the transport, never as `{ Err: … }`.
- [ ] `expire on submit` applies to the next `OnboardFromBackup` only, returns 401 before dispatch, and falls back to `active`.
- [ ] The panel exposes all four controls as selects with the labels and option order from D10.
- [ ] The forced-error list includes invalid mnemonic, unreadable backup version, existing seat directory and missing guardian archive.
- [ ] `authorization-observed` and `authorization-read-error` are in the catalog, documented, and list `setup` and `authorization` in `affects`.
- [ ] Every row of the D10 matrix is reachable from the panel without the dotted-path editor.
- [ ] `pnpm-lock.yaml` is unchanged. All seven harness gates pass.

## Out of scope

- Any production component, hook, page or util change. Those landed in Plans B1 and B2.
- Any change to `@operator-ui/mock-devtools`.
- Any daemon change.
- Real-daemon browser coverage. `BE-FMAN-SETUP-001` blocks it, and the design record's F0 explains why.
