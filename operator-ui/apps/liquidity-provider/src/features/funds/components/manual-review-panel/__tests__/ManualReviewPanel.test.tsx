import type { GetWalletOperationResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { ManualReviewPanel } from '../ManualReviewPanel';

const frozenOperation: GetWalletOperationResponse = {
  operation: {
    operation_id: 'wop-frozen',
    operation_type: 'withdrawal',
    amount: 250_000,
    address: 'tb1qdestination',
    txid: null,
    tx_vout: null,
    status: 'manual_review_required',
    confirmation_count: null,
    federation_id: null,
    item_id: null,
    created_at: 1721304000,
    updated_at: 1721307600,
    failure: {
      code: 'gateway_unavailable',
      message: 'gateway did not answer the send',
      occurred_at: 1721304000,
      federation_id: null,
      item_id: null
    }
  }
};

const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    {children}
  </QueryClientProvider>
);

const renderPanel = (onClose = vi.fn()) =>
  render(<ManualReviewPanel operationId="wop-frozen" onClose={onClose} />, { wrapper });

const mockAdminCall = () =>
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (method: string) => {
    if (method === 'get_wallet_operation') return frozenOperation as never;
    return { status: 'accepted', operation: null, detail: null } as never;
  });

describe('ManualReviewPanel', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  // The list this is opened from carries none of these fields. Showing them is
  // the reason the daemon grew a read verb for one operation.
  it('should show the destination, the age and what chain evidence exists', async () => {
    mockAdminCall();

    renderPanel();

    expect(await screen.findByText('tb1qdestination')).toBeTruthy();
    expect(screen.getByText('250,000 sats')).toBeTruthy();
    expect(screen.getByText('2024-07-18 12:00')).toBeTruthy();
    expect(screen.getAllByText('None recorded').length).toBeGreaterThan(0);
    expect(screen.getByText('gateway did not answer the send')).toBeTruthy();
  });

  // The daemon refuses `completed` without a txid. Catching it here means the
  // operator is told at the field rather than after a round trip.
  it('should refuse to resolve as completed without a transaction id', async () => {
    const adminCall = mockAdminCall();

    renderPanel();
    await screen.findByText('tb1qdestination');

    fireEvent.change(screen.getByLabelText('Outcome'), { target: { value: 'completed' } });
    fireEvent.click(screen.getByRole('button', { name: 'Resolve' }));

    expect(screen.getByText('Enter the transaction that settled this send.')).toBeTruthy();
    expect(adminCall).not.toHaveBeenCalledWith('resolve_manual_review', expect.anything());
  });

  it('should send the txid with a completed resolution and close', async () => {
    const adminCall = mockAdminCall();
    const onClose = vi.fn();

    renderPanel(onClose);
    await screen.findByText('tb1qdestination');

    fireEvent.change(screen.getByLabelText('Outcome'), { target: { value: 'completed' } });
    fireEvent.change(screen.getByLabelText('Transaction id'), { target: { value: 'deadbeef' } });
    fireEvent.change(screen.getByLabelText('Reason (optional)'), {
      target: { value: 'confirmed in mempool by ops' }
    });
    fireEvent.click(screen.getByRole('button', { name: 'Resolve' }));

    await waitFor(() =>
      expect(adminCall).toHaveBeenCalledWith('resolve_manual_review', {
        operation_id: 'wop-frozen',
        resolution: 'completed',
        txid: 'deadbeef',
        reason: 'confirmed in mempool by ops'
      })
    );
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  // `failed` and `safe_to_retry` assert no send happened, so the daemon rejects
  // a txid supplied with either. The field is not even offered for them.
  it('should not offer a transaction id for the resolutions that assert no send', async () => {
    const adminCall = mockAdminCall();

    renderPanel();
    await screen.findByText('tb1qdestination');

    expect(screen.queryByLabelText('Transaction id')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Resolve' }));

    await waitFor(() =>
      expect(adminCall).toHaveBeenCalledWith('resolve_manual_review', {
        operation_id: 'wop-frozen',
        resolution: 'safe_to_retry',
        txid: null,
        reason: null
      })
    );
  });
});
