import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { walletStatus } from '@/mocks/wallet-status';
import * as adminCallModule from '@/shared/api/adminCall';
import { PayoutsPage } from '../PayoutsPage';

const FEDERATION_ID = 'fed1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';

const seatRow = {
  seat_id: 'seat-earning-01',
  decommissioned: false,
  fi_id: 'fi_01',
  plan: { InfiniteBestEffort: { price_msats: 1 } },
  created_at_ms: 0,
  payment_claim: { state: 'success', at_ms: 0 },
  completion_callback: { state: 'not_configured' },
  guardian_fee: { remittance_account: '{}' }
};

const world = (destination: string | null) => (request: unknown) => {
  if (request === 'PayoutDestination') return Promise.resolve({ destination });
  if (request === 'ListPaymentFederations') {
    return Promise.resolve({
      federations: [
        {
          federation_id: FEDERATION_ID,
          accepted: true,
          receivable: true,
          wallet: walletStatus(250_000_000)
        }
      ]
    });
  }
  if (request === 'ListSeats') return Promise.resolve({ seats: [seatRow] });
  return Promise.resolve({
    collectable_msat: 16_000_000,
    wallet: walletStatus(8_000_000)
  });
};

const renderPage = (destination: string | null) => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(
    world(destination) as typeof adminCallModule.adminCall
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PayoutsPage />
    </QueryClientProvider>
  );
};

const withdrawButtons = () => screen.queryAllByRole('button', { name: 'Withdraw' });

afterEach(() => {
  vi.restoreAllMocks();
});

describe('PayoutsPage', () => {
  it('should claim nothing before the daemon has answered', () => {
    renderPage('operator@example.com');

    expect(screen.getByText('Loading…')).toBeInTheDocument();
    expect(withdrawButtons()).toHaveLength(0);
  });

  // The two revenue sources are shaped differently and are kept apart, so the
  // screen cannot read as one uniform "withdraw everything" list.
  it('should keep seat-sale revenue and guardian-fee revenue in separate sections', async () => {
    renderPage('operator@example.com');

    await waitFor(() =>
      expect(screen.getByRole('heading', { name: 'Seat sales' })).toBeInTheDocument()
    );
    expect(screen.getByRole('heading', { name: 'Guardian fees' })).toBeInTheDocument();
  });

  it('should state that a withdrawal takes no amount and no gateway', async () => {
    renderPage('operator@example.com');

    await waitFor(() =>
      expect(screen.getByText(/no amount to enter, nothing to configure/)).toBeInTheDocument()
    );
  });

  // Both sections withdraw to the same address, so both are gated by the same
  // missing destination. Collecting is not: it moves fees inside the fleet.
  it('should gate both withdrawals while no payout destination is stored', async () => {
    renderPage(null);

    await waitFor(() => expect(withdrawButtons()).toHaveLength(2));
    for (const button of withdrawButtons()) expect(button).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Collect fees' })).toBeEnabled();
  });

  it('should offer both withdrawals once a destination is stored', async () => {
    renderPage('operator@example.com');

    await waitFor(() => expect(withdrawButtons()).toHaveLength(2));
    for (const button of withdrawButtons()) expect(button).toBeEnabled();
  });
});
