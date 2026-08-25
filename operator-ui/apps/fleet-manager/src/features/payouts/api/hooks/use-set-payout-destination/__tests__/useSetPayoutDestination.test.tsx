import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { PAYOUT_DESTINATION_KEY } from '@/features/payouts/api/hooks/use-payout-destination/usePayoutDestination';
import * as adminCallModule from '@/shared/api/adminCall';
import { useSetPayoutDestination } from '../useSetPayoutDestination';

const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={client}>{children}</QueryClientProvider>
);

afterEach(() => {
  vi.restoreAllMocks();
  client.clear();
});

describe('useSetPayoutDestination', () => {
  it('should store the destination the operator entered', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValue({ destination: 'operator@example.com' });

    const { result } = renderHook(() => useSetPayoutDestination(), { wrapper });
    result.current.mutate('operator@example.com');

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(adminCall).toHaveBeenCalledWith({
      SetPayoutDestination: { destination: 'operator@example.com' }
    });
  });

  it('should clear the destination with an explicit null', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValue({ destination: null });

    const { result } = renderHook(() => useSetPayoutDestination(), { wrapper });
    result.current.mutate(null);

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(adminCall).toHaveBeenCalledWith({ SetPayoutDestination: { destination: null } });
  });

  // The write answers with the stored view, so seeding the cache from it is
  // correct and a follow-up read would only add a blank frame.
  it('should seed the destination cache from the write it just made', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      destination: 'operator@example.com'
    });

    const { result } = renderHook(() => useSetPayoutDestination(), { wrapper });
    result.current.mutate('operator@example.com');

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(client.getQueryData(PAYOUT_DESTINATION_KEY)).toEqual({
      destination: 'operator@example.com'
    });
  });
});
