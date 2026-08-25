// System-health fixtures for the FLIP mock server. Typed against
// @operator-ui/types; keep aligned with the Rust admin surface.

import type { GetHealthResponse, HealthComponent } from '@operator-ui/types';
// The fully-healthy scenario is the real Rust-generated contract fixture (see
// A4 remediation task), so it can't drift from what the daemon's serde impls
// actually produce. Regenerate via `just gen-contract-fixtures`; never
// hand-edit the JSON.
import healthFixture from '@operator-ui/types/fixtures/health.json';

// Shared observed-at stamp so scenarios read consistently. Unix seconds.
const OBSERVED_AT = 1721476800;

// Component list for the back-compat GET /admin/v1/health probe.
export const healthyComponents: HealthComponent[] = [
  { component: 'daemon', status: 'healthy', detail: null, observed_at: OBSERVED_AT },
  { component: 'wallet', status: 'healthy', detail: null, observed_at: OBSERVED_AT }
];

// A fully healthy snapshot for the ready scenarios.
export const healthyHealth: GetHealthResponse = healthFixture as GetHealthResponse;

// A degraded snapshot: chain observer unhealthy, wallet warning.
export const degradedHealth: GetHealthResponse = {
  overall_status: 'unhealthy',
  mode: 'normal',
  observed_at: OBSERVED_AT,
  components: [
    { component: 'daemon', status: 'healthy', detail: null, observed_at: OBSERVED_AT },
    { component: 'wallet', status: 'warning', detail: 'sync lagging', observed_at: OBSERVED_AT },
    {
      component: 'chain_observer',
      status: 'unhealthy',
      detail: 'esplora endpoint unreachable',
      observed_at: OBSERVED_AT
    },
    { component: 'gateway', status: 'healthy', detail: null, observed_at: OBSERVED_AT }
  ]
};
