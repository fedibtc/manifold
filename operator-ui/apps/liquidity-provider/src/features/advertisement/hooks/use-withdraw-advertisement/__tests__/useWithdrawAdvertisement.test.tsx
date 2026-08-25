import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { ADVERTISEMENT_KEY } from '@/shared/api/queryKeys';
import { useWithdrawAdvertisement } from '../useWithdrawAdvertisement';

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

it('should call withdraw_advertisement and invalidate the advertisement key', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ publication_status: 'withdrawn', relay_states: [] });
  const client = makeClient();
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');

  const { result } = renderHook(() => useWithdrawAdvertisement(), { wrapper: wrap(client) });
  result.current.mutate(null);

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith('withdraw_advertisement', { reason: null });
  expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ADVERTISEMENT_KEY });
});
