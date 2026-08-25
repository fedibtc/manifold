// Shared JSON fixtures + scenario generators for the app mock servers.
// Imported by apps/*/mock-server. Keep fixtures aligned with @operator-ui/types.

import type { SetupConfigView, SetupValidationSummary } from '@operator-ui/types';

export * from './advertisement';
export { allocationDetails, allocationSummaries } from './allocations';
export { seededAttestations } from './attestations';
export { backupManifest } from './backup';
export * from './funds';
export * from './health';

// Missing-fields arrays used by the setup scenarios.
export const freshMissingFields: string[] = [
  'gateway',
  'chain_observer',
  'relays',
  'capacity',
  'policy'
];

export const emptyMissingFields: string[] = [];

// A complete, realistic signet SetupConfigView (read shape — no secrets).
export const readySetupConfigView: SetupConfigView = {
  network: 'signet',
  gateway: {
    gateway_id: 'gw-signet-01',
    gateway_name: 'Mock Signet Gateway',
    admin_url: 'https://gateway.signet.example/admin',
    has_admin_credential: true,
    identity_metadata: [['operator', 'mock-flip']]
  },
  chain_observer: {
    backend: { type: 'esplora', url: 'https://esplora.signet.example' }
  },
  relays: ['wss://relay.signet.example'],
  capacity: {
    mode: 'available_funds',
    explicit_cap: null,
    supported_sources: ['gateway']
  },
  funding_policy: {
    fee_reserve: 5_000,
    confirmations: 1,
    stability_pool_min_fee_rate_ppb: 0,
    in_doubt_review_after_secs: 21600
  },
  replenishment: {
    warning_threshold: 500_000,
    critical_threshold: 100_000
  },
  advertised_endpoint: {
    endpoint_id: 'rpc-signet-01',
    transport: 'iroh',
    address: 'iroh://mock-flip-node',
    discovery_hints: [],
    rpc_protocol_name: 'flip.v1'
  },
  advertisement: {
    republish_interval: 3600,
    ready_advertisement_enabled: true
  },
  provider_display: {
    name: 'Mock FLIP',
    website: 'https://flip.example',
    contact: 'ops@flip.example'
  },
  policy: {
    accepted_attester_policies: [
      {
        attester_pubkey: '02aa'.padEnd(66, '0'),
        verification_requirement: 'all_trusted'
      }
    ],
    supported_networks: ['signet']
  },
  attestation_summary: {
    holder_authorizations: 0,
    issuer_credentials: 0,
    issuer_authorities: 0,
    valid: 0,
    invalid: 0
  }
};

const passedCheckNames = [
  'network_consistency',
  'gateway_reachability',
  'chain_observer_reachability',
  'relays_reachable',
  'policy_non_empty',
  'database_storage'
];

export const passedValidation: SetupValidationSummary = {
  status: 'passed',
  checks: passedCheckNames.map((name) => ({ name, status: 'passed', detail: null }))
};

export const failedValidation: SetupValidationSummary = {
  status: 'failed',
  checks: passedCheckNames.map((name) => {
    if (name === 'gateway_reachability') {
      return { name, status: 'failed', detail: 'gateway admin_url did not respond' };
    }
    if (name === 'relays_reachable') {
      return { name, status: 'failed', detail: 'no relays configured' };
    }
    return { name, status: 'passed', detail: null };
  })
};
