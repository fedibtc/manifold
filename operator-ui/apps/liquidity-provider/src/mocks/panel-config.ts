import type { PanelConfig } from '@operator-ui/mock-devtools';
import type { ServiceErrorCode } from '@operator-ui/types';
import { getState, type MockState, patchState, setForcedError } from '@/mocks/state';
import { adminMethods } from '@/mocks/world/verbs';

/** The codes `handlers.ts` maps to an HTTP status, plus the bare `503` express
 *  accepted for a transport failure with no service error behind it. */
const ERROR_CODES = [
  'invalid_argument',
  'failed_precondition',
  'permission_denied',
  'not_found',
  'unavailable',
  'internal',
  'unknown',
  '503'
] as const;

const PHASES = ['9', '10', '11'] as const;

const BOOT_MODES = ['normal', 'restore'] as const;

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
      id: 'phase',
      label: 'Daemon phase',
      kind: 'select',
      options: PHASES,
      read: () => String(getState().phase),
      write: (value) => patchState({ phase: Number(value) as MockState['phase'] })
    },
    {
      // The only way to reach the restore console in mock mode. The panel mounts
      // above BootGate precisely so that selecting `restore` does not strand it.
      id: 'bootMode',
      label: 'Boot mode',
      kind: 'select',
      options: BOOT_MODES,
      read: () => getState().bootMode,
      write: (value) => patchState({ bootMode: value as MockState['bootMode'] })
    }
  ],

  errors: {
    // Read off the verb map, so this cannot drift from what is actually routed.
    // `GET /health` is absent by design: it never consults `forcedErrors`.
    verbs: adminMethods,
    codes: ERROR_CODES,
    // A fresh object per read: the world mutates `forcedErrors` in place, so
    // the panel could not otherwise tell an injection from an empty map.
    active: () => ({ ...getState().forcedErrors }) as Record<string, string>,
    set: (verb, code) => setForcedError(verb, code as ServiceErrorCode | '503' | null)
  },

  patch: (path, value) => patchState({ path, value })
};
