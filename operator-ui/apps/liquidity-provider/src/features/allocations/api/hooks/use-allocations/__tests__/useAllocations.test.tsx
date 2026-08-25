import type { ListAllocationsResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { NetworkError } from '@/shared/api/errors';
import { useAllocations } from '../useAllocations';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

const response: ListAllocationsResponse = {
  allocations: {
    items: [
      {
        federation_id: 'fed-1',
        gateway_status: 'completed',
        stability_pool_status: null,
        committed_amount: 1_000_000,
        created_at: 1,
        updated_at: 2
      }
    ],
    next_page: null
  }
};

const pendingResponse: ListAllocationsResponse = {
  allocations: {
    items: [
      {
        federation_id: 'fed-1',
        gateway_status: 'pending',
        stability_pool_status: null,
        committed_amount: 1_000_000,
        created_at: 1,
        updated_at: 2
      }
    ],
    next_page: null
  }
};

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('should call list_allocations with a default first-page request', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(response);

  const { result } = renderHook(() => useAllocations(), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(result.current.data).toEqual(response);
  expect(adminCallSpy).toHaveBeenCalledWith('list_allocations', {
    page: { cursor: null, limit: 50 },
    time_range: null
  });
});

it('should surface a NetworkError when the daemon is unreachable', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new NetworkError());

  const { result } = renderHook(() => useAllocations(), { wrapper });

  await waitFor(() => expect(result.current.isError).toBe(true));
  expect(result.current.error).toBeInstanceOf(NetworkError);
});

it('should refetch every 5s while a non-terminal allocation exists', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(pendingResponse);

  renderHook(() => useAllocations(), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});

it('should stop polling once every allocation is terminal', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(response);

  renderHook(() => useAllocations(), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(1);
});
