import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import type { SetupConfig } from '@operator-ui/types';
import { expect, it } from 'vitest';
import {
  buildProviderConfigPatch,
  HARD_FIELDS,
  hasHardFieldChange,
  SOFT_FIELDS
} from '../configDiff';

const baseConfig: SetupConfig = {
  network: 'signet',
  gateway: {
    gateway_id: 'gw-signet-01',
    gateway_name: 'Mock Signet Gateway',
    admin_url: 'https://gateway.signet.example/admin',
    identity_metadata: [['operator', 'mock-flip']]
  },
  chain_observer: { backend: { type: 'esplora', url: 'https://esplora.signet.example' } },
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
  provider_display: null,
  policy: {
    accepted_attester_policies: [
      { attester_pubkey: '02aa'.padEnd(66, '0'), verification_requirement: 'all_trusted' }
    ],
    supported_networks: ['signet']
  }
};

it('should report no hard-field change when the draft equals the baseline', () => {
  const draft: SetupConfig = { ...baseConfig };
  expect(hasHardFieldChange(baseConfig, draft)).toBe(false);
});

it.each(HARD_FIELDS)('should report a hard-field change when %s differs', (field) => {
  const drafts: Record<(typeof HARD_FIELDS)[number], SetupConfig> = {
    network: { ...baseConfig, network: 'bitcoin' },
    gateway: { ...baseConfig, gateway: { ...baseConfig.gateway, gateway_name: 'Other Gateway' } },
    chain_observer: {
      ...baseConfig,
      chain_observer: { backend: { type: 'esplora', url: 'https://other.example' } }
    }
  };
  expect(hasHardFieldChange(baseConfig, drafts[field])).toBe(true);
});

it('should not report a hard-field change for soft-only edits', () => {
  const draft: SetupConfig = {
    ...baseConfig,
    replenishment: { warning_threshold: 1, critical_threshold: 0 }
  };
  expect(hasHardFieldChange(baseConfig, draft)).toBe(false);
});

it('should build an empty patch when nothing changed', () => {
  const draft: SetupConfig = { ...baseConfig };
  expect(buildProviderConfigPatch(baseConfig, draft)).toEqual({});
});

it('should include only the single soft field that changed', () => {
  const draft: SetupConfig = {
    ...baseConfig,
    replenishment: { warning_threshold: 1, critical_threshold: 0 }
  };
  expect(buildProviderConfigPatch(baseConfig, draft)).toEqual({
    replenishment: { warning_threshold: 1, critical_threshold: 0 }
  });
});

it('should encode a provider_display set as a tagged patch', () => {
  const draft: SetupConfig = {
    ...baseConfig,
    provider_display: { name: 'New name', website: null, contact: null }
  };
  expect(buildProviderConfigPatch(baseConfig, draft)).toEqual({
    provider_display: { action: 'set', value: { name: 'New name', website: null, contact: null } }
  });
});

it('should encode a provider_display clear as a tagged patch', () => {
  const withDisplay: SetupConfig = {
    ...baseConfig,
    provider_display: { name: 'Existing', website: null, contact: null }
  };
  const draft: SetupConfig = { ...withDisplay, provider_display: null };
  expect(buildProviderConfigPatch(withDisplay, draft)).toEqual({
    provider_display: { action: 'clear' }
  });
});

it('should omit hard fields from the patch even when they changed', () => {
  const draft: SetupConfig = { ...baseConfig, network: 'bitcoin' };
  expect(buildProviderConfigPatch(baseConfig, draft)).toEqual({});
});

const settingsRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');

const collectFiles = (dir: string): string[] =>
  readdirSync(dir).flatMap((entry) => {
    const fullPath = path.join(dir, entry);
    return statSync(fullPath).isDirectory() ? collectFiles(fullPath) : [fullPath];
  });

it('should declare HARD_FIELDS and SOFT_FIELDS in exactly one file', () => {
  const files = collectFiles(settingsRoot).filter(
    (file) =>
      (file.endsWith('.ts') || file.endsWith('.tsx')) &&
      !file.includes(`${path.sep}__tests__${path.sep}`)
  );
  const declaringHardFields = files.filter((file) =>
    /export const HARD_FIELDS/.test(readFileSync(file, 'utf8'))
  );
  const declaringSoftFields = files.filter((file) =>
    /export const SOFT_FIELDS/.test(readFileSync(file, 'utf8'))
  );
  const configDiffPath = path.resolve(settingsRoot, 'utils', 'configDiff.ts');
  expect(declaringHardFields).toEqual([configDiffPath]);
  expect(declaringSoftFields).toEqual([configDiffPath]);
  // Every editable SetupConfig field is classified as either hard or soft
  // (3 hard + 8 soft = 11 top-level fields).
  expect(SOFT_FIELDS.length + HARD_FIELDS.length).toBe(11);
});
