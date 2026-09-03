import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { GuardianFeeActions } from '../GuardianFeeActions';

interface Options {
  collectableMsat?: number | null;
  collectedEcashMsat?: number | null;
  hasDestination?: boolean;
}

const renderActions = ({
  collectableMsat = 16_000_000,
  collectedEcashMsat = 8_000_000,
  hasDestination = true
}: Options = {}) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <GuardianFeeActions
        seatId="seat-earning-01"
        collectableMsat={collectableMsat}
        collectedEcashMsat={collectedEcashMsat}
        hasDestination={hasDestination}
      />
    </QueryClientProvider>
  );
};

const collectButton = () => screen.getByRole('button', { name: '1. Collect out of the pool' });
const sendButton = () => screen.getByRole('button', { name: '2. Send to destination' });

afterEach(() => {
  vi.restoreAllMocks();
});

describe('GuardianFeeActions', () => {
  it('should collect the seat out of the pool', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValue({
        claimed_msat: '13000000',
        recorded_claimed_msat: '13000000',
        awaiting_cycle_msat: '3000000',
      });
    renderActions();

    fireEvent.click(collectButton());

    await waitFor(() =>
      expect(adminCall).toHaveBeenCalledWith({
        CollectGuardianFees: { seat_id: 'seat-earning-01' }
      })
    );
  });

  // The load-bearing one. A collection takes what the pool will release now;
  // locked deposits leave at the next cycle turnover. Reporting only the claimed
  // figure would tell the operator the account was emptied when it was not.
  it('should report what was claimed and what is still waiting for the cycle', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      claimed_msat: '13000000',
      recorded_claimed_msat: '13000000',
      awaiting_cycle_msat: '3000000',
    });
    renderActions();

    fireEvent.click(collectButton());

    await waitFor(() => expect(screen.getByText(/Claimed 13,000 sats/)).toBeInTheDocument());
    expect(
      screen.getByText(/3,000 sats stay locked until the next cycle turnover/)
    ).toBeInTheDocument();
  });

  it('should state the waiting figure even when nothing is locked', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      claimed_msat: '13000000',
      recorded_claimed_msat: '13000000',
      awaiting_cycle_msat: '0',
    });
    renderActions();

    fireEvent.click(collectButton());

    await waitFor(() =>
      expect(screen.getByText(/0 sats are waiting for the next cycle turnover/)).toBeInTheDocument()
    );
  });

  it('should send the collected ecash to the destination', async () => {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      request_id: 'request-1',
      scope: {
        kind: 'guardian_fee',
        federation_id: 'fed1fees',
        seat_id: 'seat-earning-01',
        invite_code: 'invite'
      },
      destination: 'operator@example.com',
      operation: { operation_id: 'op-fees-1', amount_msat: 8_000_000, committed_at_ms: 2 },
      created_at_ms: 1
    });
    renderActions();

    fireEvent.click(sendButton());

    await waitFor(() =>
      expect(adminCall).toHaveBeenCalledWith({
        SweepGuardianFees: { seat_id: 'seat-earning-01', request_id: expect.any(String) }
      })
    );
  });

  it('should report the amount the send settled', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      request_id: 'request-1',
      scope: {
        kind: 'guardian_fee',
        federation_id: 'fed1fees',
        seat_id: 'seat-earning-01',
        invite_code: 'invite'
      },
      destination: 'operator@example.com',
      operation: { operation_id: 'op-fees-1', amount_msat: 8_000_000, committed_at_ms: 2 },
      created_at_ms: 1
    });
    renderActions();

    fireEvent.click(sendButton());

    await waitFor(() => expect(screen.getByText('Sent 8,000 sats.')).toBeInTheDocument());
  });

  it('should block the send while no payout destination is stored', () => {
    renderActions({ hasDestination: false });

    expect(sendButton()).toBeDisabled();
    expect(screen.getByText('Set a payout destination first.')).toBeInTheDocument();
  });

  // Collecting moves money out of the pool into the fleet's own ecash. Nothing
  // leaves the fleet, so the daemon needs no destination for it — and neither
  // does this control.
  it('should still allow collecting while no payout destination is stored', () => {
    renderActions({ hasDestination: false });

    expect(collectButton()).toBeEnabled();
  });

  it('should block the send until something has been collected', () => {
    renderActions({ collectedEcashMsat: 0 });

    expect(sendButton()).toBeDisabled();
    expect(screen.getByText('Nothing collected yet. Collect first.')).toBeInTheDocument();
  });

  it('should block collecting when the pool is known to hold nothing', () => {
    renderActions({ collectableMsat: 0 });

    expect(collectButton()).toBeDisabled();
    expect(screen.getByText('Nothing in the pool to collect.')).toBeInTheDocument();
  });

  it('should still allow both steps when the fee account could not be read', () => {
    renderActions({ collectableMsat: null, collectedEcashMsat: null });

    expect(collectButton()).toBeEnabled();
    expect(sendButton()).toBeEnabled();
  });

  it('should report a refused collection', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(
      new Error('seat has no federation yet')
    );
    renderActions();

    fireEvent.click(collectButton());

    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent('seat has no federation yet')
    );
  });
});
