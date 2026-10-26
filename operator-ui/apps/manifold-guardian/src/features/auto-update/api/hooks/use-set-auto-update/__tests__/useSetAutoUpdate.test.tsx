import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AUTO_UPDATE_KEY } from '@/features/auto-update/api/hooks/use-auto-update/useAutoUpdate';
import * as adminCallModule from '@/shared/api/adminCall';
import { useSetAutoUpdate } from '../useSetAutoUpdate';

const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={client}>{children}</QueryClientProvider>
);

afterEach(() => {
  vi.restoreAllMocks();
  client.clear();
});

describe('useSetAutoUpdate', () => {
  it('should send the choice and seed the cache from the stored answer', async () => {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ enabled: false });
    client.setQueryData(AUTO_UPDATE_KEY, { enabled: true });

    const { result } = renderHook(() => useSetAutoUpdate(), { wrapper });
    result.current.mutate(false);

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(adminCall).toHaveBeenCalledWith({ SetAutoUpdate: { enabled: false } });
    expect(client.getQueryData(AUTO_UPDATE_KEY)).toEqual({ enabled: false });
  });
});
