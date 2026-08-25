import type { SetupConfig, SetupValidationSummary } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { SETUP_STATE_KEY } from '@/shared/api/hooks/use-setup-state/useSetupState';
import { type ConfigDraft, emptyDraftSecrets } from '@/shared/config/draft';
import { useConfigSave } from '../useConfigSave';

const baseConfig: SetupConfig = {
  network: 'signet',
  gateway: {
    gateway_id: 'gw-signet-01',
    gateway_name: 'Mock Signet Gateway',
    admin_url: 'https://gateway.signet.example/admin',
    identity_metadata: []
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

// The draft the screen holds: the config, plus the secrets the operator has
// typed. Blank means unchanged, and nothing is sent for it.
const baseDraft: ConfigDraft = { ...baseConfig, secrets: { ...emptyDraftSecrets } };

const passedValidation: SetupValidationSummary = {
  status: 'passed',
  checks: [{ name: 'gateway_reachability', status: 'passed', detail: null }]
};

const failedValidation: SetupValidationSummary = {
  status: 'failed',
  checks: [
    { name: 'gateway_reachability', status: 'failed', detail: 'gateway admin_url did not respond' }
  ]
};

const makeClient = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

const wrap = (client: QueryClient) => {
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return Wrapper;
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should only call update_provider_config for a soft-only change', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ config: baseConfig, validation: passedValidation });
  const client = makeClient();

  const { result } = renderHook(() => useConfigSave(), { wrapper: wrap(client) });
  const draft: ConfigDraft = {
    ...baseDraft,
    replenishment: { warning_threshold: 1, critical_threshold: 0 }
  };

  let outcome: Awaited<ReturnType<typeof result.current.save>> | undefined;
  await act(async () => {
    outcome = await result.current.save(baseConfig, draft);
  });

  expect(adminCallSpy).toHaveBeenCalledTimes(1);
  expect(adminCallSpy).toHaveBeenCalledWith('update_provider_config', {
    patch: { replenishment: { warning_threshold: 1, critical_threshold: 0 } }
  });
  expect(outcome).toEqual({ status: 'success', validation: null });
});

it('should validate, apply and invalidate setup state for a hard-field change', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    // The secret write comes first, then validate, then apply.
    .mockResolvedValueOnce({ secret: 'gateway_admin_credential', present: true })
    .mockResolvedValueOnce({ validation: passedValidation })
    .mockResolvedValueOnce({ status: 'ready', validation: passedValidation });
  const client = makeClient();
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');

  const { result } = renderHook(() => useConfigSave(), { wrapper: wrap(client) });
  const draft: ConfigDraft = {
    ...baseDraft,
    network: 'bitcoin',
    secrets: { ...emptyDraftSecrets, gatewayAdminCredential: 'rotated-secret' }
  };

  let outcome: Awaited<ReturnType<typeof result.current.save>> | undefined;
  await act(async () => {
    outcome = await result.current.save(baseConfig, draft);
  });

  // The secret goes first and on its own: the daemon validates a candidate
  // against the stored secrets, so one just typed has to be stored to be
  // tested. The config that follows carries no secret at all.
  expect(adminCallSpy).toHaveBeenNthCalledWith(1, 'set_config_secret', {
    secret: 'gateway_admin_credential',
    update: { action: 'set', value: 'rotated-secret' }
  });
  expect(adminCallSpy).toHaveBeenCalledWith('validate_setup', {
    candidate_config: { ...baseConfig, network: 'bitcoin' }
  });
  expect(adminCallSpy).toHaveBeenCalledWith('apply_setup_config', {
    config: { ...baseConfig, network: 'bitcoin' }
  });
  expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: SETUP_STATE_KEY });
  expect(outcome).toEqual({ status: 'success', validation: passedValidation });
  await waitFor(() => expect(result.current.phase).toBe('success'));
});

it('should return the validation summary without applying when validation fails', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValueOnce({ validation: failedValidation });
  const client = makeClient();

  const { result } = renderHook(() => useConfigSave(), { wrapper: wrap(client) });
  const draft: ConfigDraft = { ...baseDraft, network: 'bitcoin' };

  let outcome: Awaited<ReturnType<typeof result.current.save>> | undefined;
  await act(async () => {
    outcome = await result.current.save(baseConfig, draft);
  });

  expect(adminCallSpy).toHaveBeenCalledTimes(1);
  expect(adminCallSpy).toHaveBeenCalledWith('validate_setup', {
    candidate_config: { ...baseConfig, network: 'bitcoin' }
  });
  expect(outcome).toEqual({
    status: 'validation_failed',
    validation: failedValidation
  });
  await waitFor(() => expect(result.current.phase).toBe('validation_failed'));
});

// The workaround this replaced. The gateway credential used to travel inside
// the config write, so a hard-field save either carried a real credential or
// overwrote the stored one with a blank — and the screen refused to save until
// the operator retyped a credential their edit had nothing to do with. A config
// write cannot touch a secret now, so a blank box is simply unchanged.
it('should save a hard-field change with a blank credential and send no secret', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValueOnce({ validation: passedValidation })
    .mockResolvedValueOnce({ status: 'ready', validation: passedValidation });
  const client = makeClient();

  const { result } = renderHook(() => useConfigSave(), { wrapper: wrap(client) });
  const draft: ConfigDraft = { ...baseDraft, network: 'bitcoin' };

  let outcome: Awaited<ReturnType<typeof result.current.save>> | undefined;
  await act(async () => {
    outcome = await result.current.save(baseConfig, draft);
  });

  expect(adminCallSpy).not.toHaveBeenCalledWith('set_config_secret', expect.anything());
  expect(adminCallSpy).toHaveBeenCalledWith('apply_setup_config', {
    config: { ...baseConfig, network: 'bitcoin' }
  });
  expect(outcome).toEqual({ status: 'success', validation: passedValidation });
});

// The defect the whole change exists to remove. The bitcoind password used to
// travel inside the config write, where the read shape returns has_password and
// never the value — so the screen sent it back blank and the daemon read blank
// as delete. An operator changing a gateway display name lost their chain
// connection, silently.
it('should send no secret at all when the operator typed neither', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ config: baseConfig, validation: passedValidation });
  const client = makeClient();

  const { result } = renderHook(() => useConfigSave(), { wrapper: wrap(client) });
  const draft: ConfigDraft = {
    ...baseDraft,
    replenishment: { warning_threshold: 9, critical_threshold: 1 }
  };

  await act(async () => {
    await result.current.save(baseConfig, draft);
  });

  expect(adminCallSpy).toHaveBeenCalledTimes(1);
  expect(adminCallSpy).not.toHaveBeenCalledWith('set_config_secret', expect.anything());
});

// Each secret is written on its own, by name. Neither can be set, kept or
// removed as a side effect of stating a configuration.
it('should write each typed secret by name before the config', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ config: baseConfig, validation: passedValidation });
  const client = makeClient();

  const { result } = renderHook(() => useConfigSave(), { wrapper: wrap(client) });
  const draft: ConfigDraft = {
    ...baseDraft,
    secrets: { gatewayAdminCredential: 'gw-secret', chainObserverPassword: 'btc-secret' }
  };

  await act(async () => {
    await result.current.save(baseConfig, draft);
  });

  expect(adminCallSpy).toHaveBeenNthCalledWith(1, 'set_config_secret', {
    secret: 'gateway_admin_credential',
    update: { action: 'set', value: 'gw-secret' }
  });
  expect(adminCallSpy).toHaveBeenNthCalledWith(2, 'set_config_secret', {
    secret: 'chain_observer_password',
    update: { action: 'set', value: 'btc-secret' }
  });
});
