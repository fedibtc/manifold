import type { SetupConfigView } from '@operator-ui/types';
import { expect, it } from 'vitest';
import { seedDraftFromView } from '../seedDraftFromView';

const baseView: SetupConfigView = {
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
  capacity: { mode: 'available_funds', explicit_cap: null, supported_sources: ['gateway'] },
  funding_policy: {
    fee_reserve: 100_000,
    confirmations: 3,
    stability_pool_min_fee_rate_ppb: 0,
    in_doubt_review_after_secs: 21600
  },
  replenishment: { warning_threshold: 500_000, critical_threshold: 100_000 },
  advertised_endpoint: {
    endpoint_id: 'rpc-signet-01',
    transport: 'iroh',
    address: 'iroh://mock-flip-node',
    discovery_hints: [],
    rpc_protocol_name: 'flip.v1'
  },
  advertisement: { republish_interval: 3600, ready_advertisement_enabled: true },
  provider_display: {
    name: 'Mock FLIP',
    website: 'https://flip.example',
    contact: 'ops@flip.example'
  },
  policy: {
    accepted_attester_policies: [
      { attester_pubkey: '02aa'.padEnd(66, '0'), verification_requirement: 'all_trusted' }
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

it('should map every non-secret field 1:1 and seed admin_credential empty (esplora backend)', () => {
  const draft = seedDraftFromView(baseView);

  expect(draft.network).toBe(baseView.network);
  expect(draft.gateway.gateway_id).toBe(baseView.gateway.gateway_id);
  expect(draft.gateway.gateway_name).toBe(baseView.gateway.gateway_name);
  expect(draft.gateway.admin_url).toBe(baseView.gateway.admin_url);
  expect(draft.gateway.identity_metadata).toEqual(baseView.gateway.identity_metadata);
  expect(draft.secrets.gatewayAdminCredential).toBe('');
  expect(draft.chain_observer.backend).toEqual({
    type: 'esplora',
    url: baseView.chain_observer.backend.url
  });
  expect(draft.relays).toEqual(baseView.relays);
  expect(draft.capacity).toEqual(baseView.capacity);
  expect(draft.funding_policy).toEqual(baseView.funding_policy);
  expect(draft.replenishment).toEqual(baseView.replenishment);
  expect(draft.advertised_endpoint).toEqual(baseView.advertised_endpoint);
  expect(draft.advertisement).toEqual(baseView.advertisement);
  expect(draft.provider_display).toEqual(baseView.provider_display);
  expect(draft.policy).toEqual(baseView.policy);
});

// Secrets are seeded blank meaning "unchanged", and they are not config fields
// at all: the backend the draft carries has no password in it, because a config
// write cannot set, keep or remove one.
it('should seed both secrets blank and leave them out of the config for bitcoind', () => {
  const bitcoindView: SetupConfigView = {
    ...baseView,
    chain_observer: {
      backend: {
        type: 'bitcoind',
        url: 'https://bitcoind.signet.example',
        username: 'rpcuser',
        has_password: true
      }
    }
  };

  const draft = seedDraftFromView(bitcoindView);

  expect(draft.secrets.gatewayAdminCredential).toBe('');
  expect(draft.secrets.chainObserverPassword).toBe('');
  expect(draft.chain_observer.backend).toEqual({
    type: 'bitcoind',
    url: 'https://bitcoind.signet.example',
    username: 'rpcuser'
  });
});
