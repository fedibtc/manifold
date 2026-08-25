import { useCallback, useSyncExternalStore } from 'react';
import type { VerbLog } from '../types';

/** The verbs served while the given route was showing, kept live as more
 *  arrive. `list` returns a stable reference until the set actually grows,
 *  which is what lets `useSyncExternalStore` settle. */
export const useVerbLog = (log: VerbLog, routeKey: string): readonly string[] => {
  const subscribe = useCallback((listener: () => void) => log.subscribe(listener), [log]);

  return useSyncExternalStore(subscribe, () => log.list(routeKey));
};
