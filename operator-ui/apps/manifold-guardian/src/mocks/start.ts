import {
  getState,
  type MockState,
  type PatchInput,
  patchState,
  setForcedError,
  setState
} from '@/mocks/state';
import { mockStore } from '@/mocks/store';

export interface MockControl {
  active: boolean;
  getScenario: () => string;
  setScenario: (name: string) => void;
  reset: () => void;
  getState: () => MockState;
  /** Browser stand-in for express's `POST /__control/patch`. */
  patch: (input: PatchInput) => void;
  /** `POST /__control/errors`. A null message clears the injection. */
  setError: (method: string, message: string | null) => void;
  /** `POST /__control/state`. */
  setState: (next: MockState) => void;
}

declare global {
  interface Window {
    __mockControl?: MockControl;
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

  // The only scripted entry point into mock state. Defined here and nowhere
  // else, so it cannot exist in a production build or in daemon mode. Each
  // control below is the same state function the dev panel calls, so the
  // scripted and hand-driven surfaces cannot drift.
  window.__mockControl = {
    active: true,
    getScenario: () => mockStore.getScenario(),
    setScenario: (name) => mockStore.setScenario(name),
    reset: () => mockStore.reset(),
    getState,
    patch: patchState,
    setError: setForcedError,
    setState
  };
};
