# FMan Plan A — key-format sweep and setup price bound

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace every `npub` fixture in the FMan dashboard with the 64-character hexadecimal value the daemon actually returns, and reject a setup price that JSON cannot represent exactly.

**Architecture:** One new module under `apps/fleet-manager/src/mocks/world/keys.ts` owns the canonical mock key values. `mocks/scenarios.ts` and every affected unit test import from it, so the mock world and the test fixtures cannot drift apart again. Separately, `parsePriceField` in `shared/utils/offerPrice.ts` gains a `Number.isSafeInteger` bound after the sats-to-msats multiplication.

**Tech Stack:** TypeScript, React 19, Vitest, Testing Library, Tailwind v3 + CSS Modules, TanStack Query v5, MSW.

**Source of record:** `docs/superpowers/specs/2026-08-11-fman-recovery-authorization-design.md`, sections "D3 — the key-format sweep" and "D9 — setup price bounds".

## Global Constraints

These apply to every task in this plan. They come from `operator-ui/CLAUDE.md`,
`operator-ui/.claude/rules/folder-structure.md` and the design record.

- All work is under `operator-ui/apps/fleet-manager` and `operator-ui/packages`. No Rust change. No daemon change.
- **Do not modify `pnpm-lock.yaml`.** Do not add or remove any package dependency. A lockfile change is a hard policy block that cannot be repaired. Everything in this plan is achievable with the dependencies already in `apps/fleet-manager/package.json`.
- **Do not edit anything under `packages/biome-plugins`, `tasks/`, or `.github/`.** These are protected directories.
- Components are arrow-function consts with a named export. No `function` declarations, no `React.FC`.
- Every component declares an `XProps` interface. Never inline the prop object type.
- One React unit (component **or** hook) per file. A utility file exports neither, so it may export any number of helpers.
- Absolute imports only in `apps/*`: `@/` within the app, `@operator-ui/*` across packages. Never `../` or `./sibling` in app code, except a component importing its own colocated `./X.module.css`.
- Tests live in a `__tests__/` subfolder next to the unit. Vitest. Every `it` description starts with `should`.
- No Tailwind utility strings in TSX. `className` holds only `styles.*` references.
- The canonical mock key module lives in `mocks/world/keys.ts` and is imported **only** by `mocks/*` and by `__tests__/*` files. No production component, hook, page or util may import it. This keeps the mock world out of the production bundle and keeps the feature→app import direction clean for shipped code.
- Every key value in this plan is **64 lowercase hexadecimal characters**. The UI never parses the value; it copies, renders and QR-encodes it. A realistic 64-character string is sufficient — a valid curve point is not required.
- Run the gates locally before each commit:
  ```bash
  bash operator-ui/scripts/harness-gate.sh typecheck && bash operator-ui/scripts/harness-gate.sh lint && bash operator-ui/scripts/harness-gate.sh style && bash operator-ui/scripts/harness-gate.sh structure && bash operator-ui/scripts/harness-gate.sh test
  ```

---

## File Structure

**Create**

| File | Responsibility |
|---|---|
| `operator-ui/apps/fleet-manager/src/mocks/world/keys.ts` | The two canonical hexadecimal key values used by the mock world and by unit-test fixtures. |
| `operator-ui/apps/fleet-manager/src/mocks/__tests__/keys.test.ts` | Proves both values are 64 lowercase hexadecimal characters. |

**Modify**

| File | Change |
|---|---|
| `operator-ui/apps/fleet-manager/src/mocks/scenarios.ts` | Import the two constants; drop both `npub` literals. |
| `operator-ui/apps/fleet-manager/src/features/boot/hooks/use-boot-status/__tests__/useBootStatus.test.tsx` | Replace `npub1abc`. |
| `operator-ui/apps/fleet-manager/src/features/setup/utils/__tests__/setupState.test.ts` | Replace `npub1abc` and `npub1holder`. |
| `operator-ui/apps/fleet-manager/src/features/setup/components/setup-wizard/__tests__/SetupWizard.test.tsx` | Replace `npub1fleetmanagerkey` and `npub1holder`. |
| `operator-ui/apps/fleet-manager/src/features/setup/components/setup-authorization/__tests__/SetupAuthorization.test.tsx` | Replace `npub1fleetmanagerkey` and `npub1holder`. |
| `operator-ui/apps/fleet-manager/src/pages/overview/__tests__/OverviewPage.test.tsx` | Replace five `npub1abc` occurrences. |
| `operator-ui/apps/fleet-manager/src/pages/backup/__tests__/BackupPage.test.tsx` | Replace the `npub1…zp3mhq` literal and any assertion that depends on its length. |
| `operator-ui/apps/fleet-manager/src/shared/utils/offerPrice.ts` | Add the `Number.isSafeInteger` bound to `parsePriceField`. |
| `operator-ui/apps/fleet-manager/src/shared/utils/__tests__/offerPrice.test.ts` | Cover the new rejection. |

---

### Task 1: Canonical hexadecimal key module

**Files:**
- Create: `operator-ui/apps/fleet-manager/src/mocks/world/keys.ts`
- Test: `operator-ui/apps/fleet-manager/src/mocks/__tests__/keys.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `MOCK_SERVICE_NOSTR_PUBKEY: string` and `MOCK_HOLDER_PUBKEY: string`, both exported from `@/mocks/world/keys`. Tasks 2 and 3 import these by exactly these names.

- [ ] **Step 1: Write the failing test**

Create `operator-ui/apps/fleet-manager/src/mocks/__tests__/keys.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';

// The daemon serialises nostr_sdk::PublicKey with to_string(), which returns 64
// lowercase hexadecimal characters — never an npub. A fixture in any other shape
// tests a wire value that cannot occur.
const HEX_64 = /^[0-9a-f]{64}$/;

describe('mock nostr keys', () => {
  it('should expose a service key in the daemon wire format', () => {
    expect(MOCK_SERVICE_NOSTR_PUBKEY).toMatch(HEX_64);
  });

  it('should expose a holder key in the daemon wire format', () => {
    expect(MOCK_HOLDER_PUBKEY).toMatch(HEX_64);
  });

  it('should not reuse one value for both roles', () => {
    expect(MOCK_SERVICE_NOSTR_PUBKEY).not.toBe(MOCK_HOLDER_PUBKEY);
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/mocks/__tests__/keys.test.ts
```

Expected: FAIL — `Failed to resolve import "@/mocks/world/keys"`.

- [ ] **Step 3: Write the module**

Create `operator-ui/apps/fleet-manager/src/mocks/world/keys.ts`:

```ts
/**
 * The canonical nostr public keys of the mock world.
 *
 * `crates/fman/core/src/admin.rs` serialises `nostr_sdk::PublicKey` with
 * `to_string()`, which yields 64 lowercase hexadecimal characters. It is never an
 * `npub`. Fixtures used to carry `npub` values, so every screen was reviewed
 * against a wire value the daemon cannot produce.
 *
 * One module owns these so the mock scenarios and the unit-test fixtures cannot
 * drift apart again. Imported by `@/mocks/*` and by `__tests__/*` only — never by
 * production components, hooks, pages or utils.
 */
export const MOCK_SERVICE_NOSTR_PUBKEY =
  'a7f3c19e4b6d02581cae37b90f4d6152ce8b41a09d7e3f26b5c08d419e2a6f3b';

export const MOCK_HOLDER_PUBKEY =
  'c41d8e07b592a36f1d0c94e5837b62af0195d3e7c68b24a0f7e19d53c802b6a4';
```

If either literal is not exactly 64 characters, the test in Step 1 fails. Adjust the
literal — do not relax the test.

- [ ] **Step 4: Run the test and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/mocks/__tests__/keys.test.ts
```

Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/mocks/world/keys.ts operator-ui/apps/fleet-manager/src/mocks/__tests__/keys.test.ts
git commit -m "test(fman-ui): add canonical hexadecimal mock nostr keys"
```

---

### Task 2: Move the mock world onto the canonical keys

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/mocks/scenarios.ts:16-31`

**Interfaces:**
- Consumes: `MOCK_SERVICE_NOSTR_PUBKEY`, `MOCK_HOLDER_PUBKEY` from `@/mocks/world/keys` (Task 1).
- Produces: nothing new. Every scenario now carries hexadecimal keys.

- [ ] **Step 1: Replace both literals**

In `operator-ui/apps/fleet-manager/src/mocks/scenarios.ts`, add the import beside the
existing imports at the top of the file:

```ts
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
```

Then change the `onboarding` factory and the `authorized` constant. Current code:

```ts
const onboarding = (
  nostr: MockState['onboarding']['nostr'] = { state: 'waiting_for_authorization' }
): MockState['onboarding'] => ({
  // Same value the retired express scenario carried, so the two mock surfaces
  // do not disagree across the migration.
  fman_name: 'mutual-hamster',
  service_pubkey: '02a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9',
  service_nostr_pubkey: 'npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqzp3mhq',
  nostr
});

const authorized = onboarding({
  state: 'authorization_observed',
  authorizations: [],
  holders: ['npub1holder00000000000000000000000000000000000000000000000000000']
});
```

Replacement:

```ts
const onboarding = (
  nostr: MockState['onboarding']['nostr'] = { state: 'waiting_for_authorization' }
): MockState['onboarding'] => ({
  // Same value the retired express scenario carried, so the two mock surfaces
  // do not disagree across the migration.
  fman_name: 'mutual-hamster',
  service_pubkey: '02a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr
});

const authorized = onboarding({
  state: 'authorization_observed',
  authorizations: [],
  holders: [MOCK_HOLDER_PUBKEY]
});
```

Leave `service_pubkey` alone. It is a 66-character compressed secp256k1 key and is
already in the daemon's format.

- [ ] **Step 2: Prove no `npub` remains in the mock world**

```bash
cd operator-ui && grep -rn npub apps/fleet-manager/src/mocks
```

Expected: no output.

- [ ] **Step 3: Run the mock tests**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/mocks
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/mocks/scenarios.ts
git commit -m "fix(fman-ui): give the mock world the daemon's hexadecimal nostr key format"
```

---

### Task 3: Move every unit-test fixture onto the canonical keys

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/features/boot/hooks/use-boot-status/__tests__/useBootStatus.test.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/utils/__tests__/setupState.test.ts`
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-wizard/__tests__/SetupWizard.test.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-authorization/__tests__/SetupAuthorization.test.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/pages/overview/__tests__/OverviewPage.test.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/pages/backup/__tests__/BackupPage.test.tsx`

**Interfaces:**
- Consumes: `MOCK_SERVICE_NOSTR_PUBKEY`, `MOCK_HOLDER_PUBKEY` from `@/mocks/world/keys` (Task 1).
- Produces: nothing.

- [ ] **Step 1: Find every occurrence**

```bash
cd operator-ui && grep -rn npub apps/fleet-manager/src
```

Expected: 16 matches across the six test files listed above. Work through them file by
file.

- [ ] **Step 2: Replace each occurrence**

In each file, add the import at the top of the import block:

```ts
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
```

Import only the constants that file actually uses — an unused import fails the `lint`
gate.

Then:

- Every `service_nostr_pubkey:` value becomes `MOCK_SERVICE_NOSTR_PUBKEY`.
- Every entry inside a `holders: [...]` array becomes `MOCK_HOLDER_PUBKEY`.

Worked example — `setup-authorization/__tests__/SetupAuthorization.test.tsx` currently
reads:

```ts
const waiting = {
  service_pubkey: '02abc',
  service_nostr_pubkey: 'npub1fleetmanagerkey',
  nostr: { state: 'waiting_for_authorization' }
};

const observed = {
  ...waiting,
  nostr: { state: 'authorization_observed', authorizations: [], holders: ['npub1holder'] }
};
```

It becomes:

```ts
const waiting = {
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'waiting_for_authorization' }
};

const observed = {
  ...waiting,
  nostr: {
    state: 'authorization_observed',
    authorizations: [],
    holders: [MOCK_HOLDER_PUBKEY]
  }
};
```

- [ ] **Step 3: Fix the assertions that named the literal**

Two assertions name the key directly and must follow the fixture.

In `SetupAuthorization.test.tsx`:

```ts
await screen.findByText('npub1fleetmanagerkey');
```

becomes:

```ts
await screen.findByText(MOCK_SERVICE_NOSTR_PUBKEY);
```

In `pages/backup/__tests__/BackupPage.test.tsx`, `BackupPage` renders the key through
`truncateMiddle(value, 10, 10)`. Any assertion that hard-codes the truncated string must
be rebuilt from the constant rather than retyped. Import `truncateMiddle` from
`@operator-ui/common-ui` in the test and assert on
`truncateMiddle(MOCK_SERVICE_NOSTR_PUBKEY, 10, 10)`. Read the file first and change only
the assertions that actually name the old literal.

- [ ] **Step 4: Prove the sweep is complete**

```bash
cd operator-ui && grep -rn npub apps/fleet-manager/src
```

Expected: no output, or only a comment that explains why the format is not `npub`. Any
remaining string literal is a failure of this task.

- [ ] **Step 5: Run the whole app test suite**

```bash
cd operator-ui && pnpm --filter fman test
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add operator-ui/apps/fleet-manager/src
git commit -m "test(fman-ui): move every nostr key fixture to the daemon wire format"
```

---

### Task 4: Reject a setup price that JSON cannot represent exactly

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/shared/utils/offerPrice.ts:25-35`
- Test: `operator-ui/apps/fleet-manager/src/shared/utils/__tests__/offerPrice.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `parsePriceField` keeps its existing signature, `(input: string) => ParsedPrice`. Its rejection set grows by one case. Both the setup wizard's price step and the Offer page already call it, so both are protected by this one change.

- [ ] **Step 1: Write the failing tests**

Append to `operator-ui/apps/fleet-manager/src/shared/utils/__tests__/offerPrice.test.ts`,
inside the existing `describe('parsePriceField', …)` block if there is one, otherwise in
a new `describe('parsePriceField', …)`:

```ts
it('should reject a price whose millisatoshi value cannot be represented exactly', () => {
  // 10^16 sats is 10^19 msats, far past Number.MAX_SAFE_INTEGER. JSON.stringify
  // would emit a value the daemon reads back as a different number.
  const parsed = parsePriceField('10000000000000000');

  expect(parsed).toEqual({ ok: false, error: 'That price is too large.' });
});

it('should accept the largest price that survives the millisatoshi conversion', () => {
  const maxSats = Math.floor(Number.MAX_SAFE_INTEGER / 1000);

  expect(parsePriceField(String(maxSats))).toEqual({ ok: true, priceMsat: maxSats * 1000 });
});
```

- [ ] **Step 2: Run the tests and confirm they fail**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/shared/utils/__tests__/offerPrice.test.ts
```

Expected: the first new test FAILS — it currently returns `{ ok: true, priceMsat: 1e19 }`.
The second may already pass; it is a guard against over-tightening the bound.

- [ ] **Step 3: Add the bound**

In `operator-ui/apps/fleet-manager/src/shared/utils/offerPrice.ts`, replace the final
two lines of `parsePriceField`:

```ts
  if (sats < 0) return { ok: false, error: 'A price cannot be negative.' };

  return { ok: true, priceMsat: sats * MSATS_PER_SAT };
};
```

with:

```ts
  if (sats < 0) return { ok: false, error: 'A price cannot be negative.' };

  // The conversion is where precision is lost, so the bound is checked after it.
  // A msat value past Number.MAX_SAFE_INTEGER does not survive JSON: the daemon
  // would store a number the operator never typed.
  const priceMsat = sats * MSATS_PER_SAT;
  if (!Number.isSafeInteger(priceMsat)) return { ok: false, error: 'That price is too large.' };

  return { ok: true, priceMsat };
};
```

- [ ] **Step 4: Run the tests and confirm they pass**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/shared/utils/__tests__/offerPrice.test.ts
```

Expected: PASS.

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
git add operator-ui/apps/fleet-manager/src/shared/utils/offerPrice.ts operator-ui/apps/fleet-manager/src/shared/utils/__tests__/offerPrice.test.ts
git commit -m "fix(fman-ui): reject a setup price JSON cannot represent exactly"
```

---

## Acceptance criteria

- [ ] `grep -rn npub operator-ui/apps/fleet-manager/src` returns no string literal.
- [ ] `mocks/world/keys.ts` exports `MOCK_SERVICE_NOSTR_PUBKEY` and `MOCK_HOLDER_PUBKEY`, each 64 lowercase hexadecimal characters, proven by a test.
- [ ] `mocks/scenarios.ts` and all six affected test files read both values from that one module.
- [ ] No production component, hook, page or util imports `@/mocks/world/keys`.
- [ ] `parsePriceField` rejects a sats value whose millisatoshi conversion is not a safe integer, with the message `That price is too large.`
- [ ] `parsePriceField` still accepts blank, `0`, and ordinary positive integers.
- [ ] `pnpm-lock.yaml` is unchanged.
- [ ] All seven harness gates pass.

## Out of scope

- Any behaviour change to a screen. This plan corrects fixtures and adds one input bound.
- Any daemon change.
- The recovery result states, the shared authorization panel, the navigation item, the gate surface store, or the debug-panel controls. Those are Plans B1, B2 and C.
