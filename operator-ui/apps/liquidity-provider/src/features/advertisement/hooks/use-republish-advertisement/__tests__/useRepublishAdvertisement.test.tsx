import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { ADVERTISEMENT_KEY } from '@/shared/api/queryKeys';
import { useRepublishAdvertisement } from '../useRepublishAdvertisement';

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

it('should call republish_advertisement and invalidate the advertisement key', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ publication_status: 'published', relay_states: [] });
  const client = makeClient();
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');

  const { result } = renderHook(() => useRepublishAdvertisement(), { wrapper: wrap(client) });
  result.current.mutate();

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith('republish_advertisement', { force: true });
  expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ADVERTISEMENT_KEY });
});
