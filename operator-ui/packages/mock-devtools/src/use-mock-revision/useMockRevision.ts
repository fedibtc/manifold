import { useCallback, useSyncExternalStore } from 'react';
import type { ScenarioStore } from '../types';

/** Subscribes a component to every world change, not just a scenario switch.
 *  `useScenario`'s snapshot is the scenario name, which a control knob leaves
 *  untouched — anything rendering live world values needs this instead. */
export const useMockRevision = <W>(store: ScenarioStore<W>): number => {
  const subscribe = useCallback((listener: () => void) => store.subscribe(listener), [store]);

  return useSyncExternalStore(subscribe, () => store.getRevision());
};
