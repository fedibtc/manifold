import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { PROVIDER_CONFIG_KEY } from '@/features/settings/api/hooks/use-provider-config/useProviderConfig';
import * as adminCallModule from '@/shared/api/adminCall';
import { useUpdateProviderConfig } from '../useUpdateProviderConfig';

const makeClient = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

const wrap = (client: QueryClient) => {
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return Wrapper;
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should call update_provider_config with the patch and invalidate PROVIDER_CONFIG_KEY', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    config: { network: 'signet' },
    validation: { status: 'passed', checks: [] }
  });
  const client = makeClient();
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');

  const { result } = renderHook(() => useUpdateProviderConfig(), { wrapper: wrap(client) });
  const patch = { replenishment: { warning_threshold: 1, critical_threshold: 0 } };
  result.current.mutate(patch);

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith('update_provider_config', { patch });
  expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: PROVIDER_CONFIG_KEY });
});
