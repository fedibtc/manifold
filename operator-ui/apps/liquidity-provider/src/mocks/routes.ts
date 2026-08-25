import type { RouteKey } from '@operator-ui/mock-devtools';

/** Mirrors the router in `@/app/App.tsx`. `restore-console` and `auth` are
 *  rendered by gates rather than routed, so they have no pattern here; the
 *  panel opens on its Global tab when nothing claims the current route. */
const ROUTES: readonly { pattern: RegExp; key: RouteKey }[] = [
  { pattern: /^\/setup\/?$/, key: 'setup' },
  { pattern: /^\/funds\/?$/, key: 'funds' },
  { pattern: /^\/allocations\/?$/, key: 'allocations' },
  { pattern: /^\/advertisement\/?$/, key: 'advertisement' },
  { pattern: /^\/settings\/?$/, key: 'settings' },
  { pattern: /^\/$/, key: 'overview' }
];

/** `null` for an unrecognised path — including the gate-rendered surfaces
 *  above — so the panel shows its app-wide view rather than claiming a screen
 *  the developer is not looking at. */
export const routeToKey = (pathname: string): RouteKey | null =>
  ROUTES.find((route) => route.pattern.test(pathname))?.key ?? null;
