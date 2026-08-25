import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, vi } from 'vitest';
import * as adminCallModule from '../../../adminCall';
import { useSeats } from '../useSeats';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('should call adminCall with ListSeats and return the response', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ seats: [] });

  const { result } = renderHook(() => useSeats(), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(result.current.data).toEqual({ seats: [] });
  expect(adminCallSpy).toHaveBeenCalledWith('ListSeats');
});

it('should refetch every 30s', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ seats: [] });

  renderHook(() => useSeats(), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(30_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});
