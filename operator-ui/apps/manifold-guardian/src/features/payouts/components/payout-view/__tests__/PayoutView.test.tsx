import type { LightningPayoutStatus } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { PayoutView } from '../PayoutView';

const lightning = (
  state: 'pending' | 'succeeded' | 'failed_or_refunded',
  capped: { maximum_msat: number; remaining_msat: number } | null
): LightningPayoutStatus => ({
  job: {
    request_id: 'r1',
    scope: { kind: 'payment_federation', federation_id: 'fed1aaa' },
    method: 'lightning',
    destination: 'operator@example.com',
    operation: {
      operation_id: 'op-1',
      amount_msat: 100_000_000,
      capped,
      committed_at_ms: 2,
      confirmed_at_ms: null
    },
    created_at_ms: 1
  },
  payout: {
    operation_id: 'op-1',
    rail: 'lnv2',
    state,
    recipient_amount_msat: 100_000_000,
    contract_amount_msat: 100_500_000,
    encumbered_msat: 0,
    has_active_state_machines: false
  }
});

const renderView = (status: LightningPayoutStatus) => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(status);
  const onWithdrawAgain = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PayoutView requestId="r1" onClose={vi.fn()} onWithdrawAgain={onWithdrawAgain} />
    </QueryClientProvider>
  );
  return onWithdrawAgain;
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('PayoutView', () => {
  // A destination that caps a payment leaves the rest behind; the receipt says
  // how much and offers to send it.
  it('should state the cap and offer to withdraw what it left', async () => {
    const onWithdrawAgain = renderView(
      lightning('succeeded', { maximum_msat: 100_000_000, remaining_msat: 49_500_000 })
    );

    expect(await screen.findByText('Sent 100,000 sats')).toBeInTheDocument();
    expect(
      screen.getByText(
        'operator@example.com accepts at most 100,000 sats per payment. 49,500 sats are still in this balance.'
      )
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Withdraw the remaining 49,500 sats' }));
    expect(onWithdrawAgain).toHaveBeenCalledWith({
      kind: 'payment_federation',
      federationId: 'fed1aaa'
    });
  });

  // The old dashboard printed "Sent" from the job that started the payment. A
  // payment is sent only once its operation succeeded, and a payment still on
  // its way can be closed: it continues, and the page lists it.
  it('should not say Sent while the payment is pending, and let it be closed', async () => {
    const onClose = vi.fn();
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(lightning('pending', null));
    render(
      <QueryClientProvider client={new QueryClient()}>
        <PayoutView requestId="r1" onClose={onClose} onWithdrawAgain={vi.fn()} />
      </QueryClientProvider>
    );

    expect(
      await screen.findByText('Paying operator@example.com over Lightning')
    ).toBeInTheDocument();
    expect(screen.queryByText(/^Sent/)).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(onClose).toHaveBeenCalled();
  });

  it('should say a failed payment did not go through, never that it was sent', async () => {
    renderView(lightning('failed_or_refunded', null));

    expect(await screen.findByText('The payment did not go through')).toBeInTheDocument();
    expect(screen.queryByText(/^Sent/)).toBeNull();
    expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument();
  });
});
