import type { GetHealthResponse } from '@operator-ui/types';
import type { MockState } from '@/mocks/state';

// Mirrors the backend's restore_health_response: when the daemon booted in
// restore mode it reports `mode: 'restore'` and the `daemon` component's status
// flips to 'warning'. Shared by the get_health verb and the unauthenticated
// GET /health liveness probe (mock-server routes/health.ts) — the boot path
// calls the latter, so both must agree on the shape.
const DAEMON_COMPONENT = 'daemon';

export const withRestoreMarker = (
  health: GetHealthResponse,
  bootMode: MockState['bootMode']
): GetHealthResponse => {
  if (bootMode !== 'restore') return health;
  return {
    ...health,
    mode: 'restore',
    components: health.components.map((component) =>
      component.component === DAEMON_COMPONENT ? { ...component, status: 'warning' } : component
    )
  };
};
