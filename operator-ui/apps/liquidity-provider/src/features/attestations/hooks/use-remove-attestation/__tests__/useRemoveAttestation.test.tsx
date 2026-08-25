import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { ATTESTATION_LIST_KEY } from '@/features/attestations/api/hooks/use-attestation-list/useAttestationList';
import * as adminCallModule from '@/shared/api/adminCall';
import { useRemoveAttestation } from '../useRemoveAttestation';

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

it('should call attestation_remove with the target and invalidate the list key', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({});
  const client = makeClient();
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');

  const { result } = renderHook(() => useRemoveAttestation(), { wrapper: wrap(client) });
  result.current.mutate({ target: { id: 'att-1' } });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith('attestation_remove', { target: { id: 'att-1' } });
  expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ATTESTATION_LIST_KEY });
});
