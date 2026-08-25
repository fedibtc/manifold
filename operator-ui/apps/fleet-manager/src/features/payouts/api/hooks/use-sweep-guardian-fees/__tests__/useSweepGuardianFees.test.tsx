import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { useSweepGuardianFees } from '../useSweepGuardianFees';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useSweepGuardianFees', () => {
  const job = {
    request_id: 'request-1',
    scope: {
      kind: 'guardian_fee' as const,
      federation_id: 'fed1aaa',
      seat_id: 'seat-01',
      invite_code: 'invite'
    },
    destination: 'operator@example.com',
    operation: { operation_id: 'op-fees-1', amount_msat: 8_000_000, committed_at_ms: 2 },
    created_at_ms: 1
  };
  it('should sweep one seat with no amount and no gateway', async () => {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(job);

    const { result } = renderHook(() => useSweepGuardianFees('seat-01'), { wrapper });
    result.current.mutate();

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(adminCall).toHaveBeenCalledWith({
      SweepGuardianFees: { seat_id: 'seat-01', request_id: expect.any(String) }
    });
  });

  it('should answer with the settled operation and amount', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(job);

    const { result } = renderHook(() => useSweepGuardianFees('seat-01'), { wrapper });
    result.current.mutate();

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data).toEqual(job);
  });

  it('should retain the id after failure and rotate it after success', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new Error('lost response'))
      .mockResolvedValue(job);
    const { result } = renderHook(() => useSweepGuardianFees('seat-01'), { wrapper });

    await expect(result.current.mutateAsync()).rejects.toThrow('lost response');
    await act(async () => {
      await result.current.mutateAsync();
    });
    await act(async () => {
      await result.current.mutateAsync();
    });

    const ids = adminCall.mock.calls.map(([request]) => {
      if (typeof request !== 'object' || !('SweepGuardianFees' in request)) {
        throw new Error('expected SweepGuardianFees request');
      }
      return request.SweepGuardianFees.request_id;
    });
    expect(ids[0]).toBe(ids[1]);
    expect(ids[2]).not.toBe(ids[1]);
  });
});
