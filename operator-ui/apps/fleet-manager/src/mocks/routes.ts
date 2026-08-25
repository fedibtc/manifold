import type { RouteKey } from '@operator-ui/mock-devtools';

/** Mirrors the router in `@/app/index.tsx`. Ordered longest-first, so
 *  `/seats/abc` resolves to `seat-detail` rather than `seats`. */
const ROUTES: readonly { pattern: RegExp; key: RouteKey }[] = [
  { pattern: /^\/backup\/phrase\/?$/, key: 'backup-phrase' },
  { pattern: /^\/seats\/[^/]+\/?$/, key: 'seat-detail' },
  { pattern: /^\/seats\/?$/, key: 'seats' },
  { pattern: /^\/wallet\/?$/, key: 'wallet' },
  { pattern: /^\/payouts\/?$/, key: 'payouts' },
  { pattern: /^\/offer\/?$/, key: 'offer' },
  { pattern: /^\/backup\/?$/, key: 'backup' },
  { pattern: /^\/authorization\/?$/, key: 'authorization' },
  { pattern: /^\/$/, key: 'overview' }
];

/** The panel's per-page tab filters on this. `null` for an unrecognised path:
 *  the panel then shows its app-wide view rather than claiming a screen the
 *  developer is not looking at (a gate-rendered surface has no pathname). */
export const routeToKey = (pathname: string): RouteKey | null =>
  ROUTES.find((route) => route.pattern.test(pathname))?.key ?? null;
