import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AuthError } from '@/shared/api/errors';
import { useAdvertisementState } from '../useAdvertisementState';

const makeClient = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

const wrap = (client: QueryClient) => {
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return Wrapper;
};

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('should call get_advertisement_state and return the state on success', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ publication_status: 'published', relay_states: [] });

  const { result } = renderHook(() => useAdvertisementState(), { wrapper: wrap(makeClient()) });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(result.current.data).toEqual({ publication_status: 'published', relay_states: [] });
  expect(adminCallSpy).toHaveBeenCalledWith('get_advertisement_state', null);
});

it('should surface an AuthError immediately without retrying', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AuthError());

  const { result } = renderHook(() => useAdvertisementState(), { wrapper: wrap(makeClient()) });

  await waitFor(() => expect(result.current.isError).toBe(true));
  expect(result.current.error).toBeInstanceOf(AuthError);
  expect(adminCallSpy).toHaveBeenCalledTimes(1);
});

it('should refetch every 30s', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ publication_status: 'published', relay_states: [] });

  renderHook(() => useAdvertisementState(), { wrapper: wrap(makeClient()) });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(30_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});
