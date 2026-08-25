import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, vi } from 'vitest';
import * as adminCallModule from '../../../adminCall';
import { AuthError } from '../../../errors';
import { useSetupState } from '../useSetupState';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should call get_setup_state and return the setup state on success', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ status: 'ready' });

  const { result } = renderHook(() => useSetupState(), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(result.current.data).toEqual({ status: 'ready' });
  expect(adminCallSpy).toHaveBeenCalledWith('get_setup_state', null);
});

it('should surface an AuthError immediately without retrying', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AuthError());

  const { result } = renderHook(() => useSetupState(), { wrapper });

  await waitFor(() => expect(result.current.isError).toBe(true));
  expect(result.current.error).toBeInstanceOf(AuthError);
  expect(adminCallSpy).toHaveBeenCalledTimes(1);
});
