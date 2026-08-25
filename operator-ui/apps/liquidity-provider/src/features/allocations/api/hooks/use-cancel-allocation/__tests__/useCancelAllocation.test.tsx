import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { ALLOCATION_KEY } from '@/features/allocations/api/hooks/use-allocation/useAllocation';
import { ALLOCATIONS_KEY } from '@/features/allocations/api/hooks/use-allocations/useAllocations';
import * as adminCallModule from '@/shared/api/adminCall';
import { useCancelAllocation } from '../useCancelAllocation';

const makeClient = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

const wrap = (client: QueryClient) => {
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return Wrapper;
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should call cancel_allocation and invalidate the allocations and allocation keys', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ status: 'accepted', allocation_status: 'cancelled' });
  const client = makeClient();
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');

  const { result } = renderHook(() => useCancelAllocation(), { wrapper: wrap(client) });
  result.current.mutate({ federation_id: 'ft-0002', reason: null });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith('cancel_allocation', {
    federation_id: 'ft-0002',
    reason: null
  });
  expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ALLOCATIONS_KEY });
  expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ALLOCATION_KEY });
});
