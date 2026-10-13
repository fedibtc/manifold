import { createVerbLog } from '@operator-ui/mock-devtools';
import { routeToKey } from '@/mocks/routes';

/** Records what the mock actually served, stamped with the route showing at the
 *  time, so the panel's per-page tab needs no hand-written route→verbs map.
 *  Verbs served while no route is recognised land under a sentinel no page tab
 *  ever shows — better unlisted than attributed to a screen not on display. */
export const verbLog = createVerbLog(() => routeToKey(window.location.pathname) ?? 'unrouted');
