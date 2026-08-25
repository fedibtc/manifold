import type { AdminRequest } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import * as adminCallModule from '@/shared/api/adminCall';
import { SetupWizard } from '../SetupWizard';

const PHRASE = 'abandon abandon abandon abandon abandon abandon abandon abandon about';

const onboarding = (authorized: boolean) => ({
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: authorized
    ? {
        state: 'authorization_observed',
        authorizations: [],
        holders: [MOCK_HOLDER_PUBKEY],
        checked_at: 1_760_000_000
      }
    : { state: 'not_observed', checked_at: 1_760_000_000 }
});

// One stub for every verb the wizard reaches, so a test only has to say whether
// the relay has seen an authorization yet.
const stubDaemon = (authorized: boolean) =>
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request: AdminRequest) => {
    if (request === 'Onboarding') return Promise.resolve(onboarding(authorized));
    if (request === 'ShowMnemonic') return Promise.resolve({ mnemonic: PHRASE });
    if (typeof request === 'object' && 'OnboardAsNew' in request) {
      return Promise.resolve({ onboarded: 'new', seats: 0 });
    }
    if (typeof request === 'object' && 'OnboardFromBackup' in request) {
      return Promise.resolve({ onboarded: 'restored', seats: 2, formed: 1 });
    }
    return Promise.resolve({ plans: [] });
  });

const renderWizard = (onComplete = vi.fn()) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <SetupWizard onComplete={onComplete} initialStep="doors" />
    </QueryClientProvider>
  );
  return { onComplete };
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SetupWizard', () => {
  it('should open on the two doors', () => {
    stubDaemon(false);
    renderWizard();

    expect(screen.getByRole('heading', { name: 'Set up your fleet manager' })).toBeTruthy();
  });

  it('should walk a new fleet from the doors to the price step', async () => {
    stubDaemon(true);
    renderWizard();

    fireEvent.click(screen.getByRole('button', { name: 'Start a new fleet' }));
    await screen.findByRole('heading', { name: 'Record your recovery phrase' });

    fireEvent.click(screen.getByRole('button', { name: 'Reveal phrase' }));
    await screen.findByText(PHRASE);
    fireEvent.click(screen.getByRole('button', { name: "I've written it down — continue" }));

    await screen.findByRole('heading', { name: 'Get this fleet authorized' });
    await screen.findByRole('heading', { name: 'Set your price' }, { timeout: 5000 });
  });

  it('should complete setup once the price is stored', async () => {
    stubDaemon(true);
    const { onComplete } = renderWizard();

    fireEvent.click(screen.getByRole('button', { name: 'Start a new fleet' }));
    await screen.findByRole('heading', { name: 'Record your recovery phrase' });
    fireEvent.click(screen.getByRole('button', { name: 'Reveal phrase' }));
    await screen.findByText(PHRASE);
    fireEvent.click(screen.getByRole('button', { name: "I've written it down — continue" }));
    // The authorization step continues on its own once the relay reports an
    // observed authorization, so there is no click here — only the wait.
    await screen.findByRole('heading', { name: 'Get this fleet authorized' });
    await screen.findByRole('heading', { name: 'Set your price' }, { timeout: 5000 });

    fireEvent.click(screen.getByRole('button', { name: 'Finish setup' }));

    await waitFor(() => expect(onComplete).toHaveBeenCalled());
  });

  it('should reach the restore fork from the doors and nowhere else', async () => {
    stubDaemon(false);
    renderWizard();

    fireEvent.click(screen.getByRole('button', { name: 'Recover from a phrase' }));

    await screen.findByRole('heading', { name: 'Recover from your phrase' });
  });

  it('should always send a recovered fleet to the authorization step', async () => {
    // The daemon reports waiting_for_authorization right after a restore whether or
    // not an authorization exists (F3), so the wizard must not branch on it.
    stubDaemon(true);
    renderWizard();

    fireEvent.click(screen.getByRole('button', { name: 'Recover from a phrase' }));
    await screen.findByRole('heading', { name: 'Recover from your phrase' });

    fireEvent.change(screen.getByLabelText('Recovery phrase'), { target: { value: PHRASE } });
    fireEvent.click(screen.getByLabelText(/permanently offline/i));
    fireEvent.click(screen.getByRole('button', { name: 'Recover this fleet' }));

    await screen.findByRole('heading', { name: 'Recovery finished' });
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

    await screen.findByRole('heading', { name: 'Get this fleet authorized' });
  });

  it('should not use cached authorization data from an earlier identity', async () => {
    // A cached authorized Onboarding response belongs to whatever identity the host
    // carried before. Only the response fetched after the restore may decide the
    // next step.
    stubDaemon(false);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(['onboarding'], {
      fman_name: 'stale-fleet',
      service_pubkey: '02abc',
      service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
      nostr: {
        state: 'authorization_observed',
        authorizations: [],
        holders: [MOCK_HOLDER_PUBKEY],
        checked_at: 1_760_000_000
      }
    });

    render(
      <QueryClientProvider client={client}>
        <SetupWizard onComplete={vi.fn()} initialStep="doors" />
      </QueryClientProvider>
    );

    fireEvent.click(screen.getByRole('button', { name: 'Recover from a phrase' }));
    await screen.findByRole('heading', { name: 'Recover from your phrase' });

    fireEvent.change(screen.getByLabelText('Recovery phrase'), { target: { value: PHRASE } });
    fireEvent.click(screen.getByLabelText(/permanently offline/i));
    fireEvent.click(screen.getByRole('button', { name: 'Recover this fleet' }));

    await screen.findByRole('heading', { name: 'Recovery finished' });
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

    await screen.findByRole('heading', { name: 'Get this fleet authorized' });
    await waitFor(() => expect(screen.getByText(/no authorization for this fleet/i)).toBeTruthy());
  });
});
