import type { VerbLog } from './types';

// One shared instance, so `list()` on an unseen route key is reference-stable
// across renders and `useSyncExternalStore` does not loop.
const EMPTY: readonly string[] = Object.freeze([]);

/**
 * Records which verbs the mock actually served, keyed by the route that was
 * showing at the time. The per-page panel tab reads this instead of a
 * hand-written route→verbs table: a table is prose about what a page queries,
 * nothing checks it, and it lies the day a page gains a call.
 *
 * The route key is stamped at record time rather than the log being cleared on
 * navigation. Clearing from an effect races the page's own fetches — a fetch
 * resolves after every effect has run — and it would also throw away what a
 * page called when the developer walks back to it.
 */
export const createVerbLog = (getRouteKey: () => string): VerbLog => {
  const byRoute = new Map<string, string[]>();
  const listeners = new Set<() => void>();

  const notify = () => {
    for (const listener of listeners) listener();
  };

  return {
    record: (verb) => {
      const routeKey = getRouteKey();
      const seen = byRoute.get(routeKey) ?? EMPTY;
      // Reads poll (`use-authorization-watch`, seat reports), so notifying on a
      // repeat would re-render the panel on a timer. Only a new verb is news.
      if (seen.includes(verb)) return;

      byRoute.set(routeKey, [...seen, verb]);
      notify();
    },

    list: (routeKey) => byRoute.get(routeKey) ?? EMPTY,

    clear: (routeKey) => {
      if (!byRoute.delete(routeKey)) return;
      notify();
    },

    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    }
  };
};
