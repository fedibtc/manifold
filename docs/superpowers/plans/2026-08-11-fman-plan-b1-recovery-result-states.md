# FMan Plan B1 — recovery result states and recovery journey

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the FMan recovery screen real result states — success with exact counts, a full-screen daemon failure, and an unknown network result — and route every recovery to the authorization step instead of branching on a value the daemon cannot supply yet.

**Architecture:** `SetupRestore` stops letting the mutation decide what is on screen. It owns a `RestoreViewState` and selects one of three sibling result components from it. The safe parts of the daemon response and of the error are copied into that state, the mutation is reset, and the recovery phrase never survives settlement. Separately, `onRestored` loses its boolean, `handleRestored` loses its refetch, and both identity-creation mutations reset the `Onboarding` query and wait for one fresh response before the wizard moves on.

**Tech Stack:** TypeScript, React 19 (React Compiler), TanStack Query v5, Vitest, Testing Library, Tailwind v3 + CSS Modules.

**Source of record:** `docs/superpowers/specs/2026-08-11-fman-recovery-authorization-design.md`, sections D1 and D2.

**Depends on:** Plan A (`2026-08-11-fman-plan-a-key-format-and-price-bound.md`). Plan A's canonical hexadecimal key constants must already be on the branch — the tests in this plan use them.

## Global Constraints

- All work is under `operator-ui/apps/fleet-manager`. No Rust change. No daemon change.
- **Do not modify `pnpm-lock.yaml`.** Do not add or remove any package dependency. A lockfile change is a hard policy block that cannot be repaired.
- **Do not edit anything under `packages/biome-plugins`, `tasks/`, or `.github/`.**
- Components are arrow-function consts with a named export. No `function` declarations, no `React.FC`.
- Every component declares an `XProps` interface. Never inline the prop object type.
- One React unit (component **or** hook) per file. A utility file exports neither, so it may export any number of helpers.
- Every feature component sits in its own kebab-case folder with a colocated `PascalName.module.css` and a `__tests__/PascalName.test.tsx`. The `structure` gate blocks a loose or untested component.
- Absolute imports only: `@/` within the app, `@operator-ui/*` across packages. The one exception is a component importing its own `./X.module.css`.
- No Tailwind utility strings in TSX. `className` holds only `styles.*` references. Prefer the shared `u*` utilities already used by the setup screens: `uStack4`, `uStack1`, `uPageHeading`, `uPageHead`, `uIntro`, `uLabel`, `uFieldError`, `uFormActions`, `uMutedText`, `uMonoValue`, `uBorderedPanel`, `uKvRow`.
- React Compiler rules: render is pure, no setState inside `useEffect`. The sanctioned shape for "adjust state on data change" is a **guarded setState during render**, as `SetupGate.tsx:21` and `useBootStatus.ts:29` already do.
- Vitest, `it("should …")`. Restore any overridden global in an `afterEach`.
- **The recovery phrase is secret material.** It must not reach a component's props, a query cache, a mutation's retained variables, a log line, or an error message. The only places it may exist are the textarea's own state and the in-flight request body.
- Run the gates locally before each commit:
  ```bash
  bash operator-ui/scripts/harness-gate.sh typecheck && bash operator-ui/scripts/harness-gate.sh lint && bash operator-ui/scripts/harness-gate.sh style && bash operator-ui/scripts/harness-gate.sh structure && bash operator-ui/scripts/harness-gate.sh test
  ```

---

## File Structure

**Create**

| File | Responsibility |
|---|---|
| `features/setup/utils/restoreViewState.ts` | The `RestoreViewState` union, the safe result and error shapes, and the pure classifier that turns an unknown thrown value into one of them. |
| `features/setup/utils/__tests__/restoreViewState.test.ts` | Covers the classifier for each error class. |
| `features/setup/components/setup-restore-success/SetupRestoreSuccess.tsx` (+ `.module.css`, `__tests__/`) | The success screen: exact counts, the zero-seat call-out, `Continue` only. |
| `features/setup/components/setup-restore-failed/SetupRestoreFailed.tsx` (+ `.module.css`, `__tests__/`) | The daemon-refusal screen: the daemon's own message, `Try again` and `Back to setup options`. |
| `features/setup/components/setup-restore-unknown/SetupRestoreUnknown.tsx` (+ `.module.css`, `__tests__/`) | The unknown-result screen: `Check status`, then either back to the form or forward without counts. |

**Modify**

| File | Change |
|---|---|
| `features/setup/components/setup-restore/SetupRestore.tsx` | Owns the view state, selects the screen, resets the mutation, disables browser text services on the phrase field. |
| `features/setup/components/setup-restore/SetupRestore.module.css` | Styles for the new form-level pieces. |
| `features/setup/components/setup-restore/__tests__/SetupRestore.test.tsx` | New coverage for every result state. |
| `features/setup/api/hooks/use-onboard-from-backup/useOnboardFromBackup.ts` | `gcTime: 0`; reset and await a fresh `Onboarding` on success. |
| `features/setup/api/hooks/use-onboard-as-new/useOnboardAsNew.ts` | Reset and await a fresh `Onboarding` on success. |
| `features/setup/hooks/use-setup-wizard/useSetupWizard.ts` | `onRestored` loses its boolean. |
| `features/setup/components/setup-wizard/SetupWizard.tsx` | `handleRestored` disappears; the restore step calls `wizard.onRestored` directly. |
| `features/setup/components/setup-wizard/__tests__/SetupWizard.test.tsx` | The two branch tests collapse into one; a stale-cache regression test is added. |

All paths are relative to `operator-ui/apps/fleet-manager/src/`.

---

### Task 1: The view-state vocabulary and the error classifier

**Files:**
- Create: `operator-ui/apps/fleet-manager/src/features/setup/utils/restoreViewState.ts`
- Test: `operator-ui/apps/fleet-manager/src/features/setup/utils/__tests__/restoreViewState.test.ts`

**Interfaces:**
- Consumes: `AdminApiError`, `AuthError`, `NetworkError` from `@/shared/api/errors`; `describeActionError` from `@/shared/utils/describeActionError`.
- Produces, all imported by later tasks from `@/features/setup/utils/restoreViewState`:
  - `interface SafeRestoreResult { seats: number; formed: number }`
  - `type RestoreErrorClass = 'daemon' | 'auth' | 'network'`
  - `interface SafeRestoreError { errorClass: RestoreErrorClass; message: string }`
  - `type RestoreViewState = { type: 'form' } | { type: 'success'; result: SafeRestoreResult } | { type: 'failed'; error: SafeRestoreError } | { type: 'unknown'; error: SafeRestoreError }`
  - `const classifyRestoreError: (error: unknown) => SafeRestoreError`

- [ ] **Step 1: Write the failing test**

Create `operator-ui/apps/fleet-manager/src/features/setup/utils/__tests__/restoreViewState.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { AdminApiError, AuthError, NetworkError } from '@/shared/api/errors';
import { classifyRestoreError } from '@/features/setup/utils/restoreViewState';

describe('classifyRestoreError', () => {
  it('should carry a daemon refusal through unchanged', () => {
    const error = new AdminApiError('seat directory already exists: /var/lib/fman/seats/abc');

    expect(classifyRestoreError(error)).toEqual({
      errorClass: 'daemon',
      message: 'seat directory already exists: /var/lib/fman/seats/abc'
    });
  });

  it('should classify an authentication refusal', () => {
    expect(classifyRestoreError(new AuthError())).toEqual({
      errorClass: 'auth',
      message: 'Your session expired. Sign in again.'
    });
  });

  it('should classify a transport failure', () => {
    expect(classifyRestoreError(new NetworkError())).toEqual({
      errorClass: 'network',
      message: "Can't reach the fleet manager. Try again once it's back online."
    });
  });

  it('should treat an unrecognised throw as a transport failure', () => {
    // A thrown value with no error class could be anything, including a response
    // the daemon acted on. Unknown is the only safe reading.
    expect(classifyRestoreError('boom')).toEqual({
      errorClass: 'network',
      message: 'Something went wrong. Please try again.'
    });
  });
});
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/utils/__tests__/restoreViewState.test.ts
```

Expected: FAIL — cannot resolve `@/features/setup/utils/restoreViewState`.

- [ ] **Step 3: Write the module**

Create `operator-ui/apps/fleet-manager/src/features/setup/utils/restoreViewState.ts`:

```ts
import { AdminApiError, AuthError } from '@/shared/api/errors';
import { describeActionError } from '@/shared/utils/describeActionError';

/** The parts of `OnboardFromBackupResponse` the screen shows. Deliberately not the
 *  response itself: nothing that ever held the phrase enters view state. */
export interface SafeRestoreResult {
  seats: number;
  formed: number;
}

/**
 * `daemon`  — the daemon refused before it installed the identity.
 * `auth`    — the authentication middleware refused before dispatch.
 * `network` — the result is unknown; the daemon may have installed the identity.
 */
export type RestoreErrorClass = 'daemon' | 'auth' | 'network';

export interface SafeRestoreError {
  errorClass: RestoreErrorClass;
  message: string;
}

export type RestoreViewState =
  | { type: 'form' }
  | { type: 'success'; result: SafeRestoreResult }
  | { type: 'failed'; error: SafeRestoreError }
  | { type: 'unknown'; error: SafeRestoreError };

/**
 * The daemon's restore errors already name the cause and the action, so a daemon
 * refusal is passed through word for word. Classifying it in the dashboard would
 * mean matching prose, which `BE-FMAN-RECOVERY-003` exists to remove.
 *
 * Anything that is neither a daemon refusal nor an authentication refusal is read
 * as an unknown result, including a bare thrown value: the browser cannot tell a
 * lost response from a request that never arrived.
 */
export const classifyRestoreError = (error: unknown): SafeRestoreError => {
  if (error instanceof AdminApiError) return { errorClass: 'daemon', message: error.message };
  if (error instanceof AuthError) return { errorClass: 'auth', message: describeActionError(error) };
  return { errorClass: 'network', message: describeActionError(error) };
};
```

- [ ] **Step 4: Run it and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/utils/__tests__/restoreViewState.test.ts
```

Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/features/setup/utils/restoreViewState.ts operator-ui/apps/fleet-manager/src/features/setup/utils/__tests__/restoreViewState.test.ts
git commit -m "feat(fman-ui): add a safe view-state vocabulary for the recovery screen"
```

---

### Task 2: The recovery success screen

**Files:**
- Create: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore-success/SetupRestoreSuccess.tsx`
- Create: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore-success/SetupRestoreSuccess.module.css`
- Test: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore-success/__tests__/SetupRestoreSuccess.test.tsx`

**Interfaces:**
- Consumes: `SafeRestoreResult` from `@/features/setup/utils/restoreViewState` (Task 1); `Banner`, `Button`, `SectionCard` from `@operator-ui/common-ui`.
- Produces: `SetupRestoreSuccess` with props `{ result: SafeRestoreResult; onContinue: () => void }`. Task 5 renders it.

Wording rules taken from the design record, section D1. Do not paraphrase them:

- `seats` is labelled **seat records**, `formed` is labelled **records that include guardian configuration**. Neither count may be called *active*, *running* or *live*. The daemon response does not support those claims.
- When `seats` is `0`, the screen states that no seat records were found and lists the safe possibilities: an empty fleet, missing relay records, the wrong environment, or another valid phrase. It also states that the daemon has installed the identity and that setup cannot be repeated on this host.
- The only action is `Continue`. There is no `Back`: a `Back` control would suggest an undo that does not exist.

- [ ] **Step 1: Write the failing test**

Create `.../setup-restore-success/__tests__/SetupRestoreSuccess.test.tsx`:

```tsx
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { SetupRestoreSuccess } from '../SetupRestoreSuccess';

describe('SetupRestoreSuccess', () => {
  it('should state the exact counts the daemon returned', () => {
    render(<SetupRestoreSuccess result={{ seats: 2, formed: 1 }} onContinue={vi.fn()} />);

    expect(screen.getByText('2')).toBeTruthy();
    expect(screen.getByText(/seat records/i)).toBeTruthy();
    expect(screen.getByText('1')).toBeTruthy();
    expect(screen.getByText(/guardian configuration/i)).toBeTruthy();
  });

  it('should not claim a recovered seat record is running', () => {
    render(<SetupRestoreSuccess result={{ seats: 2, formed: 1 }} onContinue={vi.fn()} />);

    expect(screen.queryByText(/running/i)).toBeNull();
    expect(screen.queryByText(/\bactive\b/i)).toBeNull();
  });

  it('should call out a recovery that found no seat records', () => {
    render(<SetupRestoreSuccess result={{ seats: 0, formed: 0 }} onContinue={vi.fn()} />);

    expect(screen.getByText(/no seat records/i)).toBeTruthy();
    expect(screen.getByText(/another valid phrase/i)).toBeTruthy();
    expect(screen.getByText(/cannot repeat setup on this host/i)).toBeTruthy();
  });

  it('should offer continue and nothing else', () => {
    const onContinue = vi.fn();
    render(<SetupRestoreSuccess result={{ seats: 0, formed: 0 }} onContinue={onContinue} />);

    expect(screen.getAllByRole('button')).toHaveLength(1);

    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(onContinue).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/components/setup-restore-success
```

Expected: FAIL — cannot resolve `../SetupRestoreSuccess`.

- [ ] **Step 3: Write the component and its module**

`SetupRestoreSuccess.module.css`:

```css
.root {
  @apply uStack4;
}

.head {
  @apply uPageHead;
}

.heading {
  @apply uPageHeading;
}

.intro {
  @apply uIntro;
}

.counts {
  @apply uBorderedPanel;
}

.countRow {
  @apply uKvRow;
}

.countValue {
  @apply text-2xl font-semibold text-ink;
}

.countLabel {
  @apply text-sm text-ink/60;
}

.reasons {
  @apply list-disc space-y-1 pl-5 text-sm text-ink/70;
}

.actions {
  @apply uFormActions;
}
```

`SetupRestoreSuccess.tsx`:

```tsx
import { Banner, Button } from '@operator-ui/common-ui';
import type { SafeRestoreResult } from '@/features/setup/utils/restoreViewState';
import styles from './SetupRestoreSuccess.module.css';

interface SetupRestoreSuccessProps {
  result: SafeRestoreResult;
  onContinue: () => void;
}

// The daemon answers `{ onboarded, seats, formed }` and nothing more. It does not
// say a seat is running, so neither does this screen: `seats` is a count of seat
// records recovered, `formed` the subset that carries guardian configuration.
export const SetupRestoreSuccess = ({ result, onContinue }: SetupRestoreSuccessProps) => {
  const foundNothing = result.seats === 0;

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Recovery finished</h1>

        <p className={styles.intro}>
          This host now carries the identity behind that phrase.
        </p>
      </div>

      <dl className={styles.counts}>
        <div className={styles.countRow}>
          <dd className={styles.countValue}>{result.seats}</dd>

          <dt className={styles.countLabel}>seat records recovered</dt>
        </div>

        <div className={styles.countRow}>
          <dd className={styles.countValue}>{result.formed}</dd>

          <dt className={styles.countLabel}>of them include guardian configuration</dt>
        </div>
      </dl>
      {foundNothing ? (
        <Banner variant="warn">
          <p>
            No seat records were found for this phrase. That has several possible causes, and this
            screen cannot tell them apart:
          </p>

          <ul className={styles.reasons}>
            <li>the fleet never sold a seat;</li>

            <li>its records are not on the relay this host reads;</li>

            <li>this host points at a different environment;</li>

            <li>the phrase is valid but belongs to another fleet.</li>
          </ul>

          <p>
            The daemon has already installed this identity, so you cannot repeat setup on this host.
            Check the environment and the relay before you continue.
          </p>
        </Banner>
      ) : null}

      <div className={styles.actions}>
        <Button onClick={onContinue}>Continue</Button>
      </div>
    </div>
  );
};
```

- [ ] **Step 4: Run it and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/components/setup-restore-success
```

Expected: PASS, 4 tests. If `Banner` does not accept element children, wrap the copy in a
single `<div>` inside it rather than changing the shared component.

- [ ] **Step 5: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore-success
git commit -m "feat(fman-ui): add the recovery success screen with exact seat counts"
```

---

### Task 3: The recovery failure screen

**Files:**
- Create: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore-failed/SetupRestoreFailed.tsx`
- Create: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore-failed/SetupRestoreFailed.module.css`
- Test: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore-failed/__tests__/SetupRestoreFailed.test.tsx`

**Interfaces:**
- Consumes: `SafeRestoreError` from `@/features/setup/utils/restoreViewState` (Task 1).
- Produces: `SetupRestoreFailed` with props `{ error: SafeRestoreError; onTryAgain: () => void; onBackToDoors: () => void }`. Task 5 renders it.

- [ ] **Step 1: Write the failing test**

```tsx
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { SetupRestoreFailed } from '../SetupRestoreFailed';

const daemonError = {
  errorClass: 'daemon' as const,
  message: 'seat directory already exists: /var/lib/fman/seats/abc — remove it and retry'
};

describe('SetupRestoreFailed', () => {
  it('should show the daemon message word for word', () => {
    render(
      <SetupRestoreFailed error={daemonError} onTryAgain={vi.fn()} onBackToDoors={vi.fn()} />
    );

    expect(screen.getByText(daemonError.message)).toBeTruthy();
  });

  it('should offer a retry that returns to the form', () => {
    const onTryAgain = vi.fn();
    render(
      <SetupRestoreFailed error={daemonError} onTryAgain={onTryAgain} onBackToDoors={vi.fn()} />
    );

    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(onTryAgain).toHaveBeenCalled();
  });

  it('should offer a way back to the setup options', () => {
    const onBackToDoors = vi.fn();
    render(
      <SetupRestoreFailed error={daemonError} onTryAgain={vi.fn()} onBackToDoors={onBackToDoors} />
    );

    fireEvent.click(screen.getByRole('button', { name: 'Back to setup options' }));
    expect(onBackToDoors).toHaveBeenCalled();
  });

  it('should state that nothing was installed', () => {
    render(
      <SetupRestoreFailed error={daemonError} onTryAgain={vi.fn()} onBackToDoors={vi.fn()} />
    );

    expect(screen.getByText(/host still has no identity/i)).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/components/setup-restore-failed
```

- [ ] **Step 3: Write the component and its module**

`SetupRestoreFailed.module.css`:

```css
.root {
  @apply uStack4;
}

.head {
  @apply uPageHead;
}

.heading {
  @apply uPageHeading;
}

.intro {
  @apply uIntro;
}

.message {
  @apply uBorderedPanel uMonoValue whitespace-pre-wrap;
}

.actions {
  @apply uFormActions;
}
```

`SetupRestoreFailed.tsx`:

```tsx
import { Banner, Button } from '@operator-ui/common-ui';
import type { SafeRestoreError } from '@/features/setup/utils/restoreViewState';
import styles from './SetupRestoreFailed.module.css';

interface SetupRestoreFailedProps {
  error: SafeRestoreError;
  onTryAgain: () => void;
  onBackToDoors: () => void;
}

// The message is shown, not interpreted. The daemon's restore errors already name
// the cause and the action; re-deriving that here would mean matching prose, which
// `BE-FMAN-RECOVERY-003` exists to make unnecessary.
export const SetupRestoreFailed = ({
  error,
  onTryAgain,
  onBackToDoors
}: SetupRestoreFailedProps) => (
  <div className={styles.root}>
    <div className={styles.head}>
      <h1 className={styles.heading}>Recovery did not complete</h1>

      <p className={styles.intro}>
        The fleet manager refused the request, so this host still has no identity. Nothing was
        written.
      </p>
    </div>

    <Banner variant="error">The fleet manager said:</Banner>

    <p className={styles.message}>{error.message}</p>

    <div className={styles.actions}>
      <Button variant="secondary" onClick={onBackToDoors}>
        Back to setup options
      </Button>

      <Button onClick={onTryAgain}>Try again</Button>
    </div>
  </div>
);
```

- [ ] **Step 4: Run it and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/components/setup-restore-failed
```

Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore-failed
git commit -m "feat(fman-ui): add a full-screen recovery failure state"
```

---

### Task 4: The unknown-result screen

**Files:**
- Create: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore-unknown/SetupRestoreUnknown.tsx`
- Create: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore-unknown/SetupRestoreUnknown.module.css`
- Test: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore-unknown/__tests__/SetupRestoreUnknown.test.tsx`

**Interfaces:**
- Consumes: `SafeRestoreError` from `@/features/setup/utils/restoreViewState` (Task 1).
- Produces: `SetupRestoreUnknown` with props
  `{ error: SafeRestoreError; isChecking: boolean; identityConfirmed: boolean; onCheckStatus: () => void; onContinue: () => void }`.
  Task 5 owns the check itself and drives these props. This component makes no admin call of its own.

Behaviour, from the design record, section D1:

- The screen never offers another restore. The daemon may already have installed the identity.
- Before a check: `Check status` only.
- While checking: the action is disabled and reads as busy.
- After a check that confirms an identity: state that the identity exists but the recovery counts are unavailable, and offer `Continue`.

- [ ] **Step 1: Write the failing test**

```tsx
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { SetupRestoreUnknown } from '../SetupRestoreUnknown';

const networkError = {
  errorClass: 'network' as const,
  message: "Can't reach the fleet manager. Try again once it's back online."
};

const props = {
  error: networkError,
  isChecking: false,
  identityConfirmed: false,
  onCheckStatus: vi.fn(),
  onContinue: vi.fn()
};

describe('SetupRestoreUnknown', () => {
  it('should say the result is unknown rather than failed', () => {
    render(<SetupRestoreUnknown {...props} />);

    expect(screen.getByText(/we do not know whether the recovery finished/i)).toBeTruthy();
  });

  it('should never offer another recovery attempt', () => {
    render(<SetupRestoreUnknown {...props} />);

    expect(screen.queryByRole('button', { name: /recover/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /try again/i })).toBeNull();
  });

  it('should ask the daemon for its status', () => {
    const onCheckStatus = vi.fn();
    render(<SetupRestoreUnknown {...props} onCheckStatus={onCheckStatus} />);

    fireEvent.click(screen.getByRole('button', { name: 'Check status' }));
    expect(onCheckStatus).toHaveBeenCalled();
  });

  it('should disable the check while one is in flight', () => {
    render(<SetupRestoreUnknown {...props} isChecking />);

    const button = screen.getByRole('button', { name: 'Check status' }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
  });

  it('should continue without counts once an identity is confirmed', () => {
    const onContinue = vi.fn();
    render(<SetupRestoreUnknown {...props} identityConfirmed onContinue={onContinue} />);

    expect(screen.getByText(/recovery counts are not available/i)).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(onContinue).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run it and confirm it fails**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/components/setup-restore-unknown
```

- [ ] **Step 3: Write the component and its module**

`SetupRestoreUnknown.module.css`:

```css
.root {
  @apply uStack4;
}

.head {
  @apply uPageHead;
}

.heading {
  @apply uPageHeading;
}

.intro {
  @apply uIntro;
}

.detail {
  @apply uMutedText;
}

.actions {
  @apply uFormActions;
}
```

`SetupRestoreUnknown.tsx`:

```tsx
import { Banner, Button } from '@operator-ui/common-ui';
import type { SafeRestoreError } from '@/features/setup/utils/restoreViewState';
import styles from './SetupRestoreUnknown.module.css';

interface SetupRestoreUnknownProps {
  error: SafeRestoreError;
  isChecking: boolean;
  identityConfirmed: boolean;
  onCheckStatus: () => void;
  onContinue: () => void;
}

// A lost response is not a failure. The daemon may have installed the identity
// before the browser lost the answer, so this screen offers a status check and
// never another restore. `BE-FMAN-RECOVERY-002` replaces the inference with an
// explicit operation result.
export const SetupRestoreUnknown = ({
  error,
  isChecking,
  identityConfirmed,
  onCheckStatus,
  onContinue
}: SetupRestoreUnknownProps) => (
  <div className={styles.root}>
    <div className={styles.head}>
      <h1 className={styles.heading}>Recovery result unknown</h1>

      <p className={styles.intro}>
        The connection dropped before the fleet manager answered, so we do not know whether the
        recovery finished. Running it again could not be undone, so this screen checks instead.
      </p>
    </div>

    <Banner variant="warn">{error.message}</Banner>
    {identityConfirmed ? (
      <p className={styles.detail}>
        This host has an identity, so the recovery did complete. The recovery counts are not
        available, because the answer that carried them was lost.
      </p>
    ) : null}

    <div className={styles.actions}>
      {identityConfirmed ? (
        <Button onClick={onContinue}>Continue</Button>
      ) : (
        <Button disabled={isChecking} loading={isChecking} onClick={onCheckStatus}>
          Check status
        </Button>
      )}
    </div>
  </div>
);
```

- [ ] **Step 4: Run it and confirm it passes**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/components/setup-restore-unknown
```

Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore-unknown
git commit -m "feat(fman-ui): add an unknown-result state for a lost recovery response"
```

---

### Task 5: `SetupRestore` owns the view state

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore/SetupRestore.tsx`
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore/SetupRestore.module.css`
- Test: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore/__tests__/SetupRestore.test.tsx`

**Interfaces:**
- Consumes: `RestoreViewState`, `classifyRestoreError` (Task 1); `SetupRestoreSuccess` (Task 2); `SetupRestoreFailed` (Task 3); `SetupRestoreUnknown` (Task 4); `useOnboardFromBackup` from `@/features/setup/api/hooks/use-onboard-from-backup/useOnboardFromBackup`; `ONBOARDING_KEY` from `@/shared/api/hooks/use-onboarding/useOnboarding`; `isNotOnboardedError` from `@/features/setup/utils/setupState`.
- Produces: `SetupRestore` keeps its existing props `{ onRestored: () => void; onCancel: () => void }`. **`onRestored` takes no argument** — Task 6 removes the boolean from the wizard side.

Design rules from D1, all of which the tests below pin:

- The component owns `RestoreViewState`. The mutation carries the request only. An idle mutation must not be able to select a screen, so the mutation is reset once its result has been copied into view state.
- The restore mutation uses `gcTime: 0` (Task 7), so TanStack Query keeps no mutation variables — and therefore no phrase — after settlement.
- The textarea sets `autoComplete="off"`, `autoCapitalize="none"`, `autoCorrect="off"` and `spellCheck={false}`.
- `Try again` returns to the form **with the phrase still in the field** — 12 words are easy to mistype.
- `Back to setup options` returns to the doors **and clears the visible field**.
- An `AuthError` awaits `queryClient.refetchQueries({ queryKey: ONBOARDING_KEY, exact: true })`, clears the phrase, and stays on the form. The awaited refetch is what opens the sign-in gate; `useBootStatus` reads authentication errors from the `Onboarding` query only, so without it the gate would stay shut for up to 60 seconds. An invalidation is not a substitute: this path needs an immediate, deterministic state change a test can assert.

- [ ] **Step 1: Write the failing tests**

Replace the body of `.../setup-restore/__tests__/SetupRestore.test.tsx` with the
following. Keep any existing test that still describes current behaviour, but the cases
below are required.

```tsx
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError, AuthError, NetworkError } from '@/shared/api/errors';
import { SetupRestore } from '../SetupRestore';

const PHRASE = 'abandon abandon abandon abandon abandon abandon abandon abandon about';

const renderRestore = (onRestored = vi.fn(), onCancel = vi.fn()) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <SetupRestore onRestored={onRestored} onCancel={onCancel} />
    </QueryClientProvider>
  );
  return { onRestored, onCancel, client };
};

const fillAndSubmit = () => {
  fireEvent.change(screen.getByLabelText('Recovery phrase'), { target: { value: PHRASE } });
  fireEvent.click(screen.getByLabelText(/permanently offline/i));
  fireEvent.click(screen.getByRole('button', { name: 'Recover this fleet' }));
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SetupRestore', () => {
  it('should disable browser text services on the phrase field', () => {
    renderRestore();

    const field = screen.getByLabelText('Recovery phrase') as HTMLTextAreaElement;
    expect(field.getAttribute('autocomplete')).toBe('off');
    expect(field.getAttribute('autocapitalize')).toBe('none');
    expect(field.getAttribute('autocorrect')).toBe('off');
    expect(field.spellcheck).toBe(false);
  });

  it('should show the success state with the daemon counts', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      onboarded: 'restored',
      seats: 2,
      formed: 1
    });
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery finished' });
    expect(screen.getByText(/seat records recovered/i)).toBeTruthy();
  });

  it('should keep showing the result after the mutation is reset', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      onboarded: 'restored',
      seats: 2,
      formed: 1
    });
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery finished' });

    // The mutation is reset once its result is copied into view state, so an idle
    // mutation must not be able to send the screen back to the form.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.getByRole('heading', { name: 'Recovery finished' })).toBeTruthy();
  });

  it('should only continue when the operator asks', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      onboarded: 'restored',
      seats: 2,
      formed: 1
    });
    const { onRestored } = renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery finished' });
    expect(onRestored).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(onRestored).toHaveBeenCalled();
  });

  it('should show a daemon refusal as a full screen', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AdminApiError('invalid mnemonic'));
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery did not complete' });
    expect(screen.getByText('invalid mnemonic')).toBeTruthy();
  });

  it('should return to the form with the phrase intact on try again', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AdminApiError('invalid mnemonic'));
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery did not complete' });
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

    const field = (await screen.findByLabelText('Recovery phrase')) as HTMLTextAreaElement;
    expect(field.value).toBe(PHRASE);
  });

  it('should clear the phrase when the operator goes back to the setup options', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AdminApiError('invalid mnemonic'));
    const { onCancel } = renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery did not complete' });
    fireEvent.click(screen.getByRole('button', { name: 'Back to setup options' }));

    expect(onCancel).toHaveBeenCalled();
  });

  it('should show the unknown result for a transport failure', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new NetworkError());
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery result unknown' });
  });

  it('should continue without counts when the status check finds an identity', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new NetworkError())
      .mockResolvedValue({
        fman_name: 'mutual-hamster',
        service_pubkey: '02abc',
        service_nostr_pubkey: 'a'.repeat(64),
        nostr: { state: 'waiting_for_authorization' }
      });
    const { onRestored } = renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery result unknown' });
    fireEvent.click(screen.getByRole('button', { name: 'Check status' }));

    await screen.findByText(/recovery counts are not available/i);
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

    expect(onRestored).toHaveBeenCalled();
    expect(adminCall).toHaveBeenCalledWith('Onboarding');
  });

  it('should return to the form when the status check says the host is not onboarded', async () => {
    vi.spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new NetworkError())
      .mockRejectedValue(
        new AdminApiError(
          'this Fleet Manager has not been onboarded yet: run `admin onboard new` or `admin onboard restore`'
        )
      );
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery result unknown' });
    fireEvent.click(screen.getByRole('button', { name: 'Check status' }));

    await screen.findByRole('heading', { name: 'Recover from your phrase' });
  });

  it('should refresh the onboarding query when authentication is refused', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AuthError());
    const { client } = renderRestore();
    const refetch = vi.spyOn(client, 'refetchQueries');
    fillAndSubmit();

    await waitFor(() =>
      expect(refetch).toHaveBeenCalledWith({ queryKey: ['onboarding'], exact: true })
    );

    const field = (await screen.findByLabelText('Recovery phrase')) as HTMLTextAreaElement;
    expect(field.value).toBe('');
  });
});
```

- [ ] **Step 2: Run them and confirm they fail**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/components/setup-restore
```

Expected: most of the new cases FAIL. The component still calls `onRestored` on success
and renders a single line of red text on failure.

- [ ] **Step 3: Rewrite the component**

Replace `.../setup-restore/SetupRestore.tsx` with:

```tsx
import { Banner, Button, CheckboxField } from '@operator-ui/common-ui';
import { useQueryClient } from '@tanstack/react-query';
import { type FormEvent, useState } from 'react';
import { useOnboardFromBackup } from '@/features/setup/api/hooks/use-onboard-from-backup/useOnboardFromBackup';
import { SetupRestoreFailed } from '@/features/setup/components/setup-restore-failed/SetupRestoreFailed';
import { SetupRestoreSuccess } from '@/features/setup/components/setup-restore-success/SetupRestoreSuccess';
import { SetupRestoreUnknown } from '@/features/setup/components/setup-restore-unknown/SetupRestoreUnknown';
import { isNotOnboardedError } from '@/features/setup/utils/setupState';
import {
  classifyRestoreError,
  type RestoreViewState
} from '@/features/setup/utils/restoreViewState';
import { ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import styles from './SetupRestore.module.css';

interface SetupRestoreProps {
  onRestored: () => void;
  onCancel: () => void;
}

export const SetupRestore = ({ onRestored, onCancel }: SetupRestoreProps) => {
  const restore = useOnboardFromBackup();
  const queryClient = useQueryClient();
  const [mnemonic, setMnemonic] = useState('');
  const [acknowledged, setAcknowledged] = useState(false);
  // The screen is selected from here, never from the mutation. The mutation is
  // reset as soon as its result has been copied across, and an idle mutation can
  // select nothing.
  const [view, setView] = useState<RestoreViewState>({ type: 'form' });
  const [isChecking, setIsChecking] = useState(false);
  const [identityConfirmed, setIdentityConfirmed] = useState(false);

  const handleMnemonicChange = (event: FormEvent<HTMLTextAreaElement>) => {
    setMnemonic(event.currentTarget.value);
  };

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    restore.mutate(
      { mnemonic: mnemonic.trim(), acknowledgeOriginalHostIsGone: acknowledged },
      {
        onSuccess: (response) => {
          setView({ type: 'success', result: { seats: response.seats, formed: response.formed } });
          restore.reset();
        },
        onError: async (error) => {
          const safe = classifyRestoreError(error);
          restore.reset();

          if (safe.errorClass === 'auth') {
            // The sign-in gate reads authentication errors off the Onboarding query
            // only, so a mutation error cannot open it. An awaited refetch — not an
            // invalidation — makes the gate open now instead of at the next poll.
            setMnemonic('');
            setView({ type: 'form' });
            await queryClient.refetchQueries({ queryKey: ONBOARDING_KEY, exact: true });
            return;
          }

          setView(
            safe.errorClass === 'daemon'
              ? { type: 'failed', error: safe }
              : { type: 'unknown', error: safe }
          );
        }
      }
    );
  };

  const handleTryAgain = () => {
    setView({ type: 'form' });
  };

  const handleBackToDoors = () => {
    setMnemonic('');
    onCancel();
  };

  // The daemon may have installed the identity before the response was lost, so the
  // only safe next move is to ask it what it now is.
  const handleCheckStatus = async () => {
    setIsChecking(true);
    await queryClient.refetchQueries({ queryKey: ONBOARDING_KEY, exact: true });
    setIsChecking(false);

    const state = queryClient.getQueryState(ONBOARDING_KEY);
    if (state?.data !== undefined) {
      setIdentityConfirmed(true);
      return;
    }
    if (isNotOnboardedError(state?.error)) {
      setView({ type: 'form' });
    }
    // Anything else leaves the screen where it is. A daemon that cannot be reached
    // at all is the BootGate's problem, not this screen's: it already shows the
    // daemon-unavailable screen, and after a reconnect the host's own state decides
    // whether setup reopens at the doors or the app opens with its signposts.
  };

  const canSubmit = mnemonic.trim().length > 0 && acknowledged && !restore.isPending;

  if (view.type === 'success') {
    return <SetupRestoreSuccess result={view.result} onContinue={onRestored} />;
  }

  if (view.type === 'failed') {
    return (
      <SetupRestoreFailed
        error={view.error}
        onTryAgain={handleTryAgain}
        onBackToDoors={handleBackToDoors}
      />
    );
  }

  if (view.type === 'unknown') {
    return (
      <SetupRestoreUnknown
        error={view.error}
        isChecking={isChecking}
        identityConfirmed={identityConfirmed}
        onCheckStatus={handleCheckStatus}
        onContinue={onRestored}
      />
    );
  }

  return (
    <div className={styles.root}>
      <h1 className={styles.heading}>Recover from your phrase</h1>

      <Banner variant="error">
        The guardians this phrase belongs to have been transferred to this host. Only continue if
        the original host is permanently offline — two hosts running one guardian identity will
        equivocate, and no check here can catch it.
      </Banner>

      <form className={styles.form} onSubmit={handleSubmit}>
        <label className={styles.label} htmlFor="recovery-phrase">
          Recovery phrase
        </label>

        <textarea
          id="recovery-phrase"
          className={styles.textarea}
          rows={3}
          value={mnemonic}
          autoComplete="off"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          onChange={handleMnemonicChange}
        />

        <CheckboxField
          label="I confirm the original host and its guardians are permanently offline"
          checked={acknowledged}
          onChange={setAcknowledged}
        />

        <div className={styles.actions}>
          <Button variant="secondary" onClick={handleBackToDoors}>
            Back
          </Button>

          <Button type="submit" disabled={!canSubmit} loading={restore.isPending}>
            Recover this fleet
          </Button>
        </div>
      </form>
    </div>
  );
};
```

Notes for the implementer:

- The `.error` rule in `SetupRestore.module.css` is now unused. Remove it — the `fallow`
  gate reports dead CSS, and an orphan created by this change is this task's to clean up.
- `describeActionError` is no longer imported here; the classifier owns that call. Remove
  the import.
- If `restore.reset()` inside `onSuccess`/`onError` warns about updating state during an
  unmounted render in tests, keep the order shown: set view state first, reset second.

- [ ] **Step 4: Run the tests and confirm they pass**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/components/setup-restore
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/features/setup/components/setup-restore
git commit -m "feat(fman-ui): let the recovery screen own and show its own result"
```

---

### Task 6: Every recovery reaches the authorization step

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/hooks/use-setup-wizard/useSetupWizard.ts:26-46`
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-wizard/SetupWizard.tsx:23-49`
- Test: `operator-ui/apps/fleet-manager/src/features/setup/components/setup-wizard/__tests__/SetupWizard.test.tsx`

**Interfaces:**
- Consumes: `SetupRestore` with `onRestored: () => void` (Task 5).
- Produces: `SetupWizard.onRestored` becomes `() => void`. `SetupWizard` no longer imports `isAuthorized` and no longer calls `onboarding.refetch()`.

Why, from the design record F2–F4: `fetch_holder_authorizations` derives its query key
from the recovery phrase, so a recovered host's authorization is still on the relay — but
`FleetManagerNostr::new` seeds the presence channel with `WaitingForAuthorization` and
only `run_onboarding` corrects it, after a relay connect and one fetch, on 15-second
intervals. So the refetch immediately after a recovery reports
`waiting_for_authorization` in almost every real run. The price branch is code a real
daemon does not reach.

- [ ] **Step 1: Rewrite the two wizard tests**

In `.../setup-wizard/__tests__/SetupWizard.test.tsx`, delete these two cases:

- `'should skip the QR when a restored fleet is already authorized'`
- `'should stop a restored fleet at the QR when the relay still reports it waiting'`

Replace them with one case plus one regression test:

```tsx
it('should always send a recovered fleet to the authorization step', async () => {
  // The daemon reports waiting_for_authorization right after a restore whether or
  // not an authorization exists (F3), so the wizard must not branch on it.
  stubDaemon(true);
  renderWizard();

  fireEvent.click(screen.getByRole('button', { name: 'Recover from a phrase' }));
  await screen.findByRole('heading', { name: 'Recover from your phrase' });

  fireEvent.change(screen.getByLabelText('Recovery phrase'), { target: { value: PHRASE } });
  fireEvent.click(screen.getByLabelText(/permanently offline/i));
  fireEvent.click(screen.getByRole('button', { name: 'Recover this fleet' }));

  await screen.findByRole('heading', { name: 'Recovery finished' });
  fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

  await screen.findByRole('heading', { name: 'Get this fleet authorized' });
});

it('should not use cached authorization data from an earlier identity', async () => {
  // A cached authorized Onboarding response belongs to whatever identity the host
  // carried before. Only the response fetched after the restore may decide the
  // next step.
  stubDaemon(false);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(['onboarding'], {
    fman_name: 'stale-fleet',
    service_pubkey: '02abc',
    service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
    nostr: { state: 'authorization_observed', authorizations: [], holders: [MOCK_HOLDER_PUBKEY] }
  });

  render(
    <QueryClientProvider client={client}>
      <SetupWizard onComplete={vi.fn()} />
    </QueryClientProvider>
  );

  fireEvent.click(screen.getByRole('button', { name: 'Recover from a phrase' }));
  await screen.findByRole('heading', { name: 'Recover from your phrase' });

  fireEvent.change(screen.getByLabelText('Recovery phrase'), { target: { value: PHRASE } });
  fireEvent.click(screen.getByLabelText(/permanently offline/i));
  fireEvent.click(screen.getByRole('button', { name: 'Recover this fleet' }));

  await screen.findByRole('heading', { name: 'Recovery finished' });
  fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

  await screen.findByRole('heading', { name: 'Get this fleet authorized' });
  await waitFor(() =>
    expect(screen.getByText(/Waiting for a holder to authorize this key/i)).toBeTruthy()
  );
});
```

The regression test needs `MOCK_SERVICE_NOSTR_PUBKEY` and `MOCK_HOLDER_PUBKEY` imported
from `@/mocks/world/keys` (added by Plan A) and `QueryClient`, `QueryClientProvider`
already imported at the top of the file.

Also update `'should complete setup once the price is stored'`: with the restore path
untouched it still walks the new-fleet route, so it keeps its `Continue` click on the
authorization step for now. Plan B2 changes that step to continue automatically and will
revisit this case. Leave it alone in this plan.

- [ ] **Step 2: Run the tests and confirm the new cases fail**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/components/setup-wizard
```

- [ ] **Step 3: Drop the boolean from the hook**

In `useSetupWizard.ts`, change the interface member and the implementation:

```ts
  /** A recovery always continues to the authorization step. The daemon reports
   *  `waiting_for_authorization` immediately after a restore whether or not an
   *  authorization exists, so there is no value here worth branching on. */
  onRestored: () => void;
```

```ts
    onRestored: () => setStep('authorization'),
```

- [ ] **Step 4: Drop the refetch from the wizard**

In `SetupWizard.tsx`, delete the `handleRestored` function together with its comment
block, delete the `isAuthorized` import and the `useOnboarding` import and call if they
become unused, and wire the restore step directly:

```tsx
        {wizard.step === 'restore' && (
          <SetupRestore onRestored={wizard.onRestored} onCancel={wizard.onBackToDoors} />
        )}
```

Check whether `onboarding` is still referenced anywhere in the file. If not, remove
`const onboarding = useOnboarding();` and its import — an unused binding fails the `lint`
gate.

- [ ] **Step 5: Run the tests and confirm they pass**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/features/setup
git commit -m "fix(fman-ui): route every recovery to the authorization step"
```

---

### Task 7: The onboarding mutations wait for a fresh identity

**Files:**
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/api/hooks/use-onboard-from-backup/useOnboardFromBackup.ts`
- Modify: `operator-ui/apps/fleet-manager/src/features/setup/api/hooks/use-onboard-as-new/useOnboardAsNew.ts`
- Test: `operator-ui/apps/fleet-manager/src/features/setup/api/hooks/use-onboard-from-backup/__tests__/useOnboardFromBackup.test.tsx` (create)
- Test: `operator-ui/apps/fleet-manager/src/features/setup/api/hooks/use-onboard-as-new/__tests__/useOnboardAsNew.test.tsx` (create)

**Interfaces:**
- Consumes: `ONBOARDING_KEY` from `@/shared/api/hooks/use-onboarding/useOnboarding`.
- Produces: both hooks keep their existing call signature. Both now settle only after a fresh `Onboarding` response has landed.

- [ ] **Step 1: Write the failing tests**

Create `.../use-onboard-from-backup/__tests__/useOnboardFromBackup.test.tsx`:

```tsx
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { useOnboardFromBackup } from '../useOnboardFromBackup';

const wrapper = (client: QueryClient) => {
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return Wrapper;
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useOnboardFromBackup', () => {
  it('should keep no mutation variables after settlement', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      onboarded: 'restored',
      seats: 1,
      formed: 1
    });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { result } = renderHook(() => useOnboardFromBackup(), { wrapper: wrapper(client) });

    result.current.mutate({ mnemonic: 'twelve words here', acknowledgeOriginalHostIsGone: true });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    result.current.reset();

    // gcTime: 0 means TanStack Query drops the mutation — and the phrase it carried
    // in `variables` — as soon as it is no longer observed.
    await waitFor(() => expect(client.getMutationCache().getAll()).toHaveLength(0));
  });

  it('should replace stale onboarding data with a fresh response', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockImplementation((request) =>
        request === 'Onboarding'
          ? Promise.resolve({
              fman_name: 'fresh',
              service_pubkey: '02abc',
              service_nostr_pubkey: 'b'.repeat(64),
              nostr: { state: 'waiting_for_authorization' }
            })
          : Promise.resolve({ onboarded: 'restored', seats: 1, formed: 1 })
      );

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(['onboarding'], {
      fman_name: 'stale',
      service_pubkey: '02abc',
      service_nostr_pubkey: 'a'.repeat(64),
      nostr: { state: 'authorization_observed', authorizations: [], holders: [] }
    });

    const { result } = renderHook(() => useOnboardFromBackup(), { wrapper: wrapper(client) });
    result.current.mutate({ mnemonic: 'twelve words here', acknowledgeOriginalHostIsGone: true });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));

    expect(adminCall).toHaveBeenCalledWith('Onboarding');
    expect(client.getQueryData(['onboarding'])).toMatchObject({ fman_name: 'fresh' });
  });
});
```

Create the equivalent `.../use-onboard-as-new/__tests__/useOnboardAsNew.test.tsx` with the
second test only, calling `result.current.mutate()` with no argument and stubbing
`OnboardAsNew` to resolve `{ onboarded: 'new', seats: 0 }`. `useOnboardAsNew` carries no
secret, so it needs no `gcTime` assertion.

- [ ] **Step 2: Run them and confirm they fail**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup/api
```

- [ ] **Step 3: Change `useOnboardFromBackup`**

```ts
import type { OnboardFromBackupResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';

export interface RestoreInput {
  mnemonic: string;
  acknowledgeOriginalHostIsGone: boolean;
}

export const useOnboardFromBackup = () => {
  const queryClient = useQueryClient();

  return useMutation({
    // The recovery phrase travels in `variables`, which TanStack Query keeps for
    // the mutation's lifetime. gcTime: 0 drops it the moment nothing observes the
    // mutation, so the phrase does not sit in memory after the screen has moved on.
    gcTime: 0,
    mutationFn: ({ mnemonic, acknowledgeOriginalHostIsGone }: RestoreInput) =>
      adminCall<OnboardFromBackupResponse>({
        OnboardFromBackup: {
          mnemonic,
          acknowledge_original_host_is_gone: acknowledgeOriginalHostIsGone
        }
      }),
    // The identity just changed, so every cached answer describes a host that no
    // longer exists. The awaited refetch is what lets the wizard read a response
    // that belongs to the identity it created, rather than the one it replaced.
    onSuccess: async () => {
      queryClient.removeQueries({ queryKey: ONBOARDING_KEY, exact: true });
      await queryClient.refetchQueries({ queryKey: ONBOARDING_KEY, exact: true });
      await queryClient.invalidateQueries();
    }
  });
};
```

- [ ] **Step 4: Change `useOnboardAsNew` the same way**

Apply the identical `onSuccess` body to `useOnboardAsNew`. Do **not** add `gcTime: 0`
there: `OnboardAsNew` carries no secret, and a needless cache setting invites the reader
to look for a reason that is not there.

- [ ] **Step 5: Run the tests and confirm they pass**

```bash
cd operator-ui && pnpm --filter fman exec vitest run src/features/setup
```

Expected: PASS. If `removeQueries` provokes a duplicate `Onboarding` call in the wizard
tests, keep it — the awaited `refetchQueries` is what the wizard depends on, and one extra
call during an identity change is not a regression. Adjust any test that asserted an exact
call count.

- [ ] **Step 6: Run every gate**

```bash
bash operator-ui/scripts/harness-gate.sh typecheck
bash operator-ui/scripts/harness-gate.sh lint
bash operator-ui/scripts/harness-gate.sh style
bash operator-ui/scripts/harness-gate.sh structure
bash operator-ui/scripts/harness-gate.sh fallow
bash operator-ui/scripts/harness-gate.sh test
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add operator-ui/apps/fleet-manager/src/features/setup/api
git commit -m "fix(fman-ui): make identity creation wait for a fresh onboarding response"
```

---

## Acceptance criteria

- [ ] `SetupRestore` selects its screen from its own view state, and the result stays on screen after the mutation is reset.
- [ ] The success screen states the daemon's `seats` and `formed` counts, labels them as records, and never calls a record active or running.
- [ ] A zero-seat recovery is called out with the four safe possibilities and offers `Continue` only.
- [ ] A daemon refusal replaces the screen, shows the daemon message unchanged, and offers `Try again` (phrase kept) and `Back to setup options` (phrase cleared).
- [ ] A transport failure produces the unknown result, never another restore, and its status check either continues without counts or returns to the form.
- [ ] An authentication refusal awaits `refetchQueries({ queryKey: ONBOARDING_KEY, exact: true })` and clears the phrase.
- [ ] The phrase field sets `autoComplete="off"`, `autoCapitalize="none"`, `autoCorrect="off"` and `spellCheck={false}`.
- [ ] `useOnboardFromBackup` sets `gcTime: 0`, and the mutation cache is empty after settlement.
- [ ] Both onboarding mutations reset and await a fresh `Onboarding` response.
- [ ] Recovery always reaches the authorization step; the two old branch tests are gone and one regression test proves a stale authorized cache cannot skip it.
- [ ] `SetupWizard` no longer imports `isAuthorized` and no longer refetches on restore.
- [ ] `pnpm-lock.yaml` is unchanged. All seven harness gates pass.

## Out of scope

- Moving `isAuthorized` / `buildAuthorizationPayload` into `shared`, the shared `AuthorizationPanel`, the standalone Authorization page, the navigation item, the Overview signpost, automatic continue, and the gate-surface store. Those are Plan B2.
- Mock transport failures and the debug-control matrix. That is Plan C.
- Any daemon change. `BE-FMAN-RECOVERY-001`, `-002` and `-003` own the backend side.
