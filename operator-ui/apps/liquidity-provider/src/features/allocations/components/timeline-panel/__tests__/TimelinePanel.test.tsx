import type { AdminAllocationDetail } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('@/features/allocations/api/hooks/use-allocation/useAllocation');

import { useAllocation } from '@/features/allocations/api/hooks/use-allocation/useAllocation';
import { TimelinePanel } from '../TimelinePanel';

const mockUseAllocation = vi.mocked(useAllocation);

const detail: AdminAllocationDetail = {
  federation_id: 'ft-1',
  status: {
    details_payload_hash: [],
    provider_pubkey: '03bb',
    item_statuses: [
      {
        target: {
          gateway: { item_id: 'item-1', gateway_id: 'gw-1', gateway_name: 'Gateway', amount: 0 }
        },
        status: 'completed',
        fulfilled_amount: null,
        completion_evidence: null,
        failure: null,
        updated_at: 0
      }
    ]
  },
  wallet_operations: [
    {
      operation_id: 'op-1',
      operation_type: 'deposit',
      amount: 1_234_567,
      status: 'confirmed',
      created_at: 1,
      updated_at: 2
    }
  ],
  failures: []
};

const asDetail = (value: unknown) => value as unknown as ReturnType<typeof useAllocation>;

const renderPanel = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <TimelinePanel allocationId="ft-1" />
    </QueryClientProvider>
  );
};

afterEach(() => {
  vi.clearAllMocks();
});

describe('TimelinePanel', () => {
  it('should title the panel with the selected allocation id', () => {
    mockUseAllocation.mockReturnValue(
      asDetail({ data: { allocation: detail }, isLoading: false, isError: false })
    );

    renderPanel();

    expect(screen.getByText('In flight — ft-1')).toBeTruthy();
  });

  it('should render the timeline once the detail loads', () => {
    mockUseAllocation.mockReturnValue(
      asDetail({ data: { allocation: detail }, isLoading: false, isError: false })
    );

    renderPanel();

    expect(screen.getByText('Deposit')).toBeTruthy();
    expect(screen.getByText('Confirmed')).toBeTruthy();
  });

  it('should render a loading state while the detail fetches', () => {
    mockUseAllocation.mockReturnValue(
      asDetail({ data: undefined, isLoading: true, isError: false })
    );

    renderPanel();

    expect(screen.getByText('Loading timeline…')).toBeTruthy();
  });

  it('should render an error state when the detail fails', () => {
    mockUseAllocation.mockReturnValue(
      asDetail({ data: undefined, isLoading: false, isError: true })
    );

    renderPanel();

    expect(screen.getByText('Could not load timeline.')).toBeTruthy();
  });
});
