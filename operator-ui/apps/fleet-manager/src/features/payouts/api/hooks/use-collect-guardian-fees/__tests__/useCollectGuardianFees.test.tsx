import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { useCollectGuardianFees } from '../useCollectGuardianFees';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useCollectGuardianFees', () => {
  it('should collect one seat out of the pool', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValue({
        claimed_msat: '13000000',
        recorded_claimed_msat: '13000000',
        awaiting_cycle_msat: '3000000',
      });

    const { result } = renderHook(() => useCollectGuardianFees('seat-01'), { wrapper });
    result.current.mutate();

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(adminCall).toHaveBeenCalledWith({ CollectGuardianFees: { seat_id: 'seat-01' } });
  });

  // Both halves of the answer reach the caller. The one this hook exists to
  // carry is awaiting_cycle_msat: without it a collection reads as an emptied
  // account.
  it('should carry both the claimed and the awaiting-cycle figures', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      claimed_msat: '13000000',
      recorded_claimed_msat: '13000000',
      awaiting_cycle_msat: '3000000',
    });

    const { result } = renderHook(() => useCollectGuardianFees('seat-01'), { wrapper });
    result.current.mutate();

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data).toEqual({
      claimed_msat: '13000000',
      recorded_claimed_msat: '13000000',
      awaiting_cycle_msat: '3000000',
    });
  });
});
