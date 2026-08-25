// System-health surface. Mirrors the health method of
// crates/service-liquidity-manager/src/admin.rs (`get_health`) and the component
// health struct in crates/services. Wire values are serde snake_case strings.
// The unauthenticated liveness probe (GET /health) returns this same shape with
// every `detail` withheld — see `redacted_for_public` in admin.rs. Read `mode`,
// never a detail string, for anything the boot path decides.

import type { Timestamp } from './admin';

// What the reporting process can serve right now. Mirrors admin.rs `HealthMode`.
// Reported on the unauthenticated probe because during a live restore swap no
// authenticated route answers to carry it.
export type HealthMode = 'normal' | 'restore' | 'reloading' | 'no_runtime';

// Overall / per-component health level. Mirrors admin.rs `HealthStatus`.
export type HealthStatus = 'healthy' | 'warning' | 'unhealthy' | 'unknown';

// Identifies the monitored subsystem. Mirrors admin.rs `HealthComponent`.
export type HealthComponentName =
  | 'daemon'
  | 'package'
  | 'web_client'
  | 'wallet'
  | 'database'
  | 'relays'
  | 'admin_api'
  | 'public_liquidity_api'
  | 'gateway'
  | 'chain_observer'
  | 'background_workers';

// A single monitored subsystem (daemon, wallet, gateway, chain observer, …).
export interface HealthComponent {
  component: HealthComponentName;
  status: HealthStatus;
  detail?: string | null;
  observed_at: Timestamp;
}

export type GetHealthRequest = null; // unit struct → null

export interface GetHealthResponse {
  overall_status: HealthStatus;
  mode: HealthMode;
  components: HealthComponent[];
  observed_at: Timestamp;
}
