import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { PayoutSource } from '@/features/payouts/utils/payoutSource';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';
import { type WithdrawBalance, WithdrawDialog } from '../WithdrawDialog';

const FEDERATION: PayoutSource = {
  kind: 'payment_federation',
  federationId: 'fed1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
};
const SEAT: PayoutSource = { kind: 'guardian_fee', seatId: 'seat-earning-01' };

const renderDialog = (
  source: PayoutSource,
  balance: WithdrawBalance,
  destination: string | null = null
) => {
  const onStarted = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <WithdrawDialog
        source={source}
        balance={balance}
        destination={destination}
        onClose={vi.fn()}
        onChangeAddress={vi.fn()}
        onStarted={onStarted}
      />
    </QueryClientProvider>
  );
  return onStarted;
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('WithdrawDialog', () => {
  // Without an address only ecash can leave, so the dialog opens on it.
  it('should open on ecash when no payout address is saved', () => {
    renderDialog(FEDERATION, { availableMsat: 150_000_000, sweepableMsat: 149_250_000 });

    expect(screen.getByRole('radio', { name: 'Ecash' })).toBeChecked();
    fireEvent.click(screen.getByRole('radio', { name: 'Lightning' }));
    expect(screen.getByText('No payout address yet.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Withdraw 149,250 sats' })).toBeDisabled();
  });

  it('should refuse Lightning for a balance that fees would eat, and point to ecash', () => {
    renderDialog(FEDERATION, { availableMsat: 900_000, sweepableMsat: 0 }, 'a@b.c');

    expect(screen.getByRole('radio', { name: 'Lightning' })).toBeChecked();
    expect(screen.getByText(/too small to send over Lightning/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Withdraw over Lightning' })).toBeDisabled();
  });

  // A lost answer may hide a payout that already happened, so a retry must
  // carry the same request id for the daemon to answer with that payout.
  it('should retry a failed start under the same request id', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new AdminApiError('connection lost'))
      .mockResolvedValueOnce({ request_id: 'whatever', method: 'ecash' });
    const onStarted = renderDialog(FEDERATION, {
      availableMsat: 150_000_000,
      sweepableMsat: 149_250_000
    });

    fireEvent.click(screen.getByRole('button', { name: 'Withdraw 150,000 sats as ecash' }));
    expect(await screen.findByText('The withdrawal did not start')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

    await waitFor(() => expect(onStarted).toHaveBeenCalledWith('whatever'));
    const [first, second] = adminCall.mock.calls.map(
      ([request]) => request as { SweepPaymentFees: { request_id: string; method: string } }
    );
    expect(first?.SweepPaymentFees.method).toBe('ecash');
    expect(second?.SweepPaymentFees.request_id).toBe(first?.SweepPaymentFees.request_id);
  });

  it('should offer to collect the guardian fee pool first, warning about a small one', () => {
    renderDialog(SEAT, { availableMsat: 0, sweepableMsat: 0, poolMsat: 93_000 });

    expect(screen.getByText(/93 sats are still in the fee pool/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Collect 93 sats' })).toBeEnabled();
    expect(screen.getByText(/about 0.1 sat for every note/)).toBeInTheDocument();
  });
});
