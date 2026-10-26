import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { useAutoUpdate } from '../useAutoUpdate';

const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={client}>{children}</QueryClientProvider>
);

afterEach(() => {
  vi.restoreAllMocks();
  client.clear();
});

describe('useAutoUpdate', () => {
  it('should read the stored choice with ShowAutoUpdate', async () => {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ enabled: false });

    const { result } = renderHook(() => useAutoUpdate(), { wrapper });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(adminCall).toHaveBeenCalledWith('ShowAutoUpdate');
    expect(result.current.data).toEqual({ enabled: false });
  });
});
