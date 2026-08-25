import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { ATTESTATION_LIST_KEY } from '@/features/attestations/api/hooks/use-attestation-list/useAttestationList';
import * as adminCallModule from '@/shared/api/adminCall';
import { useInstallAttestation } from '../useInstallAttestation';

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

it('should convert the file to a byte array, call attestation_install, and invalidate the list key', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ id: 'att-new', kind: 'holder_authorization' });
  const client = makeClient();
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');
  const bytes = new Uint8Array([1, 2, 3, 4]);
  const file = new File([bytes], 'attestation.bin', { type: 'application/octet-stream' });
  Object.defineProperty(file, 'arrayBuffer', {
    value: async () => bytes.buffer
  });

  const { result } = renderHook(() => useInstallAttestation(), { wrapper: wrap(client) });
  result.current.mutate(file);

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith('attestation_install', {
    payload: [1, 2, 3, 4]
  });
  expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ATTESTATION_LIST_KEY });
});
