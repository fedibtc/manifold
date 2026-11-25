import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { PAYOUT_DESTINATION_KEY } from '@/features/payouts/api/hooks/use-payout-destination/usePayoutDestination';
import { useSetPayoutDestination } from '@/features/payouts/api/hooks/use-set-payout-destination/useSetPayoutDestination';
import * as adminCallModule from '@/shared/api/adminCall';
import { useSweepPaymentFees } from '../useSweepPaymentFees';

const wrapperFor =
  (client: QueryClient) =>
  ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );

// The Payouts page shows a sweep only once the destination read has answered.
const clientWithDestination = (destination: string | null) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(PAYOUT_DESTINATION_KEY, { destination });
  return client;
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useSweepPaymentFees', () => {
  const job = {
    request_id: 'request-1',
    scope: { kind: 'payment_federation' as const, federation_id: 'fed1aaa' },
    method: 'lightning' as const,
    destination: 'operator@example.com',
    operation: {
      operation_id: 'op-1',
      amount_msat: 250_000_000,
      capped: null,
      committed_at_ms: 2,
      confirmed_at_ms: null
    },
    created_at_ms: 1
  };
  // The request carries the federation and a retry-stable request id: no amount, because a
  // sweep takes the largest economically fundable amount, and no gateway,
  // because the daemon selects one.
  it('should ask for the federation alone', async () => {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(job);
    const wrapper = wrapperFor(clientWithDestination('operator@example.com'));

    const { result } = renderHook(() => useSweepPaymentFees('fed1aaa', 'lightning'), { wrapper });
    result.current.mutate();

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(adminCall).toHaveBeenCalledWith({
      SweepPaymentFees: {
        federation_id: 'fed1aaa',
        request_id: expect.any(String),
        method: 'lightning'
      }
    });
  });

  it('should answer with the settled operation and amount', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(job);
    const wrapper = wrapperFor(clientWithDestination('operator@example.com'));

    const { result } = renderHook(() => useSweepPaymentFees('fed1aaa', 'lightning'), { wrapper });
    result.current.mutate();

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data).toEqual(job);
  });

  it('should retain the id after failure and rotate it after success', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new Error('lost response'))
      .mockResolvedValue(job);
    const wrapper = wrapperFor(clientWithDestination('operator@example.com'));
    const { result } = renderHook(() => useSweepPaymentFees('fed1aaa', 'lightning'), { wrapper });

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

  // The daemon keeps a request id's first destination (SPEC-admin-socket), so a
  // retry under the old id after a new destination is saved would go to the old one.
  it('should use a new id for a retry after the operator saves another destination', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new Error('lnurl lookup failed'))
      .mockResolvedValueOnce({ destination: 'fixed@example.com' })
      .mockResolvedValueOnce(job);
    const wrapper = wrapperFor(clientWithDestination('typo@exmple.com'));
    const { result } = renderHook(
      () => ({
        sweep: useSweepPaymentFees('fed1aaa', 'lightning'),
        save: useSetPayoutDestination()
      }),
      { wrapper }
    );

    await expect(result.current.sweep.mutateAsync()).rejects.toThrow('lnurl lookup failed');
    await act(async () => {
      await result.current.save.mutateAsync('fixed@example.com');
    });
    await act(async () => {
      await result.current.sweep.mutateAsync();
    });

    const [[first], , [retry]] = adminCall.mock.calls;
    expect(retry).toEqual({
      SweepPaymentFees: {
        federation_id: 'fed1aaa',
        request_id: expect.any(String),
        method: 'lightning'
      }
    });
    expect(retry).not.toEqual(first);
  });

  // An unread destination is not a change: reading it later must not turn a retry
  // after a lost response into a second payout.
  it('should keep the id when the first try went out before the destination was read', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new Error('lost response'))
      .mockResolvedValue(job);
    const client = new QueryClient();
    const { result } = renderHook(() => useSweepPaymentFees('fed1aaa', 'lightning'), {
      wrapper: wrapperFor(client)
    });

    await expect(result.current.mutateAsync()).rejects.toThrow('lost response');
    client.setQueryData(PAYOUT_DESTINATION_KEY, { destination: 'operator@example.com' });
    await act(async () => {
      await result.current.mutateAsync();
    });

    const [[first], [retry]] = adminCall.mock.calls;
    expect(retry).toEqual(first);
  });

  // An ecash payout needs no destination, and a lost response is retried under
  // the same id: a new one would spend whatever came in since.
  it('should ask for ecash without a destination and retry it under the same id', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new Error('lost response'))
      .mockResolvedValue({ ...job, method: 'ecash' });
    const wrapper = wrapperFor(clientWithDestination(null));
    const { result } = renderHook(() => useSweepPaymentFees('fed1aaa', 'ecash'), { wrapper });

    await expect(result.current.mutateAsync()).rejects.toThrow('lost response');
    await act(async () => {
      await result.current.mutateAsync();
    });

    const [[first], [retry]] = adminCall.mock.calls;
    expect(first).toEqual({
      SweepPaymentFees: {
        federation_id: 'fed1aaa',
        request_id: expect.any(String),
        method: 'ecash'
      }
    });
    expect(retry).toEqual(first);
  });

  // The daemon binds an id to its first method, so a retry after switching to
  // the other method would be refused under the old id.
  it('should use a new id once the operator switches method after a failure', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new Error('no gateway'))
      .mockResolvedValue({ ...job, method: 'ecash' });
    const wrapper = wrapperFor(clientWithDestination('operator@example.com'));
    const { result, rerender } = renderHook(
      ({ method }: { method: 'lightning' | 'ecash' }) => useSweepPaymentFees('fed1aaa', method),
      { wrapper, initialProps: { method: 'lightning' as 'lightning' | 'ecash' } }
    );

    await expect(result.current.mutateAsync()).rejects.toThrow('no gateway');
    rerender({ method: 'ecash' });
    await act(async () => {
      await result.current.mutateAsync();
    });

    const ids = adminCall.mock.calls.map(([request]) => {
      if (typeof request !== 'object' || !('SweepPaymentFees' in request)) {
        throw new Error('expected SweepPaymentFees request');
      }
      return request.SweepPaymentFees.request_id;
    });
    expect(ids[1]).not.toBe(ids[0]);
  });
});
