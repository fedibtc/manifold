import type { PanelConfig } from '@operator-ui/mock-devtools';
import {
  getState,
  type OnboardingTransport,
  patchState,
  type RestoreResultChoice,
  type RestoreSession,
  type RestoreTransport,
  setForcedError
} from '@/mocks/state';
import { adminMethods } from '@/mocks/world/verbs';

/** FMan's forced errors are the message the daemon would return, not a code —
 *  `dispatch` answers `{ Err: message }`. These are the classes a recovery can
 *  actually fail with; `BE-FMAN-RECOVERY-003` is what would let the UI select an
 *  action from a code instead of the prose. An arbitrary message is still reachable
 *  through `window.__mockControl.setError`. */
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

const AUTH_MODES = ['password', 'trusted_proxy'] as const;

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

// A control only ever writes back a label it rendered, but the write arrives as a
// bare string, so the lookup goes through here rather than a cast per control.
const valueFor = <V extends string>(map: Record<string, V>, label: string): V =>
  map[label] ?? Object.values(map)[0];

export const panelConfig: PanelConfig = {
  controls: [
    {
      id: 'latencyMs',
      label: 'Latency (ms)',
      kind: 'number',
      read: () => String(getState().latencyMs),
      write: (value) => patchState({ latencyMs: Number(value) })
    },
    {
      id: 'authMode',
      label: 'Auth mode',
      kind: 'select',
      options: AUTH_MODES,
      read: () => getState().authMode,
      write: (value) => patchState({ authMode: value as (typeof AUTH_MODES)[number] })
    },
    {
      id: 'restoreResult',
      label: 'Restore result',
      kind: 'select',
      options: Object.keys(RESTORE_RESULTS),
      read: () => labelFor(RESTORE_RESULTS, getState().restoreResult),
      write: (value) =>
        patchState({ path: 'restoreResult', value: valueFor(RESTORE_RESULTS, value) })
    },
    {
      id: 'restoreTransport',
      label: 'Restore transport',
      kind: 'select',
      options: Object.keys(RESTORE_TRANSPORTS),
      read: () => labelFor(RESTORE_TRANSPORTS, getState().restoreTransport),
      write: (value) =>
        patchState({ path: 'restoreTransport', value: valueFor(RESTORE_TRANSPORTS, value) })
    },
    {
      id: 'restoreSession',
      label: 'Restore session',
      kind: 'select',
      options: Object.keys(RESTORE_SESSIONS),
      read: () => labelFor(RESTORE_SESSIONS, getState().restoreSession),
      write: (value) =>
        patchState({ path: 'restoreSession', value: valueFor(RESTORE_SESSIONS, value) })
    },
    {
      id: 'onboardingTransport',
      label: 'Onboarding transport',
      kind: 'select',
      options: Object.keys(ONBOARDING_TRANSPORTS),
      read: () => labelFor(ONBOARDING_TRANSPORTS, getState().onboardingTransport),
      write: (value) =>
        patchState({ path: 'onboardingTransport', value: valueFor(ONBOARDING_TRANSPORTS, value) })
    }
  ],

  errors: {
    // Read off the verb map, so this cannot drift from what is actually routed.
    verbs: adminMethods,
    codes: ERROR_MESSAGES,
    // A fresh object per read: the world mutates `forcedErrors` in place, so
    // the panel could not otherwise tell an injection from an empty map.
    active: () => ({ ...getState().forcedErrors }) as Record<string, string>,
    set: setForcedError
  },

  patch: (path, value) => patchState({ path, value })
};
