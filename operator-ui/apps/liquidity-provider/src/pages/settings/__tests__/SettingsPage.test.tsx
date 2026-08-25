import type { GetProviderConfigResponse, SetupConfigView } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as providerConfigHooks from '@/features/settings/api/hooks/use-provider-config/useProviderConfig';
import * as configSaveHooks from '@/features/settings/hooks/use-config-save/useConfigSave';
import { seedDraftFromView } from '@/features/settings/utils/seedDraftFromView';
import * as adminCallModule from '@/shared/api/adminCall';
import { AuthError } from '@/shared/api/errors';
import { SettingsPage } from '../SettingsPage';

const readyView: SetupConfigView = {
  network: 'signet',
  gateway: {
    gateway_id: 'gw-signet-01',
    gateway_name: 'Mock Signet Gateway',
    admin_url: 'https://gateway.signet.example/admin',
    has_admin_credential: true,
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
  },
  attestation_summary: {
    holder_authorizations: 0,
    issuer_credentials: 0,
    issuer_authorities: 0,
    valid: 0,
    invalid: 0
  }
};

const readyResponse: GetProviderConfigResponse = { config: readyView };

type ProviderConfigResult = ReturnType<typeof providerConfigHooks.useProviderConfig>;
type ConfigSaveResult = ReturnType<typeof configSaveHooks.useConfigSave>;

const asProviderConfigResult = (partial: Partial<ProviderConfigResult>): ProviderConfigResult =>
  partial as unknown as ProviderConfigResult;

const asConfigSaveResult = (partial: Partial<ConfigSaveResult>): ConfigSaveResult =>
  partial as unknown as ConfigSaveResult;

const renderPage = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const { rerender } = render(
    <QueryClientProvider client={client}>
      <SettingsPage />
    </QueryClientProvider>
  );
  return { client, rerender };
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SettingsPage — loading, error and layout', () => {
  it('should render a loading placeholder while settings are fetching', () => {
    vi.spyOn(providerConfigHooks, 'useProviderConfig').mockReturnValue(
      asProviderConfigResult({ isLoading: true })
    );
    vi.spyOn(configSaveHooks, 'useConfigSave').mockReturnValue(
      asConfigSaveResult({
        save: vi.fn(),
        phase: 'idle',
        validation: null,
        isSaving: false
      })
    );

    renderPage();

    expect(screen.getByText('Loading…')).toBeTruthy();
  });

  it('should render the failure and a retry control when nothing has answered', () => {
    vi.spyOn(providerConfigHooks, 'useProviderConfig').mockReturnValue(
      asProviderConfigResult({ isError: true, error: new AuthError() })
    );
    vi.spyOn(configSaveHooks, 'useConfigSave').mockReturnValue(
      asConfigSaveResult({
        save: vi.fn(),
        phase: 'idle',
        validation: null,
        isSaving: false
      })
    );

    renderPage();

    expect(screen.getByText('unauthorized')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy();
  });

  // A refresh that fails over a config the app still holds. React Query keeps
  // `data` and flips status to error, which the screen used to read as having
  // nothing: it blanked, and the operator lost the settings they had opened it
  // to read. Two renders, because that sequence is the only way the state
  // arises — a load that succeeded, then a refetch that did not.
  it('should keep the form under a staleness marker when a refresh fails', () => {
    const providerConfig = vi
      .spyOn(providerConfigHooks, 'useProviderConfig')
      .mockReturnValue(asProviderConfigResult({ isSuccess: true, data: readyResponse }));
    vi.spyOn(configSaveHooks, 'useConfigSave').mockReturnValue(
      asConfigSaveResult({
        save: vi.fn(),
        phase: 'idle',
        validation: null,
        isSaving: false
      })
    );

    const { rerender, client } = renderPage();
    expect(screen.queryByText('Showing last-known data')).toBeNull();

    providerConfig.mockReturnValue(
      asProviderConfigResult({
        isError: true,
        error: new AuthError(),
        data: readyResponse,
        dataUpdatedAt: 1_700_000_000_000
      })
    );
    rerender(
      <QueryClientProvider client={client}>
        <SettingsPage />
      </QueryClientProvider>
    );

    expect(screen.getByText('Showing last-known data')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Save changes' })).toBeTruthy();
  });

  it('should render every reused section without any restart warning', () => {
    vi.spyOn(providerConfigHooks, 'useProviderConfig').mockReturnValue(
      asProviderConfigResult({ isSuccess: true, data: readyResponse })
    );
    vi.spyOn(configSaveHooks, 'useConfigSave').mockReturnValue(
      asConfigSaveResult({
        save: vi.fn(),
        phase: 'idle',
        validation: null,
        isSaving: false
      })
    );

    renderPage();

    expect(screen.getByText('Capacity mode')).toBeTruthy();
    expect(screen.getByText('Relays')).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Network' })).toBeTruthy();
    expect(screen.getByText('Gateway name')).toBeTruthy();
    expect(screen.getByText('Backend')).toBeTruthy();
    expect(screen.queryByText('Restart required')).toBeNull();
    expect(screen.getByRole('button', { name: 'Save changes' })).toBeTruthy();
  });

  it('should render failed validation checks inline', () => {
    vi.spyOn(providerConfigHooks, 'useProviderConfig').mockReturnValue(
      asProviderConfigResult({ isSuccess: true, data: readyResponse })
    );
    vi.spyOn(configSaveHooks, 'useConfigSave').mockReturnValue(
      asConfigSaveResult({
        save: vi.fn(),
        phase: 'validation_failed',
        validation: {
          status: 'failed',
          checks: [
            {
              name: 'gateway_reachability',
              status: 'failed',
              detail: 'gateway admin_url did not respond'
            }
          ]
        },
        isSaving: false
      })
    );

    renderPage();

    expect(screen.getByText("Couldn't save — 1 checks failed")).toBeTruthy();
    expect(screen.getByText(/gateway_reachability/)).toBeTruthy();
  });

  it('should not ask for a restart after a successful hard-path save', () => {
    vi.spyOn(providerConfigHooks, 'useProviderConfig').mockReturnValue(
      asProviderConfigResult({ isSuccess: true, data: readyResponse })
    );
    vi.spyOn(configSaveHooks, 'useConfigSave').mockReturnValue(
      asConfigSaveResult({
        save: vi.fn(),
        phase: 'success',
        validation: null,
        isSaving: false
      })
    );

    renderPage();

    expect(screen.queryByText(/[Rr]estart/)).toBeNull();
  });

  // The screen used to demand the gateway credential be retyped before any
  // hard-field save, because that save carried the credential and a blank one
  // would have overwritten the stored secret. Secrets are written by name now,
  // so a blank box is unchanged and the demand is gone.
  it('should explain that a blank secret keeps the stored one, and never demand a retype', () => {
    vi.spyOn(providerConfigHooks, 'useProviderConfig').mockReturnValue(
      asProviderConfigResult({ isSuccess: true, data: readyResponse })
    );
    vi.spyOn(configSaveHooks, 'useConfigSave').mockReturnValue(
      asConfigSaveResult({
        save: vi.fn(),
        phase: 'idle',
        validation: null,
        isSaving: false
      })
    );

    renderPage();

    expect(screen.getByText('Secrets are kept separately')).toBeTruthy();
    expect(screen.queryByText(/re-enter/i)).toBeNull();
    expect(screen.queryByText('Gateway admin credential required')).toBeNull();
  });
});

describe('SettingsPage — save flow', () => {
  it('should call configSave.save with the seeded baseline and draft and invalidate advertisement state on success', async () => {
    vi.spyOn(providerConfigHooks, 'useProviderConfig').mockReturnValue(
      asProviderConfigResult({ isSuccess: true, data: readyResponse })
    );
    const expectedDraft = seedDraftFromView(readyView);
    const saveSpy = vi.fn().mockResolvedValue({ status: 'success', validation: null });
    vi.spyOn(configSaveHooks, 'useConfigSave').mockReturnValue(
      asConfigSaveResult({
        save: saveSpy,
        phase: 'idle',
        validation: null,
        isSaving: false
      })
    );

    const { client } = renderPage();
    const invalidateSpy = vi.spyOn(client, 'invalidateQueries');

    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));

    await waitFor(() => expect(saveSpy).toHaveBeenCalledWith(expectedDraft, expectedDraft));
    await waitFor(() =>
      expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ['advertisement-state'] })
    );
  });

  it('should send only an empty patch to update_provider_config when nothing was edited', async () => {
    const adminCallSpy = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockImplementation(async (method: unknown) => {
        if (method === 'get_provider_config') return readyResponse;
        if (method === 'update_provider_config') {
          return { config: readyView, validation: { status: 'passed', checks: [] } };
        }
        throw new Error(`unexpected method ${String(method)}`);
      });

    renderPage();

    await waitFor(() => expect(screen.getByRole('button', { name: 'Save changes' })).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));

    await waitFor(() =>
      expect(adminCallSpy).toHaveBeenCalledWith('update_provider_config', { patch: {} })
    );
    expect(adminCallSpy).not.toHaveBeenCalledWith('validate_setup', expect.anything());
    expect(adminCallSpy).not.toHaveBeenCalledWith('apply_setup_config', expect.anything());
  });
});
