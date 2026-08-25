import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';
import { useInspectBackup } from '../useInspectBackup';

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

it('should call inspect_backup with the archive and return the manifest', async () => {
  const response = {
    manifest: {
      version: 3,
      created_at: 1721476800,
      state_groups: ['database'],
      recovery_point: { quiesced_at: 1721476790, stores: ['sqlite', 'data_directory'] }
    }
  };
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(response);

  const { result } = renderHook(() => useInspectBackup(), { wrapper: wrap(makeClient()) });
  result.current.mutate({ archive: 'archive-contents' });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith('inspect_backup', { archive: 'archive-contents' });
  expect(result.current.data).toEqual(response);
});

it('should surface an AdminApiError when the archive is malformed', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(
    new AdminApiError('invalid_argument', 'archive is not valid')
  );

  const { result } = renderHook(() => useInspectBackup(), { wrapper: wrap(makeClient()) });
  result.current.mutate({ archive: 'garbage' });

  await waitFor(() => expect(result.current.isError).toBe(true));
  expect(result.current.error).toBeInstanceOf(AdminApiError);
});
