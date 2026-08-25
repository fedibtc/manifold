import type { AdminAllocationDetail, AdminAllocationSummary } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, vi } from 'vitest';
import { useAllocation } from '@/features/allocations/api/hooks/use-allocation/useAllocation';
import { useAllocations } from '@/features/allocations/api/hooks/use-allocations/useAllocations';
import { AllocationsPage } from '../AllocationsPage';

vi.mock('@/features/allocations/api/hooks/use-allocations/useAllocations');
vi.mock('@/features/allocations/api/hooks/use-allocation/useAllocation');

const mockUseAllocations = vi.mocked(useAllocations);
const mockUseAllocation = vi.mocked(useAllocation);

// AllocationTimeline uses the retry/cancel mutation hooks (useQueryClient),
// so rendering it — even via the mocked list/detail hooks above — needs a
// QueryClientProvider ancestor.
const renderWithClient = (ui: ReactNode) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
};

const summary: AdminAllocationSummary = {
  federation_id: 'ft-1',
  gateway_status: 'completed',
  stability_pool_status: null,
  committed_amount: 1_234_567,
  created_at: 1,
  updated_at: 2
};

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

const asList = (value: unknown) => value as unknown as ReturnType<typeof useAllocations>;
const asDetail = (value: unknown) => value as unknown as ReturnType<typeof useAllocation>;

const idle = { data: undefined, isLoading: false, isError: false };

afterEach(() => {
  vi.clearAllMocks();
});

it('should render a row for each allocation', () => {
  mockUseAllocations.mockReturnValue(
    asList({
      data: { allocations: { items: [summary], next_page: null } },
      isLoading: false,
      isError: false
    })
  );
  mockUseAllocation.mockReturnValue(asDetail(idle));

  renderWithClient(<AllocationsPage />);

  expect(screen.getByRole('button', { name: 'ft-1' })).toBeTruthy();
  expect(screen.getByText('1,234,567')).toBeTruthy();
  expect(screen.getByText('Completed')).toBeTruthy();
});

it('should show an empty state when there are no allocations', () => {
  mockUseAllocations.mockReturnValue(
    asList({
      data: { allocations: { items: [], next_page: null } },
      isLoading: false,
      isError: false
    })
  );
  mockUseAllocation.mockReturnValue(asDetail(idle));

  renderWithClient(<AllocationsPage />);

  expect(screen.getByText('No allocations yet.')).toBeTruthy();
});

it('should show a loading state while the list loads', () => {
  mockUseAllocations.mockReturnValue(asList({ data: undefined, isLoading: true, isError: false }));
  mockUseAllocation.mockReturnValue(asDetail(idle));

  renderWithClient(<AllocationsPage />);

  expect(screen.getByText('Loading allocations…')).toBeTruthy();
});

it('should show an error state when the list fails', () => {
  mockUseAllocations.mockReturnValue(asList({ data: undefined, isLoading: false, isError: true }));
  mockUseAllocation.mockReturnValue(asDetail(idle));

  renderWithClient(<AllocationsPage />);

  expect(screen.getByText('Could not load allocations.')).toBeTruthy();
});

it('should keep the rows visible under a stale banner when a refetch fails', () => {
  mockUseAllocations.mockReturnValue(
    asList({
      data: { allocations: { items: [summary], next_page: null } },
      isLoading: false,
      isError: true,
      dataUpdatedAt: 1721476800000
    })
  );
  mockUseAllocation.mockReturnValue(asDetail(idle));

  renderWithClient(<AllocationsPage />);

  expect(screen.getByRole('button', { name: 'ft-1' })).toBeTruthy();
  expect(screen.getByText('Showing last-known data')).toBeTruthy();
  expect(screen.queryByText('Could not load allocations.')).toBeNull();
});

it('should keep the timeline visible under a stale banner when its refetch fails', () => {
  mockUseAllocations.mockReturnValue(
    asList({
      data: { allocations: { items: [summary], next_page: null } },
      isLoading: false,
      isError: false
    })
  );
  mockUseAllocation.mockReturnValue(
    asDetail({
      data: { allocation: detail },
      isLoading: false,
      isError: true,
      dataUpdatedAt: 1721476800000
    })
  );

  renderWithClient(<AllocationsPage />);

  fireEvent.click(screen.getByRole('button', { name: 'ft-1' }));

  expect(screen.getByText('Deposit')).toBeTruthy();
  expect(screen.getByText('Showing last-known data')).toBeTruthy();
  expect(screen.queryByText('Could not load timeline.')).toBeNull();
});

it('should expand the inline timeline when a row is selected', () => {
  mockUseAllocations.mockReturnValue(
    asList({
      data: { allocations: { items: [summary], next_page: null } },
      isLoading: false,
      isError: false
    })
  );
  mockUseAllocation.mockReturnValue(
    asDetail({ data: { allocation: detail }, isLoading: false, isError: false })
  );

  renderWithClient(<AllocationsPage />);

  expect(screen.queryByText(/In flight —/)).toBeNull();

  fireEvent.click(screen.getByRole('button', { name: 'ft-1' }));

  expect(screen.getByText('In flight — ft-1')).toBeTruthy();
  expect(screen.getByText('Deposit')).toBeTruthy();
  expect(screen.getByText('Confirmed')).toBeTruthy();
});

it('should show a timeline error when the detail fails', () => {
  mockUseAllocations.mockReturnValue(
    asList({
      data: { allocations: { items: [summary], next_page: null } },
      isLoading: false,
      isError: false
    })
  );
  mockUseAllocation.mockReturnValue(asDetail({ data: undefined, isLoading: false, isError: true }));

  renderWithClient(<AllocationsPage />);

  fireEvent.click(screen.getByRole('button', { name: 'ft-1' }));

  expect(screen.getByText('Could not load timeline.')).toBeTruthy();
});
