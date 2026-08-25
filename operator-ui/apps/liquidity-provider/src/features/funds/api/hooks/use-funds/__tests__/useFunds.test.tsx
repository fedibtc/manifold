import type { GetFundsResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AuthError } from '@/shared/api/errors';
import { useFunds } from '../useFunds';

const funds: GetFundsResponse = {
  balance: {
    spendable: 4_200_000,
    pending_incoming: 150_000,
    pending_outgoing: 50_000,
    in_flight_allocations: 800_000,
    fee_reserve: 150_000,
    available_balance: 3_250_000
  },
  replenishment: 'ok',
  gateway: {
    gateway_id: 'gw-signet-01',
    gateway_name: 'Mock Signet Gateway',
    status: 'available',
    available_amount: 3_000_000,
    observed_at: 1721476800
  },
  stability_pool: { status: 'available', available_amount: 250_000, observed_at: 1721476800 },
  effective_liquidity: [{ source_type: 'gateway', gateway_id: 'gw-signet-01', amount: 3_000_000 }]
};

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('should call get_funds and return the funds snapshot on success', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(funds);

  const { result } = renderHook(() => useFunds(), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(result.current.data).toEqual(funds);
  expect(adminCallSpy).toHaveBeenCalledWith('get_funds', null);
});

it('should surface an AuthError immediately without retrying', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AuthError());

  const { result } = renderHook(() => useFunds(), { wrapper });

  await waitFor(() => expect(result.current.isError).toBe(true));
  expect(result.current.error).toBeInstanceOf(AuthError);
  expect(adminCallSpy).toHaveBeenCalledTimes(1);
});

it('should refetch every 30s', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(funds);

  renderHook(() => useFunds(), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(30_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});
