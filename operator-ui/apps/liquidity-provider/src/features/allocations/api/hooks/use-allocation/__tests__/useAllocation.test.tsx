import type { GetAdminAllocationResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { useAllocation } from '../useAllocation';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

const response: GetAdminAllocationResponse = {
  allocation: {
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
    wallet_operations: [],
    failures: []
  }
};

const pendingResponse: GetAdminAllocationResponse = {
  allocation: {
    federation_id: 'ft-1',
    status: {
      details_payload_hash: [],
      provider_pubkey: '03bb',
      item_statuses: [
        {
          target: {
            gateway: { item_id: 'item-1', gateway_id: 'gw-1', gateway_name: 'Gateway', amount: 0 }
          },
          status: 'pending',
          fulfilled_amount: null,
          completion_evidence: null,
          failure: null,
          updated_at: 0
        }
      ]
    },
    wallet_operations: [],
    failures: []
  }
};

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('should call get_allocation with the funding target id', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(response);

  const { result } = renderHook(() => useAllocation('ft-1'), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(result.current.data).toEqual(response);
  expect(adminCallSpy).toHaveBeenCalledWith('get_allocation', { federation_id: 'ft-1' });
});

it('should stay disabled and not call the daemon when no id is selected', () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(response);

  const { result } = renderHook(() => useAllocation(null), { wrapper });

  expect(result.current.fetchStatus).toBe('idle');
  expect(adminCallSpy).not.toHaveBeenCalled();
});

it('should refetch every 5s while the allocation is non-terminal', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(pendingResponse);

  renderHook(() => useAllocation('ft-1'), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});

it('should stop polling once the allocation is terminal', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(response);

  renderHook(() => useAllocation('ft-1'), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(1);
});
