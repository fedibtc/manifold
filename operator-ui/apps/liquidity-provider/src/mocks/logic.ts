import type {
  SetupConfig,
  SetupConfigView,
  SetupValidationCheck,
  SetupValidationSummary
} from '@operator-ui/types';

interface Rule {
  name: string;
  ok: (config: SetupConfig) => boolean;
  detail: string;
}

const RULES: Rule[] = [
  {
    name: 'network_consistency',
    ok: (c) => c.policy.supported_networks.length >= 1,
    detail: 'no supported networks declared'
  },
  {
    name: 'gateway_reachability',
    ok: (c) => c.gateway.admin_url.length > 0 && c.gateway.gateway_name.length > 0,
    detail: 'gateway admin_url or gateway_name missing'
  },
  {
    name: 'chain_observer_reachability',
    ok: (c) => Boolean(c.chain_observer.backend?.type),
    detail: 'chain observer backend missing'
  },
  {
    name: 'relays_reachable',
    ok: (c) => c.relays.length >= 1,
    detail: 'no relays configured'
  },
  {
    name: 'capacity_sources',
    ok: (c) => c.capacity.supported_sources.length >= 1,
    detail: 'no capacity sources configured'
  },
  {
    name: 'policy_non_empty',
    ok: (c) => c.policy.accepted_attester_policies.length >= 1,
    detail: 'no accepted attester policies'
  },
  {
    // Mirrors the daemon's advertisement_config_check. Without it the mock
    // passed a config the daemon refuses, which is the one thing the mock must
    // never do: it is what let the zero-interval defect reach a release.
    name: 'advertisement_config',
    ok: (c) => c.advertisement.republish_interval > 0,
    detail: 'advertisement republish_interval must be greater than zero'
  }
];

// Which named secrets the mock daemon holds. The config shape carries none.
export interface StoredSecrets {
  gateway_admin_credential: boolean;
  chain_observer_password: boolean;
}

// Deterministic, puppet-grade check list. Not the real daemon's validation.
export const evaluateConfig = (config: SetupConfig | null): SetupValidationSummary => {
  if (!config) {
    return {
      status: 'failed',
      checks: [{ name: 'config_present', status: 'failed', detail: 'no config to validate' }]
    };
  }
  const checks: SetupValidationCheck[] = RULES.map((rule) => {
    const passed = rule.ok(config);
    return {
      name: rule.name,
      status: passed ? 'passed' : 'failed',
      detail: passed ? null : rule.detail
    };
  });
  const allPassed = checks.every((check) => check.status === 'passed');
  return { status: allPassed ? 'passed' : 'failed', checks };
};

// A structurally unusable config (spec S5b hard reject).
export const isMalformed = (config: SetupConfig): boolean =>
  config.relays.length === 0 && config.gateway.admin_url.length === 0;

// Map the write shape to the read shape.
//
// `secrets` is the mock's own record of which named secrets are stored — the
// config carries none, so the view's two presence flags are projections of the
// secret store exactly as they are in the daemon.
export const toView = (config: SetupConfig, secrets: StoredSecrets): SetupConfigView => {
  const backend = config.chain_observer.backend;
  const viewBackend =
    backend.type === 'esplora'
      ? { type: 'esplora' as const, url: backend.url }
      : {
          type: 'bitcoind' as const,
          url: backend.url,
          username: backend.username ?? null,
          has_password: secrets.chain_observer_password
        };
  return {
    network: config.network,
    gateway: {
      // No fallback. The mock used to invent an id when the config carried
      // none, which is why every dashboard test passed while first-time setup
      // was impossible against the real daemon: it refuses a config without one.
      gateway_id: config.gateway.gateway_id ?? '',
      gateway_name: config.gateway.gateway_name,
      admin_url: config.gateway.admin_url,
      has_admin_credential: secrets.gateway_admin_credential,
      identity_metadata: config.gateway.identity_metadata
    },
    chain_observer: { backend: viewBackend },
    relays: config.relays,
    capacity: config.capacity,
    funding_policy: config.funding_policy,
    replenishment: config.replenishment,
    advertised_endpoint: config.advertised_endpoint,
    advertisement: config.advertisement,
    provider_display: config.provider_display ?? null,
    policy: config.policy,
    attestation_summary: {
      holder_authorizations: 0,
      issuer_credentials: 0,
      issuer_authorities: 0,
      valid: 0,
      invalid: 0
    }
  };
};
