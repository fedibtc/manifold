import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { useOnboardAsNew } from '../useOnboardAsNew';

const wrapper = (client: QueryClient) => {
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return Wrapper;
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useOnboardAsNew', () => {
  it('should replace stale onboarding data with a fresh response', async () => {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) =>
      request === 'Onboarding'
        ? Promise.resolve({
            fman_name: 'fresh',
            service_pubkey: '02abc',
            service_nostr_pubkey: 'b'.repeat(64),
            nostr: { state: 'not_observed', checked_at: 1_760_000_000 }
          })
        : Promise.resolve({ onboarded: 'new', seats: 0 })
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

    const { result } = renderHook(() => useOnboardAsNew(), { wrapper: wrapper(client) });
    result.current.mutate();

    await waitFor(() => expect(result.current.isSuccess).toBe(true));

    expect(adminCall).toHaveBeenCalledWith('Onboarding');
    expect(client.getQueryData(['onboarding'])).toMatchObject({ fman_name: 'fresh' });
  });
});
