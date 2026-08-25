import type { GetHealthResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AuthError } from '@/shared/api/errors';
import { SYSTEM_HEALTH_KEY, useSystemHealth } from '../useSystemHealth';

const health: GetHealthResponse = {
  overall_status: 'healthy',
  mode: 'normal',
  observed_at: 1721476800,
  components: [{ component: 'daemon', status: 'healthy', detail: null, observed_at: 1721476800 }]
};

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('should expose a stable query key', () => {
  expect(SYSTEM_HEALTH_KEY).toEqual(['system-health']);
});

it('should call get_health and return the health snapshot on success', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(health);

  const { result } = renderHook(() => useSystemHealth(), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(result.current.data).toEqual(health);
  expect(adminCallSpy).toHaveBeenCalledWith('get_health', null);
});

it('should surface an AuthError immediately without retrying', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AuthError());

  const { result } = renderHook(() => useSystemHealth(), { wrapper });

  await waitFor(() => expect(result.current.isError).toBe(true));
  expect(result.current.error).toBeInstanceOf(AuthError);
  expect(adminCallSpy).toHaveBeenCalledTimes(1);
});

it('should refetch every 30s', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(health);

  renderHook(() => useSystemHealth(), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(30_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});
