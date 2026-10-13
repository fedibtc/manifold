import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { usePayoutDestination } from '../usePayoutDestination';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('usePayoutDestination', () => {
  it('should read the destination with the bare unit verb', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValue({ destination: 'operator@example.com' });

    const { result } = renderHook(() => usePayoutDestination(), { wrapper });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(adminCall).toHaveBeenCalledWith('PayoutDestination');
  });

  // Null is the daemon's answer for "none configured", not a failure to read.
  it('should carry a null destination through as an answer', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ destination: null });

    const { result } = renderHook(() => usePayoutDestination(), { wrapper });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data).toEqual({ destination: null });
  });
});
