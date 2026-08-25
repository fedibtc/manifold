import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { useOnboardFromBackup } from '../useOnboardFromBackup';

const wrapper = (client: QueryClient) => {
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return Wrapper;
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useOnboardFromBackup', () => {
  it('should keep no mutation variables after settlement', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      onboarded: 'restored',
      seats: 1,
      formed: 1
    });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { result } = renderHook(() => useOnboardFromBackup(), { wrapper: wrapper(client) });

    result.current.mutate({ mnemonic: 'twelve words here', acknowledgeOriginalHostIsGone: true });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    result.current.reset();

    // gcTime: 0 means TanStack Query drops the mutation — and the phrase it carried
    // in `variables` — as soon as it is no longer observed.
    await waitFor(() => expect(client.getMutationCache().getAll()).toHaveLength(0));
  });

  it('should replace stale onboarding data with a fresh response', async () => {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) =>
      request === 'Onboarding'
        ? Promise.resolve({
            fman_name: 'fresh',
            service_pubkey: '02abc',
            service_nostr_pubkey: 'b'.repeat(64),
            nostr: { state: 'not_observed', checked_at: 1_760_000_000 }
          })
        : Promise.resolve({ onboarded: 'restored', seats: 1, formed: 1 })
    );

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(['onboarding'], {
      fman_name: 'stale',
      service_pubkey: '02abc',
      service_nostr_pubkey: 'a'.repeat(64),
      nostr: {
        state: 'authorization_observed',
        authorizations: [],
        holders: [],
        checked_at: 1_760_000_000
      }
    });

    const { result } = renderHook(() => useOnboardFromBackup(), { wrapper: wrapper(client) });
    result.current.mutate({ mnemonic: 'twelve words here', acknowledgeOriginalHostIsGone: true });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));

    expect(adminCall).toHaveBeenCalledWith('Onboarding');
    expect(client.getQueryData(['onboarding'])).toMatchObject({ fman_name: 'fresh' });
  });

  it('should not settle on an onboarding request that predates the restore', async () => {
    // react-query answers a fetch with whatever request is already in flight for
    // the key. Onboarding polls throughout setup, so a cached fetch here would
    // routinely resolve with a reading of the identity this host just replaced.
    const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) =>
      request === 'Onboarding'
        ? Promise.resolve({
            fman_name: 'fresh',
            service_pubkey: '02abc',
            service_nostr_pubkey: 'b'.repeat(64),
            nostr: { state: 'not_observed', checked_at: 1_760_000_000 }
          })
        : Promise.resolve({ onboarded: 'restored', seats: 1, formed: 1 })
    );

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    // A poll issued before the restore, still waiting for its answer.
    const stalled = new Promise(() => {});
    client.fetchQuery({ queryKey: ['onboarding'], queryFn: () => stalled });

    const { result } = renderHook(() => useOnboardFromBackup(), { wrapper: wrapper(client) });
    result.current.mutate({ mnemonic: 'twelve words here', acknowledgeOriginalHostIsGone: true });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));

    expect(adminCall).toHaveBeenCalledWith('Onboarding');
    expect(client.getQueryData(['onboarding'])).toMatchObject({ fman_name: 'fresh' });
  });
});
