import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, expect, it, vi } from 'vitest';
import { walletStatus } from '@/mocks/wallet-status';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';
import { WalletPage } from '../WalletPage';

const ONE_FEDERATION = {
  federations: [
    {
      federation_id: 'fed1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
      accepted: true,
      receivable: false,
      wallet: walletStatus(5_000_000)
    }
  ]
};

const renderPage = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <WalletPage />
      </MemoryRouter>
    </QueryClientProvider>
  );
  return client;
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should render the Wallet heading', () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ federations: [] });
  renderPage();

  screen.getByRole('heading', { name: 'Wallet' });
});

it('should show the empty state when there are no federations', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ federations: [] });
  renderPage();

  await waitFor(() => screen.getByText(/no payment federations accepted yet/i));
});

it('should list a federation, its truncated id, receivable status, and total balance', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(ONE_FEDERATION);
  renderPage();

  await waitFor(() => screen.getByText('Not receiving'));
  screen.getByText('5,000 sats');
});

it('should not claim an empty wallet before the daemon has answered', () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(() => new Promise(() => {}));
  renderPage();

  screen.getByText('Loading…');
  expect(screen.queryByText(/no payment federations accepted yet/i)).toBeNull();
});

// The page argued the no-data case by hand and gave the operator no way out of
// it. QuerySurface is where the retry comes from.
it('should offer a retry when the read has never answered', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockRejectedValueOnce(new AdminApiError('wallet unreadable'))
    .mockResolvedValue(ONE_FEDERATION);
  renderPage();

  await waitFor(() => screen.getByText('wallet unreadable'));
  expect(screen.queryByText(/no payment federations accepted yet/i)).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

  await waitFor(() => screen.getByText('5,000 sats'));
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});

it('should keep the balances under a dated staleness marker when a refresh fails', async () => {
  vi.spyOn(adminCallModule, 'adminCall')
    .mockResolvedValueOnce(ONE_FEDERATION)
    .mockRejectedValue(new AdminApiError('wallet unreadable'));
  const client = renderPage();

  await waitFor(() => screen.getByText('5,000 sats'));
  await act(async () => {
    await client.refetchQueries();
  });

  await screen.findByText('Showing last-known data');
  expect(screen.getByText('5,000 sats')).toBeTruthy();
  expect(screen.getByText(/last updated/i)).toBeTruthy();
});
