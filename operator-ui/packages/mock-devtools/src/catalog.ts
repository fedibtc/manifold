import type { RouteKey, ScenarioCatalogEntry } from './types';

/** The scenarios that change what the given route renders, per their `affects`
 *  notes. Drives both the per-page tab and the choice of which tab to open on. */
export const scenariosFor = (
  catalog: readonly ScenarioCatalogEntry[],
  routeKey: RouteKey
): readonly ScenarioCatalogEntry[] => catalog.filter((entry) => entry.affects.includes(routeKey));

/** `seat-detail` → `Seat detail`, for the tab label. */
export const formatRouteKey = (routeKey: RouteKey): string => {
  const spaced = routeKey.replace(/-/g, ' ');
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
};
