import { useCallback, useSyncExternalStore } from 'react';
import type { ScenarioStore } from '../types';

export interface ScenarioControls {
  scenario: string;
  /** Whether a control surface has overridden the world since the scenario
   *  loaded — read through the store subscription, so it is compiler-safe. */
  isDirty: boolean;
  setScenario: (name: string) => void;
  reset: () => void;
}

export const useScenario = <W>(store: ScenarioStore<W>): ScenarioControls => {
  const subscribe = useCallback((listener: () => void) => store.subscribe(listener), [store]);
  const scenario = useSyncExternalStore(subscribe, () => store.getScenario());
  const isDirty = useSyncExternalStore(subscribe, () => store.isDirty());

  const setScenario = useCallback((name: string) => store.setScenario(name), [store]);
  const reset = useCallback(() => store.reset(), [store]);

  return { scenario, isDirty, setScenario, reset };
};
