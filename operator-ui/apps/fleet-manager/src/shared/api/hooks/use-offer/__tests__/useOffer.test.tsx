import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, vi } from 'vitest';
import * as adminCallModule from '../../../adminCall';
import { useOffer } from '../useOffer';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('should call adminCall with ShowPlans and return the response', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ plans: [] });

  const { result } = renderHook(() => useOffer(), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(result.current.data).toEqual({ plans: [] });
  expect(adminCallSpy).toHaveBeenCalledWith('ShowPlans');
});

it('should not poll — useSetPrice invalidation is the only way this refetches', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ plans: [] });

  renderHook(() => useOffer(), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(5 * 60_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(1);
});
