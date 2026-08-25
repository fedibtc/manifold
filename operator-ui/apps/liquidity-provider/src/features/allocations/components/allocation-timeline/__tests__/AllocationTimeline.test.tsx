import type { AdminAllocationDetail, WalletOperation } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AllocationTimeline } from '@/features/allocations/components/allocation-timeline/AllocationTimeline';
import * as adminCallModule from '@/shared/api/adminCall';

const renderWithClient = (ui: ReactNode) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
};

const runningOperation: WalletOperation = {
  operation_id: 'op-1',
  operation_type: 'gateway_funding',
  amount: 1_500_000,
  status: 'broadcast',
  created_at: 1,
  updated_at: 2
};

const failedOperation: WalletOperation = {
  operation_id: 'op-2',
  operation_type: 'gateway_funding',
  amount: 750_000,
  status: 'failed',
  federation_id: 'ft-1',
  item_id: 'item-2',
  created_at: 3,
  updated_at: 4
};

const baseDetail: AdminAllocationDetail = {
  federation_id: 'ft-1',
  status: {
    details_payload_hash: [],
    provider_pubkey: '03bb',
    item_statuses: [
      {
        target: {
          gateway: { item_id: 'item-1', gateway_id: 'gw-1', gateway_name: 'Gateway', amount: 0 }
        },
        status: 'running',
        fulfilled_amount: null,
        completion_evidence: null,
        failure: null,
        updated_at: 0
      }
    ]
  },
  wallet_operations: [runningOperation],
  failures: []
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('AllocationTimeline', () => {
  it('should render a step for each wallet operation', () => {
    renderWithClient(<AllocationTimeline detail={baseDetail} />);
    expect(screen.getByText('Gateway funding')).toBeTruthy();
    expect(screen.getByText('Broadcast')).toBeTruthy();
    expect(screen.getByText('1,500,000 SATS')).toBeTruthy();
  });

  it('should render an empty state when there are no wallet operations', () => {
    renderWithClient(<AllocationTimeline detail={{ ...baseDetail, wallet_operations: [] }} />);
    expect(screen.getByText('No wallet operations recorded.')).toBeTruthy();
  });

  it('should render recorded failures', () => {
    const detail: AdminAllocationDetail = {
      ...baseDetail,
      failures: [{ code: 'gateway_timeout', message: 'Gateway did not respond', occurred_at: 3 }]
    };
    renderWithClient(<AllocationTimeline detail={detail} />);
    expect(screen.getByText('gateway_timeout')).toBeTruthy();
    expect(screen.getByText('Gateway did not respond')).toBeTruthy();
  });

  it('should not render the stale Phase 10 note', () => {
    renderWithClient(<AllocationTimeline detail={baseDetail} />);
    expect(screen.queryByText(/Phase 10/)).toBeNull();
    expect(screen.queryByText(/declined by the API/)).toBeNull();
  });

  it('should show a Retry button only on a failed step', () => {
    const detail: AdminAllocationDetail = {
      ...baseDetail,
      wallet_operations: [runningOperation, failedOperation]
    };
    renderWithClient(<AllocationTimeline detail={detail} />);
    expect(screen.getAllByRole('button', { name: 'Retry' })).toHaveLength(1);
  });

  it('should call retry_funding_step with the failed step identifiers when Retry is clicked', async () => {
    const adminCallSpy = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValue({ status: 'accepted' });
    const detail: AdminAllocationDetail = {
      ...baseDetail,
      wallet_operations: [failedOperation]
    };
    renderWithClient(<AllocationTimeline detail={detail} />);

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() =>
      expect(adminCallSpy).toHaveBeenCalledWith('retry_funding_step', {
        federation_id: 'ft-1',
        item_id: 'item-2',
        operation_id: 'op-2'
      })
    );
  });

  it('should show a success banner when a retry is accepted', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ status: 'accepted' });
    const detail: AdminAllocationDetail = { ...baseDetail, wallet_operations: [failedOperation] };
    renderWithClient(<AllocationTimeline detail={detail} />);

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    expect(await screen.findByText(/Retry submitted/)).toBeTruthy();
  });

  it('should show an error banner with the response detail when a retry is not_found', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      status: 'not_found',
      detail: 'no matching failed step'
    });
    const detail: AdminAllocationDetail = { ...baseDetail, wallet_operations: [failedOperation] };
    renderWithClient(<AllocationTimeline detail={detail} />);

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    expect(await screen.findByText('no matching failed step')).toBeTruthy();
  });

  it('should show an error banner via describeActionError when retry throws', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new Error('boom'));
    const detail: AdminAllocationDetail = { ...baseDetail, wallet_operations: [failedOperation] };
    renderWithClient(<AllocationTimeline detail={detail} />);

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    expect(await screen.findByText('boom')).toBeTruthy();
  });

  it.each([
    'pending',
    'running',
    'failed'
  ] as const)('should show Cancel allocation when status is %s', (status) => {
    const detail = {
      ...baseDetail,
      status: {
        ...baseDetail.status,
        item_statuses: [{ ...baseDetail.status.item_statuses[0], status }]
      }
    };
    renderWithClient(<AllocationTimeline detail={detail} />);
    expect(screen.getByRole('button', { name: 'Cancel allocation' })).toBeTruthy();
  });

  it.each([
    'completed',
    'cancelled'
  ] as const)('should hide Cancel allocation when status is %s', (status) => {
    const detail = {
      ...baseDetail,
      status: {
        ...baseDetail.status,
        item_statuses: [{ ...baseDetail.status.item_statuses[0], status }]
      }
    };
    renderWithClient(<AllocationTimeline detail={detail} />);
    expect(screen.queryByRole('button', { name: 'Cancel allocation' })).toBeNull();
  });

  it('should require a confirm step before the cancel mutation fires', async () => {
    const adminCallSpy = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValue({ status: 'accepted', allocation_status: 'cancelled' });
    renderWithClient(<AllocationTimeline detail={baseDetail} />);

    fireEvent.click(screen.getByRole('button', { name: 'Cancel allocation' }));
    expect(adminCallSpy).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Confirm cancel' }));
    await waitFor(() =>
      expect(adminCallSpy).toHaveBeenCalledWith('cancel_allocation', {
        federation_id: 'ft-1',
        reason: null
      })
    );
  });

  it('should not cancel when Back is clicked instead of confirming', () => {
    const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall');
    renderWithClient(<AllocationTimeline detail={baseDetail} />);

    fireEvent.click(screen.getByRole('button', { name: 'Cancel allocation' }));
    fireEvent.click(screen.getByRole('button', { name: 'Back' }));

    expect(adminCallSpy).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Cancel allocation' })).toBeTruthy();
  });

  it('should show a success banner when a cancel is accepted', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      status: 'accepted',
      allocation_status: 'cancelled'
    });
    renderWithClient(<AllocationTimeline detail={baseDetail} />);

    fireEvent.click(screen.getByRole('button', { name: 'Cancel allocation' }));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm cancel' }));

    expect(await screen.findByText('Allocation cancelled.')).toBeTruthy();
  });

  it('should show an error banner with the response detail when a cancel is rejected', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      status: 'rejected',
      detail: 'allocation already in a terminal state'
    });
    renderWithClient(<AllocationTimeline detail={baseDetail} />);

    fireEvent.click(screen.getByRole('button', { name: 'Cancel allocation' }));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm cancel' }));

    expect(await screen.findByText('allocation already in a terminal state')).toBeTruthy();
  });

  it('should show an error banner via describeActionError when cancel throws', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new Error('boom'));
    renderWithClient(<AllocationTimeline detail={baseDetail} />);

    fireEvent.click(screen.getByRole('button', { name: 'Cancel allocation' }));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm cancel' }));

    expect(await screen.findByText('boom')).toBeTruthy();
  });
});
