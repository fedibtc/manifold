# FMan Plan B2 — shared authorization surface, navigation, gate surface and signposts

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the FMan authorization state reachable after setup — as a permanent navigation item and an Overview signpost — by lifting the QR, the key and the watch into `shared`, and make the dev mock panel name the surface a gate is actually showing.

**Architecture:** The authorization helpers, the 3-second watch and a new presentational `AuthorizationPanel` move to `shared`, because `features` may not import each other and both `features/setup` and `pages/authorization` need them. A small owner-keyed store in `shared/surface/` lets each gate declare the surface it is rendering, so the mock panel stops guessing from `location.pathname`. The setup authorization step gains an automatic continue with a manual override; the standalone page has neither.

**Tech Stack:** TypeScript, React 19 (React Compiler), TanStack Query v5, react-router-dom v7, `qrcode.react`, Vitest, Testing Library, Tailwind v3 + CSS Modules.

**Source of record:** `docs/superpowers/specs/2026-08-11-fman-recovery-authorization-design.md`, sections D3, D4, D5, D6, D7 and D8.

**Depends on:** Plan A (canonical hexadecimal keys) and Plan B1 (recovery journey). Both must already be on the branch.

## Global Constraints

- All work is under `operator-ui/apps/fleet-manager`. No Rust change. No daemon change.
- **Do not modify `pnpm-lock.yaml`.** Do not add or remove any package dependency. A lockfile change is a hard policy block that cannot be repaired. Everything here uses dependencies already in `apps/fleet-manager/package.json`, including `qrcode.react`.
- **Do not edit anything under `packages/biome-plugins`, `tasks/`, or `.github/`.**
- **Import direction is `shared ← features ← pages/app`.** Concretely for this plan:
  - `shared/components/authorization-panel` imports `shared` and `@operator-ui/*` only.
  - `features/setup` imports the panel from `shared`. It imports no other feature.
  - `pages/authorization` imports the panel from `shared`.
  - `app` gate components import the surface store from `shared`, **never** `@/mocks/*`. The mock world must stay out of the production bundle, which is why the store lives in `shared` and not in `mocks`.
- Components are arrow-function consts with a named export and a declared `XProps` interface. One React unit per file.
- Every component sits in its own kebab-case folder with a colocated `.module.css` and a `__tests__/` test. A page folder holds only `<X>Page.tsx`, `<X>Page.module.css` and `__tests__/`.
- No Tailwind utility strings in TSX. Available shared utilities include `uStack1`, `uStack2`, `uStack4`, `uStack6`, `uPageHead`, `uPageHeading`, `uIntro`, `uMutedText`, `uMutedSm`, `uLongIdText`, `uMonoValue`, `uCopyRow`, `uBorderedPanel`, `uKvRow`, `uFormActions`, `uRowCenter3`.
- React Compiler rules: render is pure, no setState inside `useEffect`, no reading or writing refs during render. Timers and external-store writes belong in `useEffect` with a cleanup.
- Vitest, `it("should …")`. Restore any overridden global (including fake timers) in an `afterEach`.
- Run the gates locally before each commit:
  ```bash
  bash operator-ui/scripts/harness-gate.sh typecheck && bash operator-ui/scripts/harness-gate.sh lint && bash operator-ui/scripts/harness-gate.sh style && bash operator-ui/scripts/harness-gate.sh structure && bash operator-ui/scripts/harness-gate.sh test
  ```

---

## File Structure

**Create** (paths relative to `operator-ui/apps/fleet-manager/src/`)

| File | Responsibility |
|---|---|
| `shared/utils/authorization.ts` (+ `__tests__/`) | `buildAuthorizationPayload`, `isAuthorized`. Moved out of `features/setup/utils/setupState.ts`. |
| `shared/api/hooks/use-authorization-watch/useAuthorizationWatch.ts` | The 3-second watch. Moved from `features/setup/api/hooks/`. |
| `shared/components/authorization-panel/AuthorizationPanel.tsx` (+ `.module.css`, `__tests__/`) | QR, full hexadecimal key with a copy control, status banner. Renders no actions. |
| `shared/surface/gateSurface.ts` (+ `__tests__/`) | Owner-keyed store of the surface each gate is rendering. |
| `pages/authorization/AuthorizationPage.tsx` (+ `.module.css`, `__tests__/`) | The permanent authorization screen: panel plus the observed holder list. |

**Modify**

| File | Change |
|---|---|
| `features/setup/utils/setupState.ts` (+ test) | Keeps `isNotOnboardedError` only. |
| `features/setup/components/setup-authorization/SetupAuthorization.tsx` (+ `.module.css`, test) | Uses the shared panel; automatic continue with a manual override; honest copy. |
| `features/overview/utils/deriveOverview.ts` (+ test) | New attention item for an unobserved authorization. |
| `pages/overview/OverviewPage.tsx` (+ test) | Passes the onboarding nostr state through. |
| `pages/backup/BackupPage.tsx` (+ `.module.css`, test) | Reload signpost about the recovery phrase. |
| `app/components/navigation-items/nav-config.ts` (+ tests) | New `authorization` item after Overview. |
| `app/components/boot-gate/BootGate.tsx` (+ test) | Declares `boot` / `auth` / `daemon-error`. |
| `app/components/setup-gate/SetupGate.tsx` | Declares `setup`. |
| `app/components/mock-panel-mount/MockPanelMount.tsx` (+ test) | Reads the resolved surface instead of the pathname alone. |
| `app/index.tsx` | New `/authorization` route. |
| `mocks/routes.ts` (+ test) | New `authorization` route key. |
| `mocks/scenarios.ts` | `awaiting-authorization` gains `authorization` in `affects`. |

**Delete**

| File | Reason |
|---|---|
| `features/setup/api/hooks/use-authorization-watch/useAuthorizationWatch.ts` | Moved to `shared`. |

---

### Task 1: Lift the authorization helpers into `shared`

**Files:**
- Create: `operator-ui/apps/fleet-manager/src/shared/utils/authorization.ts`
- Create: `operator-ui/apps/fleet-manager/src/shared/utils/__tests__/authorization.test.ts`
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/utils/setupState.ts`
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/utils/__tests__/setupState.test.ts`
- Move: `operator-ui/apps/fleet-manager/src/features/setup/api/hooks/use-authorization-watch/useAuthorizationWatch.ts` → `operator-ui/apps/fleet-manager/src/shared/api/hooks/use-authorization-watch/useAuthorizationWatch.ts`

**Interfaces:**
- Consumes: `OnboardingResponse` from `@operator-ui/types`.
- Produces, imported by every later task:
  - `buildAuthorizationPayload: (onboarding: OnboardingResponse) => string` from `@/shared/utils/authorization`
  - `isAuthorized: (onboarding: OnboardingResponse | undefined) => boolean` from `@/shared/utils/authorization`
  - `useAuthorizationWatch` from `@/shared/api/hooks/use-authorization-watch/useAuthorizationWatch`
- `isNotOnboardedError` stays at `@/features/setup/utils/setupState`. Only `SetupGate` and `SetupRestore` use it, and both are allowed to reach it.

- [ ] **Step 1: Write the failing test**

Create `operator-ui/apps/fleet-manager/src/shared/utils/__tests__/authorization.test.ts`:

```ts
import type { OnboardingResponse } from '@operator-ui/types';
import { describe, expect, it } from 'vitest';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import { buildAuthorizationPayload, isAuthorized } from '@/shared/utils/authorization';

const waiting: OnboardingResponse = {
  fman_name: 'mutual-hamster',
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'waiting_for_authorization' }
};

const observed: OnboardingResponse = {
  ...waiting,
  nostr: { state: 'authorization_observed', authorizations: [], holders: [MOCK_HOLDER_PUBKEY] }
};

describe('buildAuthorizationPayload', () => {
  it('should encode the bare service nostr public key', () => {
    expect(buildAuthorizationPayload(waiting)).toBe(MOCK_SERVICE_NOSTR_PUBKEY);
  });
});

describe('isAuthorized', () => {
  it('should be false while the relay still reports waiting', () => {
    expect(isAuthorized(waiting)).toBe(false);
  });

  it('should be true once an authorization is observed', () => {
    expect(isAuthorized(observed)).toBe(true);
  });

  it('should be false with no response at all', () => {
    expect(isAuthorized(undefined)).toBe(false);
  });
});
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/shared/utils/__tests__/authorization.test.ts
```

- [ ] **Step 3: Create the shared module**

Create `operator-ui/apps/fleet-manager/src/shared/utils/authorization.ts` by moving the
two helpers out of `features/setup/utils/setupState.ts` **with their comments intact**:

```ts
import type { OnboardingResponse } from '@operator-ui/types';

/**
 * The one place the QR's content is decided.
 *
 * The app-link format is still undefined and `credential-app` has no holder-
 * authorization code yet, so this encodes the bare key the attester signs over.
 * `BE-FMAN-AUTH-002` owns the shared payload contract; when it lands, the format
 * changes here and nowhere else.
 */
export const buildAuthorizationPayload = (onboarding: OnboardingResponse): string =>
  onboarding.service_nostr_pubkey;

export const isAuthorized = (onboarding: OnboardingResponse | undefined): boolean =>
  onboarding?.nostr.state === 'authorization_observed';
```

Lives in `shared` rather than `features/setup` because `pages/authorization` needs it too
and a page may not reach into a feature's utils for it while a second feature would then
have no legal path to the same code.

- [ ] **Step 4: Shrink `setupState.ts`**

`features/setup/utils/setupState.ts` keeps only `NOT_ONBOARDED_MARKER` and
`isNotOnboardedError`, with their existing comment. Delete the two moved helpers and the
now-unused `OnboardingResponse` import.

Update `features/setup/utils/__tests__/setupState.test.ts`: move the
`buildAuthorizationPayload` and `isAuthorized` cases out (they are now covered by Step 1)
and keep the `isNotOnboardedError` cases.

- [ ] **Step 5: Move the watch hook**

```bash
cd operator-ui/apps/fleet-manager/src
mkdir -p shared/api/hooks/use-authorization-watch
git mv features/setup/api/hooks/use-authorization-watch/useAuthorizationWatch.ts \
       shared/api/hooks/use-authorization-watch/useAuthorizationWatch.ts
rmdir features/setup/api/hooks/use-authorization-watch
```

In the moved file, change the one import:

```ts
import { isAuthorized } from '@/shared/utils/authorization';
```

Everything else in the file, including its long comment about `refetchOnMount: false`,
stays exactly as it is. That comment is load-bearing.

- [ ] **Step 6: Repoint every importer**

```bash
cd operator-ui && grep -rn "features/setup/utils/setupState\|features/setup/api/hooks/use-authorization-watch" apps/fleet-manager/src
```

Fix each hit:
- `isAuthorized` / `buildAuthorizationPayload` → `@/shared/utils/authorization`
- `useAuthorizationWatch` → `@/shared/api/hooks/use-authorization-watch/useAuthorizationWatch`
- `isNotOnboardedError` → unchanged

- [ ] **Step 7: Run the suite and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman test
```

- [ ] **Step 8: Commit**

```bash
git add -A operator-ui/apps/fleet-manager/src
git commit -m "refactor(fman-ui): lift the authorization helpers and watch into shared"
```

---

### Task 2: The shared `AuthorizationPanel`

**Files:**
- Create: `operator-ui/apps/fleet-manager/src/shared/components/authorization-panel/AuthorizationPanel.tsx`
- Create: `operator-ui/apps/fleet-manager/src/shared/components/authorization-panel/AuthorizationPanel.module.css`
- Test: `operator-ui/apps/fleet-manager/src/shared/components/authorization-panel/__tests__/AuthorizationPanel.test.tsx`

**Interfaces:**
- Consumes: `buildAuthorizationPayload`, `isAuthorized` (Task 1); `Banner`, `CopyButton` from `@operator-ui/common-ui`; `QRCodeSVG` from `qrcode.react`; `describeActionError` from `@/shared/utils/describeActionError`.
- Produces: `AuthorizationPanel` with props
  `{ data: OnboardingResponse | undefined; isLoading: boolean; error: unknown }`.
  It renders **no actions**. Each surface owns its own. Tasks 3 and 4 render it.

Rules from the design record, D3:

- The service Nostr public key renders **in full**, in `uLongIdText`, with a `CopyButton`
  beside it. No truncation. This differs from `BackupPage` on purpose, and the reason
  goes in a comment: that page lists keys for reference, where middle truncation is
  right; this screen shows one value a holder may compare against their own application,
  and a truncated value does not permit that check.
- The UI treats the wire value as hexadecimal. It never parses it.
- Copy must not claim that a holder app can scan the QR and complete the flow. That
  contract does not exist yet; `BE-FMAN-AUTH-002` owns it. The panel may say a holder or
  holder tool can use the key.
- Before the first `Onboarding` response the panel shows an explicit loading state. It
  never renders waiting text from missing data.
- An error with no data shows the error state.
- An error with cached data keeps the last known key and status visible and adds a
  refresh warning.

- [ ] **Step 1: Write the failing test**

```tsx
import type { OnboardingResponse } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import { AdminApiError, NetworkError } from '@/shared/api/errors';
import { AuthorizationPanel } from '../AuthorizationPanel';

const waiting: OnboardingResponse = {
  fman_name: 'mutual-hamster',
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'waiting_for_authorization' }
};

const observed: OnboardingResponse = {
  ...waiting,
  nostr: { state: 'authorization_observed', authorizations: [], holders: [MOCK_HOLDER_PUBKEY] }
};

describe('AuthorizationPanel', () => {
  it('should render the service nostr key in full', () => {
    render(<AuthorizationPanel data={waiting} isLoading={false} error={null} />);

    expect(screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY)).toBeTruthy();
  });

  it('should offer a copy control for the key', () => {
    render(<AuthorizationPanel data={waiting} isLoading={false} error={null} />);

    expect(screen.getByRole('button', { name: /copy service nostr/i })).toBeTruthy();
  });

  it('should not claim a holder app can scan and finish the flow', () => {
    render(<AuthorizationPanel data={waiting} isLoading={false} error={null} />);

    expect(screen.queryByText(/scans this with their app/i)).toBeNull();
  });

  it('should show a loading state instead of waiting text before the first response', () => {
    render(<AuthorizationPanel data={undefined} isLoading error={null} />);

    expect(screen.getByText(/reading the authorization state/i)).toBeTruthy();
    expect(screen.queryByText(/no authorization has been observed/i)).toBeNull();
  });

  it('should show the error state when it has no data at all', () => {
    render(
      <AuthorizationPanel
        data={undefined}
        isLoading={false}
        error={new AdminApiError('relay unavailable')}
      />
    );

    expect(screen.getByText('relay unavailable')).toBeTruthy();
    expect(screen.queryByText(/no authorization has been observed/i)).toBeNull();
  });

  it('should keep the last known state and warn when a refresh fails', () => {
    render(<AuthorizationPanel data={observed} isLoading={false} error={new NetworkError()} />);

    expect(screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY)).toBeTruthy();
    expect(screen.getByText(/authorization observed/i)).toBeTruthy();
    expect(screen.getByText(/could not be refreshed/i)).toBeTruthy();
  });

  it('should say the state is not yet observed rather than absent', () => {
    // BE-FMAN-AUTH-001 owns the daemon state needed to say "there is none".
    render(<AuthorizationPanel data={waiting} isLoading={false} error={null} />);

    expect(screen.getByText(/has not been observed/i)).toBeTruthy();
    expect(screen.queryByText(/no holder has authorized/i)).toBeNull();
  });
});
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/shared/components/authorization-panel
```

- [ ] **Step 3: Write the module**

`AuthorizationPanel.module.css`:

```css
.root {
  @apply uStack4;
}

.qrBox {
  @apply flex w-fit flex-col items-center gap-3 rounded-card border border-muted bg-surface p-4;
}

.keyRow {
  @apply uCopyRow;
}

.key {
  @apply uLongIdText;
}

.loading {
  @apply uMutedSm;
}

.hint {
  @apply uMutedText;
}
```

`AuthorizationPanel.tsx`:

```tsx
import { Banner, CopyButton } from '@operator-ui/common-ui';
import type { OnboardingResponse } from '@operator-ui/types';
import { QRCodeSVG } from 'qrcode.react';
import { describeActionError } from '@/shared/utils/describeActionError';
import { buildAuthorizationPayload, isAuthorized } from '@/shared/utils/authorization';
import styles from './AuthorizationPanel.module.css';

const QR_SIZE = 192;

interface AuthorizationPanelProps {
  data: OnboardingResponse | undefined;
  isLoading: boolean;
  error: unknown;
}

// Renders state, never actions. The setup step and the standalone page need the
// same key, QR and status but entirely different controls, so the buttons stay
// with each surface.
export const AuthorizationPanel = ({ data, isLoading, error }: AuthorizationPanelProps) => {
  if (!data) {
    if (error) return <Banner variant="error">{describeActionError(error)}</Banner>;
    if (isLoading) return <p className={styles.loading}>Reading the authorization state…</p>;
    return null;
  }

  const payload = buildAuthorizationPayload(data);
  const authorized = isAuthorized(data);

  return (
    <div className={styles.root}>
      <div className={styles.qrBox}>
        <QRCodeSVG value={payload} size={QR_SIZE} marginSize={2} />
      </div>

      {/* Shown whole, unlike BackupPage, which truncates the same class of value.
          That page lists keys for reference; this one shows the single value a
          holder may compare against their own application, and a truncated value
          does not permit that comparison. */}
      <div className={styles.keyRow}>
        <code className={styles.key}>{payload}</code>

        <CopyButton value={payload} label="Copy service Nostr pubkey" />
      </div>

      <p className={styles.hint}>
        This is the fleet manager's service Nostr public key, as the daemon reports it. A holder or
        a holder tool signs an authorization over it.
      </p>
      {authorized ? (
        <Banner variant="success">Authorization observed. This fleet can be evaluated.</Banner>
      ) : (
        <Banner variant="info">
          An authorization has not been observed yet. The daemon may still be reading the relay.
        </Banner>
      )}
      {error ? (
        <Banner variant="warn">
          This state could not be refreshed: {describeActionError(error)}
        </Banner>
      ) : null}
    </div>
  );
};
```

- [ ] **Step 4: Run it and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/shared/components/authorization-panel
```

Expected: PASS, 7 tests. `CopyButton`'s accessible name comes from its `label` prop —
check `packages/shared-ui/src/components/copy-button/CopyButton.tsx` and adjust the query
in the test if the label is composed differently, rather than changing the shared
component.

- [ ] **Step 5: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/shared/components/authorization-panel
git commit -m "feat(fman-ui): add a shared authorization panel with the full service key"
```

---

### Task 3: The standalone Authorization page

**Files:**
- Create: `operator-ui/apps/fleet-manager/src/pages/authorization/AuthorizationPage.tsx`
- Create: `operator-ui/apps/fleet-manager/src/pages/authorization/AuthorizationPage.module.css`
- Test: `operator-ui/apps/fleet-manager/src/pages/authorization/__tests__/AuthorizationPage.test.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/app/components/navigation-items/nav-config.ts`
- Modify: `operator-ui/apps/fleet-manager/src/app/index.tsx`

**Interfaces:**
- Consumes: `AuthorizationPanel` (Task 2); `useAuthorizationWatch` (Task 1); `isAuthorized` (Task 1).
- Produces: `AuthorizationPage`, routed at `/authorization`. `NAV_ITEMS` gains `{ key: 'authorization', label: 'Authorization', path: '/authorization' }` **immediately after Overview**, always present.

The page has **no skip, no continue and no redirect**. It shows the panel plus the
observed holder list, with no spinner.

- [ ] **Step 1: Write the failing test**

```tsx
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import * as adminCallModule from '@/shared/api/adminCall';
import { AuthorizationPage } from '../AuthorizationPage';

const waiting = {
  fman_name: 'mutual-hamster',
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'waiting_for_authorization' }
};

const observed = {
  ...waiting,
  nostr: { state: 'authorization_observed', authorizations: [], holders: [MOCK_HOLDER_PUBKEY] }
};

const renderPage = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <AuthorizationPage />
      </MemoryRouter>
    </QueryClientProvider>
  );
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('AuthorizationPage', () => {
  it('should show the waiting state with the full key', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(waiting);
    renderPage();

    await screen.findByText(MOCK_SERVICE_NOSTR_PUBKEY);
    expect(screen.getByText(/has not been observed/i)).toBeTruthy();
  });

  it('should list the holders once an authorization is observed', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    renderPage();

    await screen.findByText(MOCK_HOLDER_PUBKEY);
    expect(screen.getByText(/authorization observed/i)).toBeTruthy();
  });

  it('should offer no way to skip or continue', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(waiting);
    renderPage();

    await screen.findByText(MOCK_SERVICE_NOSTR_PUBKEY);
    expect(screen.queryByRole('button', { name: /skip/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /continue/i })).toBeNull();
  });
});
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/pages/authorization
```

- [ ] **Step 3: Write the page and its module**

`AuthorizationPage.module.css`:

```css
.root {
  @apply uStack6;
}

.heading {
  @apply uPageHeading;
}

.intro {
  @apply uIntro;
}

.holders {
  @apply uBorderedPanel;
}

.holdersHeading {
  @apply text-sm font-medium text-ink/60;
}

.holderList {
  @apply uStack1;
}

.holder {
  @apply uLongIdText;
}
```

`AuthorizationPage.tsx`:

```tsx
import { SectionCard } from '@operator-ui/common-ui';
import { useAuthorizationWatch } from '@/shared/api/hooks/use-authorization-watch/useAuthorizationWatch';
import { AuthorizationPanel } from '@/shared/components/authorization-panel/AuthorizationPanel';
import styles from './AuthorizationPage.module.css';

const renderHolder = (holder: string) => (
  <li key={holder} className={styles.holder}>
    {holder}
  </li>
);

export const AuthorizationPage = () => {
  const onboarding = useAuthorizationWatch();
  const nostr = onboarding.data?.nostr;
  const holders = nostr?.state === 'authorization_observed' ? nostr.holders : [];

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Authorization</h1>

      <p className={styles.intro}>
        Until a holder has authorized this fleet manager, initiators have no way to evaluate it.
        This page stays available for as long as the fleet runs.
      </p>

      <AuthorizationPanel
        data={onboarding.data}
        isLoading={onboarding.isLoading}
        error={onboarding.error}
      />
      {holders.length > 0 ? (
        <SectionCard title="Observed holders">
          <ul className={styles.holderList}>{holders.map(renderHolder)}</ul>
        </SectionCard>
      ) : null}
    </div>
  );
};
```

The page folder must contain only these three entries plus `__tests__/`. The `structure`
gate fails on anything else.

- [ ] **Step 4: Add the navigation item and the route**

`nav-config.ts`:

```ts
export const NAV_ITEMS: NavItem[] = [
  { key: 'overview', label: 'Overview', path: '/' },
  { key: 'authorization', label: 'Authorization', path: '/authorization' },
  { key: 'seats', label: 'Seats', path: '/seats' },
  { key: 'wallet', label: 'Wallet', path: '/wallet' },
  { key: 'backup', label: 'Backup', path: '/backup' }
];
```

`app/index.tsx` — add the import and the route inside the `AppShell` children, after the
index route:

```tsx
import { AuthorizationPage } from '@/pages/authorization/AuthorizationPage';
```

```tsx
                  { index: true, element: <OverviewPage /> },
                  { path: 'authorization', element: <AuthorizationPage /> },
```

No navigation badge. The Overview attention item added in Task 4 covers the need with a
mechanism that already exists.

- [ ] **Step 5: Fix the navigation tests**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/app/components/navigation-items
```

Any test asserting an exact item count or an exact label list needs `Authorization` added
in second position.

- [ ] **Step 6: Run the suite and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman test
```

- [ ] **Step 7: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/pages/authorization operator-ui/apps/fleet-manager/src/app
git commit -m "feat(fman-ui): make authorization a permanent screen in the main navigation"
```

---

### Task 4: The Overview signpost

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/features/overview/utils/deriveOverview.ts`
- Modify: `operator-ui/apps/fleet-manager/src/features/overview/utils/__tests__/deriveOverview.test.ts`
- Modify: `operator-ui/apps/fleet-manager/src/pages/overview/OverviewPage.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/pages/overview/__tests__/OverviewPage.test.tsx`

**Interfaces:**
- Consumes: `OnboardingNostrStatus` from `@operator-ui/types`; `useOnboarding` from `@/shared/api/hooks/use-onboarding/useOnboarding`.
- Produces: `OverviewInputs` gains an optional `nostrState?: OnboardingNostrStatus['state']`. `deriveOverview`'s return shape is unchanged.

Copy rule, from D4: the item says the authorization **has not been observed**, and its
detail explains that the daemon may still be checking the relay. It must **not** state
that the fleet has no authorization — `BE-FMAN-AUTH-001` owns the daemon state needed for
a definitive message.

- [ ] **Step 1: Write the failing tests**

Append to `features/overview/utils/__tests__/deriveOverview.test.ts`:

```ts
it('should raise an attention item when the authorization has not been observed', () => {
  const model = deriveOverview({ nostrState: 'waiting_for_authorization' });

  const item = model.attention.find((entry) => entry.key === 'authorization-not-observed');
  expect(item?.title).toBe('Authorization has not been observed');
  expect(item?.path).toBe('/authorization');
});

it('should not claim the fleet has no authorization', () => {
  const model = deriveOverview({ nostrState: 'waiting_for_authorization' });

  const item = model.attention.find((entry) => entry.key === 'authorization-not-observed');
  expect(item?.detail).toMatch(/may still be reading the relay/i);
  expect(item?.detail).not.toMatch(/no authorization/i);
});

it('should raise nothing once the authorization is observed', () => {
  const model = deriveOverview({ nostrState: 'authorization_observed' });

  expect(model.attention).toHaveLength(0);
});

it('should raise nothing when the state is unknown', () => {
  const model = deriveOverview({});

  expect(model.attention).toHaveLength(0);
});
```

- [ ] **Step 2: Run them and confirm they fail**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/overview/utils
```

- [ ] **Step 3: Extend `deriveOverview`**

Add the import, extend `OverviewInputs`, destructure the new field, and push the item
after the existing payment-federation checks:

```ts
import type { OnboardingNostrStatus, PaymentFederation, Plan } from '@operator-ui/types';
```

```ts
export interface OverviewInputs {
  paymentFederations?: PaymentFederation[];
  plans?: Plan[];
  /** Absent while the Onboarding query has not answered. The Overview says nothing
   *  rather than guessing. */
  nostrState?: OnboardingNostrStatus['state'];
}
```

```ts
export const deriveOverview = ({
  paymentFederations = [],
  plans = [],
  nostrState
}: OverviewInputs): OverviewModel => {
```

```ts
  // The daemon reports `waiting_for_authorization` both for a fleet nobody has
  // authorized and for one whose relay it has not read yet (design record F3), so
  // this item reports what is known — that nothing has been observed — and not
  // what is not. BE-FMAN-AUTH-001 is what would let it be definite.
  if (nostrState === 'waiting_for_authorization') {
    attention.push({
      key: 'authorization-not-observed',
      title: 'Authorization has not been observed',
      detail:
        'The daemon may still be reading the relay. Open Authorization to see the current state.',
      path: '/authorization'
    });
  }
```

- [ ] **Step 4: Pass the state from the page**

In `OverviewPage.tsx`, add the query and thread the state through. Do **not** add
`onboarding` to the `isLoading` / `isError` gates: the Overview must still render when the
authorization state is unknown.

```tsx
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
```

```tsx
  const onboarding = useOnboarding();
```

```tsx
  const model = deriveOverview({
    paymentFederations: paymentFederations.data?.federations,
    plans,
    nostrState: onboarding.data?.nostr.state
  });
```

- [ ] **Step 5: Cover it on the page**

Add to `pages/overview/__tests__/OverviewPage.test.tsx` a case that stubs `Onboarding`
with `waiting_for_authorization` and asserts the attention row is rendered and links to
`/authorization`. Follow whatever stubbing shape the file already uses.

- [ ] **Step 6: Run the suite and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman test
```

- [ ] **Step 7: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/features/overview operator-ui/apps/fleet-manager/src/pages/overview
git commit -m "feat(fman-ui): signpost an unobserved authorization from the Overview"
```

---

### Task 5: Automatic continue on the setup authorization step

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-authorization/SetupAuthorization.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-authorization/SetupAuthorization.module.css`
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-authorization/__tests__/SetupAuthorization.test.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-wizard/__tests__/SetupWizard.test.tsx`

**Interfaces:**
- Consumes: `AuthorizationPanel` (Task 2); `useAuthorizationWatch`, `isAuthorized` (Task 1).
- Produces: `SetupAuthorization` keeps its props `{ onSettled: () => void }`. It now calls `onSettled` **at most once**.

Behaviour from D5 and D6:

- While the authorization is **not** observed: `Skip for now` and a disabled `Continue`, exactly as today. Skipping is what makes D6's poll-stop test meaningful.
- Once it **is** observed: `Skip for now` disappears; a `role="status"` line reads
  `Authorization observed.` and `Continuing to the price step…`; a spinner turns; after
  about 2 seconds `onSettled()` runs; a `Continue now` button stays active for an operator
  who does not want to wait.
- The button row keeps its position, so `Continue` does not move under the pointer.
- Under `prefers-reduced-motion` the spinner does not turn; the text carries the message.
- The timer is cleared on unmount.
- **One shared guard protects both the timer and the manual button**, so `onSettled()`
  runs once when both fire in the same event window.
- There is no shared `Spinner` component. The only one lives inside `Button`. This screen
  gets a local one — a CSS-only element in this module, not a new component. The project
  extracts on the third use; this is the second.
- The screen's intro copy loses the claim that a holder scans the QR with their app. The
  panel now owns that wording.

- [ ] **Step 1: Write the failing tests**

Replace the `'should confirm and allow continuing once the authorization is observed'`
case, and add the rest:

```tsx
it('should continue on its own once the authorization is observed', async () => {
  vi.useFakeTimers();
  try {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    const { onSettled } = renderAuthorization();

    await vi.waitFor(() => expect(screen.getByRole('status').textContent).toMatch(/observed/i));
    expect(screen.queryByRole('button', { name: 'Skip for now' })).toBeNull();
    expect(onSettled).not.toHaveBeenCalled();

    await act(async () => {
      vi.advanceTimersByTime(2000);
    });

    expect(onSettled).toHaveBeenCalledTimes(1);
  } finally {
    vi.useRealTimers();
  }
});

it('should continue once when the timer and the manual action race', async () => {
  vi.useFakeTimers();
  try {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    const { onSettled } = renderAuthorization();

    const button = await vi.waitFor(() => screen.getByRole('button', { name: 'Continue now' }));

    await act(async () => {
      fireEvent.click(button);
      vi.advanceTimersByTime(2000);
    });

    expect(onSettled).toHaveBeenCalledTimes(1);
  } finally {
    vi.useRealTimers();
  }
});

it('should clear its timer when it unmounts', async () => {
  vi.useFakeTimers();
  try {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const onSettled = vi.fn();
    const { unmount } = render(
      <QueryClientProvider client={client}>
        <SetupAuthorization onSettled={onSettled} />
      </QueryClientProvider>
    );

    await vi.waitFor(() => expect(screen.getByRole('status')).toBeTruthy());
    unmount();

    await act(async () => {
      vi.advanceTimersByTime(5000);
    });

    expect(onSettled).not.toHaveBeenCalled();
  } finally {
    vi.useRealTimers();
  }
});

it('should stop its three-second poll after a skip', async () => {
  // D6: the fast observer lives inside this screen, so unmounting it is what stops
  // the poll. A later change that hoists the hook cannot pass this test.
  vi.useFakeTimers();
  try {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(waiting);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { unmount } = render(
      <QueryClientProvider client={client}>
        <SetupAuthorization onSettled={() => undefined} />
      </QueryClientProvider>
    );

    await vi.waitFor(() => expect(adminCall).toHaveBeenCalled());
    fireEvent.click(screen.getByRole('button', { name: 'Skip for now' }));
    unmount();

    const callsAtSkip = adminCall.mock.calls.length;
    // Past one 3-second watch interval, and well below the 60-second gate poll.
    await act(async () => {
      vi.advanceTimersByTime(10_000);
    });

    expect(adminCall.mock.calls.length).toBe(callsAtSkip);
  } finally {
    vi.useRealTimers();
  }
});
```

Add `act` and `render` to the Testing Library import, and `QueryClient`,
`QueryClientProvider` if they are not already imported. Keep the existing cases for the
waiting text, the skip callback and the error state; the assertion for the key moves to
the panel's own test, so the `'should show the key an attester signs over'` case may
assert on `MOCK_SERVICE_NOSTR_PUBKEY` still being rendered through the panel.

- [ ] **Step 2: Run them and confirm they fail**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/components/setup-authorization
```

- [ ] **Step 3: Rewrite the component**

```tsx
import { Button } from '@operator-ui/common-ui';
import { useEffect, useRef } from 'react';
import { useAuthorizationWatch } from '@/shared/api/hooks/use-authorization-watch/useAuthorizationWatch';
import { AuthorizationPanel } from '@/shared/components/authorization-panel/AuthorizationPanel';
import { isAuthorized } from '@/shared/utils/authorization';
import styles from './SetupAuthorization.module.css';

const CONTINUE_DELAY_MS = 2_000;

interface SetupAuthorizationProps {
  onSettled: () => void;
}

export const SetupAuthorization = ({ onSettled }: SetupAuthorizationProps) => {
  const onboarding = useAuthorizationWatch();
  const authorized = isAuthorized(onboarding.data);
  // One guard for both routes out of this screen. The timer and the manual button
  // can land in the same event window, and the wizard must advance once.
  const hasSettled = useRef(false);

  const settleOnce = () => {
    if (hasSettled.current) return;
    hasSettled.current = true;
    onSettled();
  };

  useEffect(() => {
    if (!authorized) return;

    const timer = setTimeout(() => {
      if (hasSettled.current) return;
      hasSettled.current = true;
      onSettled();
    }, CONTINUE_DELAY_MS);

    return () => clearTimeout(timer);
  }, [authorized, onSettled]);

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Get this fleet authorized</h1>

        <p className={styles.intro}>
          A holder signs an authorization binding this fleet manager's key. Until one is published,
          initiators have no way to evaluate you.
        </p>
      </div>

      <AuthorizationPanel
        data={onboarding.data}
        isLoading={onboarding.isLoading}
        error={onboarding.error}
      />
      {authorized ? (
        <p className={styles.statusLine} role="status">
          <span className={styles.spinner} aria-hidden="true" />
          Authorization observed. Continuing to the price step…
        </p>
      ) : null}

      <div className={styles.actions}>
        {authorized ? null : (
          <Button variant="secondary" onClick={settleOnce}>
            Skip for now
          </Button>
        )}
        {authorized ? (
          <Button onClick={settleOnce}>Continue now</Button>
        ) : (
          <Button disabled>Continue</Button>
        )}
      </div>
    </div>
  );
};
```

Reading and writing `hasSettled.current` inside the effect and inside an event handler is
allowed; the React Compiler rule forbids it **during render**, which this does not do.

- [ ] **Step 4: Extend the module**

Remove the `.payload` rule — the panel owns the key now — and add:

```css
.statusLine {
  @apply uRowCenter3 text-sm text-ink;
}

.spinner {
  @apply inline-block h-4 w-4 shrink-0 animate-spin rounded-full border-2 border-muted border-t-ink motion-reduce:animate-none;
}
```

Also remove `.qrBox` if nothing in the file still uses it — the panel carries its own.
An orphan rule is reported by the `fallow` gate.

- [ ] **Step 5: Fix the wizard test**

`SetupWizard.test.tsx`'s `'should complete setup once the price is stored'` still clicks
`Continue` on the authorization step. With automatic continue that click is gone; the
step advances by itself. Replace the click with an await on the price heading:

```tsx
  await screen.findByRole('heading', { name: 'Get this fleet authorized' });
  await screen.findByRole('heading', { name: 'Set your price' });
```

If the two-second delay makes the wait exceed the default timeout, give that
`findByRole` an explicit `{ timeout: 5000 }` rather than mocking timers in an
integration test.

`'should walk a new fleet from the doors to the price step'` stubs an **unauthorized**
daemon, so it still clicks `Skip for now`. Leave it as it is.

- [ ] **Step 6: Run the suite and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman test
```

- [ ] **Step 7: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/features/setup
git commit -m "feat(fman-ui): continue automatically once the authorization is observed"
```

---

### Task 6: The gate surface store

**Files:**
- Create: `operator-ui/apps/fleet-manager/src/shared/surface/gateSurface.ts`
- Create: `operator-ui/apps/fleet-manager/src/shared/surface/__tests__/gateSurface.test.ts`
- Modify: `operator-ui/apps/fleet-manager/src/app/components/boot-gate/BootGate.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/app/components/setup-gate/SetupGate.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/app/components/mock-panel-mount/MockPanelMount.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/mocks/routes.ts`
- Modify: `operator-ui/apps/fleet-manager/src/mocks/__tests__/routes.test.ts`
- Modify: `operator-ui/apps/fleet-manager/src/mocks/scenarios.ts`

**Interfaces:**
- Consumes: nothing.
- Produces, from `@/shared/surface/gateSurface`:
  - `type GateOwner = 'boot' | 'setup'`
  - `type GateSurface = 'boot' | 'auth' | 'daemon-error' | 'setup'`
  - `const gateSurface: { set(owner: GateOwner, surface: GateSurface): void; clear(owner: GateOwner): void; getSnapshot(): GateSurface | null; subscribe(listener: () => void): () => void }`

Why this exists, from F6: four surfaces are rendered by gates and have no path of their
own — the setup wizard, the sign-in prompt, the daemon-error screen and the boot screen.
Each inherits the last pathname, usually `/`, so the mock panel reports "Overview". The
store lives in `shared`, not in `mocks`, because the gates must not import `@/mocks/*`;
that rule is what keeps the mock world out of production bundles.

Resolution order: a `boot` owner's value wins, then `setup`, then the caller's pathname.
A parent's cleanup must not be able to clear a child owner's value, which is what keying
by owner buys.

- [ ] **Step 1: Write the failing test**

```ts
import { afterEach, describe, expect, it, vi } from 'vitest';
import { gateSurface } from '@/shared/surface/gateSurface';

afterEach(() => {
  gateSurface.clear('boot');
  gateSurface.clear('setup');
});

describe('gateSurface', () => {
  it('should report nothing while no gate owns a surface', () => {
    expect(gateSurface.getSnapshot()).toBeNull();
  });

  it('should report the setup surface when only the setup gate owns one', () => {
    gateSurface.set('setup', 'setup');

    expect(gateSurface.getSnapshot()).toBe('setup');
  });

  it('should prefer the boot surface over the setup surface', () => {
    gateSurface.set('setup', 'setup');
    gateSurface.set('boot', 'auth');

    expect(gateSurface.getSnapshot()).toBe('auth');
  });

  it('should fall back to the setup surface when the boot gate clears its own', () => {
    gateSurface.set('setup', 'setup');
    gateSurface.set('boot', 'auth');
    gateSurface.clear('boot');

    expect(gateSurface.getSnapshot()).toBe('setup');
  });

  it('should not let one owner clear another owner value', () => {
    gateSurface.set('setup', 'setup');
    gateSurface.clear('boot');

    expect(gateSurface.getSnapshot()).toBe('setup');
  });

  it('should notify subscribers on every change', () => {
    const listener = vi.fn();
    const unsubscribe = gateSurface.subscribe(listener);

    gateSurface.set('boot', 'boot');
    gateSurface.clear('boot');
    unsubscribe();
    gateSurface.set('boot', 'daemon-error');

    expect(listener).toHaveBeenCalledTimes(2);
  });

  it('should keep a stable snapshot while nothing changes', () => {
    gateSurface.set('boot', 'boot');

    expect(gateSurface.getSnapshot()).toBe(gateSurface.getSnapshot());
  });
});
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/shared/surface
```

- [ ] **Step 3: Write the store**

```ts
export type GateOwner = 'boot' | 'setup';

export type GateSurface = 'boot' | 'auth' | 'daemon-error' | 'setup';

/** BootGate outranks SetupGate: it sits above it, so whatever it is showing is
 *  what is on the screen. */
const PRIORITY: readonly GateOwner[] = ['boot', 'setup'];

const owned = new Map<GateOwner, GateSurface>();
const listeners = new Set<() => void>();

const announce = () => {
  for (const listener of listeners) listener();
};

/**
 * Which surface a gate is rendering, keyed by the gate that owns it.
 *
 * Four surfaces have no route of their own — the boot screen, the sign-in prompt,
 * the daemon-error screen and the setup wizard — so a pathname cannot name them.
 * Each gate declares and retracts only its own value, which is what stops a
 * parent's cleanup from clearing a child's.
 *
 * Lives in `shared`, not in `mocks`: `BootGate` and `SetupGate` are production
 * components and must not import `@/mocks/*`, or the mock world reaches the
 * production bundle.
 */
export const gateSurface = {
  set(owner: GateOwner, surface: GateSurface): void {
    if (owned.get(owner) === surface) return;
    owned.set(owner, surface);
    announce();
  },

  clear(owner: GateOwner): void {
    if (!owned.delete(owner)) return;
    announce();
  },

  getSnapshot(): GateSurface | null {
    for (const owner of PRIORITY) {
      const surface = owned.get(owner);
      if (surface) return surface;
    }
    return null;
  },

  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }
};
```

`getSnapshot` returns a string or `null`, both stable by value, so it is safe for
`useSyncExternalStore`.

- [ ] **Step 4: Let each gate declare its surface**

`BootGate.tsx` — add the effect above the returns:

```tsx
import { useEffect } from 'react';
import { Outlet } from 'react-router-dom';
import { gateSurface } from '@/shared/surface/gateSurface';
```

```tsx
  useEffect(() => {
    if (status === 'ready') {
      gateSurface.clear('boot');
      return;
    }

    const surface = status === 'booting' ? 'boot' : status === 'needs-auth' ? 'auth' : 'daemon-error';
    gateSurface.set('boot', surface);

    return () => gateSurface.clear('boot');
  }, [status]);
```

`SetupGate.tsx` — the same shape, driven by `isSettingUp`:

```tsx
  useEffect(() => {
    if (!isSettingUp) return;

    gateSurface.set('setup', 'setup');

    return () => gateSurface.clear('setup');
  }, [isSettingUp]);
```

Both are effects, not render-time writes: writing to an external store during render
breaks the React Compiler's purity rule. StrictMode's double invoke runs
set → cleanup → set, which the owner key makes idempotent.

- [ ] **Step 5: Read the resolved surface in the mock panel**

In `MockPanelMount.tsx`:

- import `useSyncExternalStore` from `react` and `gateSurface` from
  `@/shared/surface/gateSurface`;
- add `const surface = useSyncExternalStore(gateSurface.subscribe, gateSurface.getSnapshot);`
  beside the existing `useLocation`;
- rename the lazy component's prop from `pathname` to `routeKey` and resolve it at the
  mount, so `@/mocks/routes` still stays inside the lazy factory:

```tsx
interface BoundMockPanelProps {
  pathname: string;
  surface: string | null;
}
```

```tsx
      const BoundMockPanel = ({ pathname, surface }: BoundMockPanelProps) => (
        <Panel
          store={mockStore}
          catalog={scenarioCatalog}
          config={panelConfig}
          verbLog={verbLog}
          routeKey={surface ?? routeToKey(pathname)}
          appName="Fleet Manager"
        />
      );
```

```tsx
    <Suspense fallback={null}>
      <MockPanel pathname={pathname} surface={surface} />
    </Suspense>
```

A gate surface is already a route key, so it needs no translation. Leave the
`mocksEnabled` guard and the comment above it untouched: it is what keeps `@/mocks/*` out
of the production chunk graph.

- [ ] **Step 6: Give the mock catalog the new keys**

`mocks/routes.ts` — add the pattern, keeping the longest-first ordering:

```ts
  { pattern: /^\/authorization\/?$/, key: 'authorization' },
```

Place it above the `/^\/$/` entry. Add a case to `mocks/__tests__/routes.test.ts` proving
`routeToKey('/authorization')` returns `'authorization'`.

`mocks/scenarios.ts` — `awaiting-authorization` gains the new key:

```ts
  'awaiting-authorization': {
    desc: 'Onboarded, but no holder has authorized it yet — the QR step is still waiting.',
    affects: ['setup', 'authorization', 'backup', 'overview']
  },
```

`not-onboarded` keeps `affects: ['setup']`. With the surface store in place, `setup` is a
value the panel can now actually resolve, so both entries reach a page tab for the first
time.

- [ ] **Step 7: Cover the gate transitions**

Add cases to `app/components/mock-panel-mount/__tests__/MockPanelMount.test.tsx`, or to
the gate tests where the existing harness makes it simpler, covering:

- boot → setup;
- auth → setup;
- setup → a route (the store empties and the pathname takes over);
- cleanup under React StrictMode — mounting the gate inside `<StrictMode>` and asserting
  the surface is still set after the double invoke.

- [ ] **Step 8: Run the suite and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman test
```

- [ ] **Step 9: Commit**

```bash
git add operator-ui/apps/fleet-manager/src
git commit -m "fix(fman-ui): let each gate name the surface the mock panel is showing"
```

---

### Task 7: Interruption signposts

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/pages/backup/BackupPage.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/pages/backup/BackupPage.module.css`
- Modify: `operator-ui/apps/fleet-manager/src/pages/backup/__tests__/BackupPage.test.tsx`

**Interfaces:**
- Consumes: nothing new.
- Produces: nothing new.

D8: the UI cannot know whether an operator recorded the recovery phrase, and it cannot
restore the in-memory wizard step after a reload. `BE-FMAN-SETUP-002` owns the production
answer. This phase keeps every safe action reachable after a reload, and claims nothing
more:

- Backup links to the recovery phrase and states that the browser did not save it. **This task.**
- Authorization stays available from the main navigation. **Task 3.**
- Overview shows the current offer through `Change price`. **Already true** —
  `OfferSummary` renders that link today. No change needed; confirm it, do not duplicate it.

- [ ] **Step 1: Write the failing test**

Add to `pages/backup/__tests__/BackupPage.test.tsx`:

```tsx
it('should state that the browser did not keep the recovery phrase', async () => {
  // The wizard's step lives in memory only, so a reload during setup loses it. The
  // phrase is still reachable here, and this line says so without claiming the
  // operator ever wrote it down.
  renderBackupPage();

  await screen.findByText(/did not save your recovery phrase/i);
  expect(screen.getByRole('link', { name: 'Reveal recovery phrase' })).toBeTruthy();
});
```

Use whatever render helper the file already defines instead of `renderBackupPage` if the
name differs.

- [ ] **Step 2: Run it and confirm it fails**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/pages/backup
```

- [ ] **Step 3: Add the signpost**

In `BackupPage.tsx`, immediately above the existing `Link`:

```tsx
      <p className={styles.reloadNote}>
        This dashboard did not save your recovery phrase, and it cannot tell whether you wrote it
        down. If setup was interrupted, reveal it here and record it now.
      </p>
```

In `BackupPage.module.css`:

```css
.reloadNote {
  @apply uMutedText;
}
```

- [ ] **Step 4: Run every gate**

```bash
bash operator-ui/scripts/harness-gate.sh typecheck
bash operator-ui/scripts/harness-gate.sh lint
bash operator-ui/scripts/harness-gate.sh style
bash operator-ui/scripts/harness-gate.sh structure
bash operator-ui/scripts/harness-gate.sh fallow
bash operator-ui/scripts/harness-gate.sh test
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/pages/backup
git commit -m "feat(fman-ui): signpost the recovery phrase after an interrupted setup"
```

---

## Acceptance criteria

- [ ] `buildAuthorizationPayload` and `isAuthorized` live in `shared/utils/authorization.ts`; `isNotOnboardedError` stays in `features/setup/utils/setupState.ts`.
- [ ] `useAuthorizationWatch` lives under `shared/api/hooks/`, and no file imports it from `features/setup`.
- [ ] `AuthorizationPanel` renders the service Nostr key in full with a copy control, renders no actions, and distinguishes loading, no-data error, and cached-data refresh error.
- [ ] No screen claims a holder app can scan the QR and complete the flow.
- [ ] `/authorization` is routed, is second in `NAV_ITEMS`, and its page has no skip, no continue and no redirect.
- [ ] `deriveOverview` raises `authorization-not-observed` for `waiting_for_authorization`, links it to `/authorization`, and never states that no authorization exists.
- [ ] `SetupAuthorization` continues automatically about two seconds after the authorization is observed, offers `Continue now`, calls `onSettled` exactly once when the timer and the button race, clears its timer on unmount, and stops its 3-second poll after a skip.
- [ ] The spinner does not turn under `prefers-reduced-motion`.
- [ ] `gateSurface` resolves boot over setup over pathname, and one owner cannot clear another's value. Boot→setup, auth→setup, setup→route and StrictMode cleanup are covered.
- [ ] `MockPanelMount` reports the gate surface; `routes.ts` resolves `/authorization`.
- [ ] `BackupPage` states that the browser did not save the recovery phrase.
- [ ] No `app` gate component imports `@/mocks/*`.
- [ ] `pnpm-lock.yaml` is unchanged. All seven harness gates pass.

## Out of scope

- Mock transport failures, the typed debug controls, the `authorization-observed` and `authorization-read-error` scenarios, and the recovery-result variants in the mock verbs. Those are Plan C.
- Tuning the 3-second watch interval. The daemon reads the relay every 15 seconds, so the dashboard already asks about five times more often than a new answer can appear. This is existing behaviour and is left alone.
- A navigation badge for the authorization state. The Overview attention item covers the need with a mechanism that already exists.
- Any daemon change. `BE-FMAN-AUTH-001` and `BE-FMAN-AUTH-002` own the backend side.
