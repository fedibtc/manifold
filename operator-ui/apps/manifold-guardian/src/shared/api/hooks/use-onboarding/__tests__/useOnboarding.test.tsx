import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, vi } from 'vitest';
import * as adminCallModule from '../../../adminCall';
import { AuthError, NetworkError } from '../../../errors';
import { useOnboarding } from '../useOnboarding';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

// Midpoint jitter, drawn before any poller draws its own offset, so every
// interval below is exactly its nominal value.
beforeEach(() => {
  vi.spyOn(Math, 'random').mockReturnValue(0.5);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('should call adminCall with Onboarding and return the response on success', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    fman_name: 'blissful-chiffchaff',
    service_pubkey: 'abc',
    nostr: { state: 'disabled' }
  });

  const { result } = renderHook(() => useOnboarding(), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(result.current.data).toEqual({
    fman_name: 'blissful-chiffchaff',
    service_pubkey: 'abc',
    nostr: { state: 'disabled' }
  });
  expect(adminCallSpy).toHaveBeenCalledWith('Onboarding');
});

it('should retry promptly on the first failure, then grow the gap', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new NetworkError());

  renderHook(() => useOnboarding(), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));

  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);

  // The second failure is not retried 5s later like the first: the cadence has
  // doubled, so this window passes with no call at all.
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);

  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(3);
});

it('should surface an AuthError immediately without retrying', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AuthError());

  const { result } = renderHook(() => useOnboarding(), { wrapper });

  await waitFor(() => expect(result.current.isError).toBe(true));
  expect(result.current.error).toBeInstanceOf(AuthError);
  expect(adminCallSpy).toHaveBeenCalledTimes(1);
});
