import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { useSweepPaymentFees } from '../useSweepPaymentFees';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useSweepPaymentFees', () => {
  const job = {
    request_id: 'request-1',
    scope: { kind: 'payment_federation' as const, federation_id: 'fed1aaa' },
    destination: 'operator@example.com',
    operation: { operation_id: 'op-1', amount_msat: 250_000_000, committed_at_ms: 2 },
    created_at_ms: 1
  };
  // The request carries the federation and a retry-stable request id: no amount, because a
  // sweep takes the largest economically fundable amount, and no gateway,
  // because the daemon selects one.
  it('should ask for the federation alone', async () => {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(job);

    const { result } = renderHook(() => useSweepPaymentFees('fed1aaa'), { wrapper });
    result.current.mutate();

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(adminCall).toHaveBeenCalledWith({
      SweepPaymentFees: { federation_id: 'fed1aaa', request_id: expect.any(String) }
    });
  });

  it('should answer with the settled operation and amount', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(job);

    const { result } = renderHook(() => useSweepPaymentFees('fed1aaa'), { wrapper });
    result.current.mutate();

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data).toEqual(job);
  });

  it('should retain the id after failure and rotate it after success', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new Error('lost response'))
      .mockResolvedValue(job);
    const { result } = renderHook(() => useSweepPaymentFees('fed1aaa'), { wrapper });

    await expect(result.current.mutateAsync()).rejects.toThrow('lost response');
    await act(async () => {
      await result.current.mutateAsync();
    });
    await act(async () => {
      await result.current.mutateAsync();
    });

    const ids = adminCall.mock.calls.map(([request]) => {
      if (typeof request !== 'object' || !('SweepPaymentFees' in request)) {
        throw new Error('expected SweepPaymentFees request');
      }
      return request.SweepPaymentFees.request_id;
    });
    expect(ids[0]).toBe(ids[1]);
    expect(ids[2]).not.toBe(ids[1]);
  });
});
