import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('@/shared/api/adminCall', () => ({ adminCall: vi.fn() }));

import { ReviewStep } from '@/features/setup/components/steps/review-step/ReviewStep';
import { type ConfigDraft, initialDraft } from '@/features/setup/services/draft';
import { adminCall } from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';

const completeDraft: ConfigDraft = {
  ...initialDraft,
  gateway: {
    ...initialDraft.gateway,
    gateway_name: 'gw',
    admin_url: 'https://gw.example.com',
    // Read from the gateway on the gateway step, never typed.
    gateway_id: 'gw-probed'
  },
  chain_observer: { backend: { type: 'esplora', url: 'https://esplora.example.com' } },
  relays: ['wss://relay.example.com'],
  advertised_endpoint: { ...initialDraft.advertised_endpoint, address: 'iroh-addr' },
  capacity: { mode: 'available_funds', supported_sources: ['gateway'] },
  replenishment: { warning_threshold: 100, critical_threshold: 50 },
  policy: {
    accepted_attester_policies: [
      { attester_pubkey: 'npub-abcdef-1234567890', verification_requirement: 'all_trusted' }
    ],
    supported_networks: ['signet']
  },
  // Typed on the gateway step. Stored by name before the config is applied, and
  // never carried inside it.
  secrets: { ...initialDraft.secrets, gatewayAdminCredential: 'secret' }
};

const renderReview = (draft: ConfigDraft) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <ReviewStep draft={draft} onChange={() => {}} errors={{}} onComplete={() => {}} />
      </MemoryRouter>
    </QueryClientProvider>
  );
  return invalidateSpy;
};

const applyButton = () =>
  screen.getByRole('button', { name: 'Apply & go live' }) as HTMLButtonElement;

describe('ReviewStep', () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it('should disable Apply until every earlier step is complete', () => {
    renderReview(initialDraft);
    expect(applyButton().disabled).toBe(true);
  });

  it('should enable Apply once the draft is complete', () => {
    renderReview(completeDraft);
    expect(applyButton().disabled).toBe(false);
  });

  it('should never render the admin credential secret', () => {
    renderReview(completeDraft);
    expect(screen.getByText(/credential set/)).toBeTruthy();
    expect(screen.queryByText(/secret/)).toBeNull();
  });

  it('should show the success screen and invalidate setup-state when apply returns ready', async () => {
    vi.mocked(adminCall).mockResolvedValue({
      status: 'ready',
      validation: { status: 'passed', checks: [] }
    });
    const invalidateSpy = renderReview(completeDraft);

    fireEvent.click(applyButton());

    expect(await screen.findByText("You're live")).toBeTruthy();
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ['setup-state'] });
  });

  it('should show the soft-fail banner and keep the summary when apply returns pending_validation', async () => {
    vi.mocked(adminCall).mockResolvedValue({
      status: 'pending_validation',
      validation: {
        status: 'failed',
        checks: [{ name: 'gateway_reachable', status: 'failed', detail: 'timed out' }]
      }
    });
    renderReview(completeDraft);

    fireEvent.click(applyButton());

    expect(await screen.findByText(/Couldn't apply — 1 checks failed/)).toBeTruthy();
    expect(screen.getByText('Configuration')).toBeTruthy();
    expect(screen.getByText(/timed out/)).toBeTruthy();
  });

  it('should show an error banner when apply throws an AdminApiError', async () => {
    vi.mocked(adminCall).mockRejectedValue(new AdminApiError('invalid_argument', 'bad config'));
    renderReview(completeDraft);

    fireEvent.click(applyButton());

    expect(await screen.findByText('bad config')).toBeTruthy();
    expect(screen.getByText('Configuration')).toBeTruthy();
  });

  it('should render validation checks after re-run validation', async () => {
    vi.mocked(adminCall).mockResolvedValue({
      validation: {
        status: 'passed',
        checks: [{ name: 'relays_reachable', status: 'passed', detail: 'ok' }]
      }
    });
    renderReview(completeDraft);

    fireEvent.click(screen.getByRole('button', { name: 'Re-run validation' }));

    expect(await screen.findByText('relays_reachable')).toBeTruthy();
  });
});
