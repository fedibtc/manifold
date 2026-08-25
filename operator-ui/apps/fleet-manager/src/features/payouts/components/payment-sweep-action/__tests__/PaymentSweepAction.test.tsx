import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { PaymentSweepAction } from '../PaymentSweepAction';

const FEDERATION_ID = 'fed1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';

interface Options {
  balanceMsat?: number | null;
  hasDestination?: boolean;
}

const renderAction = ({ balanceMsat = 250_000_000, hasDestination = true }: Options = {}) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PaymentSweepAction
        federationId={FEDERATION_ID}
        balanceMsat={balanceMsat}
        hasDestination={hasDestination}
      />
    </QueryClientProvider>
  );
};

const sweepButton = () => screen.getByRole('button', { name: 'Sweep' });
const payoutJob = {
  request_id: 'request-1',
  scope: { kind: 'payment_federation', federation_id: FEDERATION_ID },
  destination: 'operator@example.com',
  operation: { operation_id: 'op-payment-1', amount_msat: 250_000_000, committed_at_ms: 2 },
  created_at_ms: 1
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('PaymentSweepAction', () => {
  it('should sweep the federation with no amount and no gateway', async () => {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(payoutJob);
    renderAction();

    fireEvent.click(sweepButton());

    await waitFor(() =>
      expect(adminCall).toHaveBeenCalledWith({
        SweepPaymentFees: { federation_id: FEDERATION_ID, request_id: expect.any(String) }
      })
    );
  });

  it('should report the amount the sweep settled', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(payoutJob);
    renderAction();

    fireEvent.click(sweepButton());

    await waitFor(() => expect(screen.getByText('Sent 250,000 sats.')).toBeInTheDocument());
  });

  it('should offer the settled operation id for copying', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(payoutJob);
    renderAction();

    fireEvent.click(sweepButton());

    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Copy operation ID' })).toBeInTheDocument()
    );
  });

  // The daemon answers "no payout destination configured" here, and an operator
  // should not have to learn the ordering from a refusal.
  it('should block the sweep while no payout destination is stored', () => {
    renderAction({ hasDestination: false });

    expect(sweepButton()).toBeDisabled();
    expect(screen.getByText('Set a payout destination first.')).toBeInTheDocument();
  });

  it('should not call the daemon while the sweep is blocked', () => {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall');
    renderAction({ hasDestination: false });

    fireEvent.click(sweepButton());

    expect(adminCall).not.toHaveBeenCalled();
  });

  it('should block the sweep when the wallet is known to hold nothing', () => {
    renderAction({ balanceMsat: 0 });

    expect(sweepButton()).toBeDisabled();
    expect(screen.getByText('This wallet holds nothing to sweep.')).toBeInTheDocument();
  });

  // An unread balance is not an empty one: the daemon is the authority on
  // whether there is anything there, so the button stays live.
  it('should still allow a sweep when the balance could not be read', () => {
    renderAction({ balanceMsat: null });

    expect(sweepButton()).toBeEnabled();
  });

  it('should report a refused sweep', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new Error('no balance to sweep'));
    renderAction();

    fireEvent.click(sweepButton());

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('no balance to sweep'));
  });
});
