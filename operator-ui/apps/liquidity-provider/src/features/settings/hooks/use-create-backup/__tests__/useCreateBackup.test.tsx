import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { useCreateBackup } from '../useCreateBackup';

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

it('should call create_backup with a null body and return the archive and manifest', async () => {
  const response = {
    archive: 'opaque-archive-contents',
    manifest: {
      version: 3,
      created_at: 1721476800,
      state_groups: ['provider_identity', 'database'],
      recovery_point: { quiesced_at: 1721476790, stores: ['sqlite', 'data_directory'] }
    }
  };
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(response);

  const { result } = renderHook(() => useCreateBackup(), { wrapper: wrap(makeClient()) });
  result.current.mutate();

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith('create_backup', null);
  expect(result.current.data).toEqual(response);
});
