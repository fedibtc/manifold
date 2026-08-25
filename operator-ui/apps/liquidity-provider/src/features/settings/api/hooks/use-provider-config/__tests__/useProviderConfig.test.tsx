import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { PROVIDER_CONFIG_KEY, useProviderConfig } from '../useProviderConfig';

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

it('should call get_provider_config with a null body and expose the response under config', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ config: { network: 'signet' } });
  const client = makeClient();

  const { result } = renderHook(() => useProviderConfig(), { wrapper: wrap(client) });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith('get_provider_config', null);
  expect(result.current.data).toEqual({ config: { network: 'signet' } });
});

it('should expose PROVIDER_CONFIG_KEY as its query key', () => {
  expect(PROVIDER_CONFIG_KEY).toEqual(['provider-config']);
});

it('should use retry:false and a staleTime', () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ config: { network: 'signet' } });
  const client = makeClient();

  renderHook(() => useProviderConfig(), { wrapper: wrap(client) });

  const query = client.getQueryCache().find({ queryKey: PROVIDER_CONFIG_KEY });
  const options = query?.options as { retry?: boolean; staleTime?: number } | undefined;
  expect(options?.retry).toBe(false);
  expect(options?.staleTime).toBe(55_000);
});
