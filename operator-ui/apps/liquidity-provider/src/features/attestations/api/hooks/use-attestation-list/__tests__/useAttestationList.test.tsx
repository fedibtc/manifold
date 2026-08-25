import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AuthError } from '@/shared/api/errors';
import { useAttestationList } from '../useAttestationList';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should call attestation_list and return payloads on success', async () => {
  const response = {
    payloads: [
      {
        id: 'att-1',
        kind: 'holder_authorization' as const,
        subject: { holder: '02aa'.padEnd(66, '0') },
        ingested_at: 1784634480, // 2026-07-21T11:48:00Z
        valid: true
      }
    ]
  };
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(response);

  const { result } = renderHook(() => useAttestationList(), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(result.current.data).toEqual(response);
  expect(adminCallSpy).toHaveBeenCalledWith('attestation_list', null);
});

it('should surface an AuthError immediately without retrying', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AuthError());

  const { result } = renderHook(() => useAttestationList(), { wrapper });

  await waitFor(() => expect(result.current.isError).toBe(true));
  expect(result.current.error).toBeInstanceOf(AuthError);
  expect(adminCallSpy).toHaveBeenCalledTimes(1);
});
