import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { vi } from 'vitest';
import { MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import {
  AccessDeniedError,
  AdminApiError,
  AuthError,
  HttpStatusError,
  NetworkError,
  ProtocolError
} from '@/shared/api/errors';
import { ONBOARDING_KEY, useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { useBootStatus } from '../useBootStatus';

// Partial mock: keep the real ONBOARDING_KEY export (the boot hook's cache-removal
// predicate compares against it via hashKey) and only replace the query hook itself.
vi.mock('@/shared/api/hooks/use-onboarding/useOnboarding', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/shared/api/hooks/use-onboarding/useOnboarding')>()),
  useOnboarding: vi.fn()
}));

const useOnboardingMock = vi.mocked(useOnboarding);

const makeClient = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

const wrap = (client: QueryClient) => {
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return Wrapper;
};

const onboardingDefaults = {
  data: undefined,
  error: null,
  isError: false,
  isPending: false,
  isSuccess: false,
  refetch: vi.fn()
};

const arrangeQuery = (overrides: Partial<ReturnType<typeof useOnboarding>>) => {
  useOnboardingMock.mockReturnValue({
    ...onboardingDefaults,
    ...overrides
  } as ReturnType<typeof useOnboarding>);
};

const renderBootStatus = (client: QueryClient = makeClient()) =>
  renderHook(() => useBootStatus(), { wrapper: wrap(client) });

const onboardedData = {
  stage: 'complete' as const,
  runtime: 'ready' as const,
  fman_name: 'blissful-chiffchaff',
  service_pubkey: 'abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'not_observed' as const, checked_at: 1_760_000_000 },
  fman_version: { current: '0.1.0', latest: '0.1.0', update_required: false }
};

it('should report booting while onboarding is still pending', () => {
  arrangeQuery({ isPending: true });

  const { result } = renderBootStatus();

  expect(result.current.status).toBe('booting');
});

it('should report needs-auth when onboarding fails with an AuthError', () => {
  arrangeQuery({ error: new AuthError(), isError: true });

  const { result } = renderBootStatus();

  expect(result.current.status).toBe('needs-auth');
});

it('should report daemon-unreachable when onboarding fails with a NetworkError', () => {
  arrangeQuery({ error: new NetworkError(), isError: true });

  const { result } = renderBootStatus();

  expect(result.current.status).toBe('daemon-unreachable');
});

it('should report daemon-unreachable when the daemon answers with a server error', () => {
  arrangeQuery({ error: new HttpStatusError(502), isError: true });

  const { result } = renderBootStatus();

  expect(result.current.status).toBe('daemon-unreachable');
});

it('should report daemon-unreachable when the answer is not an admin result', () => {
  arrangeQuery({ error: new ProtocolError(), isError: true });

  const { result } = renderBootStatus();

  expect(result.current.status).toBe('daemon-unreachable');
});

// A 403 is neither of its neighbours and is not `ready` either: the daemon
// answered, so it is not unreachable; the session was accepted, so a sign-in
// prompt would ask for the wrong thing; and the call the dashboard is built on
// was refused, so mounting the routed tree would show a shell of failing panels.
it('should report access-denied for a 403', () => {
  arrangeQuery({ error: new AccessDeniedError(), isError: true });

  const { result } = renderBootStatus();

  expect(result.current.status).toBe('access-denied');
});

it('should report access-denied for a 403 on a refetch even though cached data exists', () => {
  arrangeQuery({ data: onboardedData, error: new AccessDeniedError(), isError: true });

  const { result } = renderBootStatus();

  expect(result.current.status).toBe('access-denied');
});

it('should remove cached privileged query data once gated by a 403', async () => {
  const client = makeClient();
  const SEATS_KEY = ['seats'] as const;
  client.setQueryData(SEATS_KEY, { seats: [] });
  client.setQueryData(ONBOARDING_KEY, onboardedData);

  arrangeQuery({ data: onboardedData, error: new AccessDeniedError(), isError: true });

  renderBootStatus(client);

  await waitFor(() => {
    expect(client.getQueryData(SEATS_KEY)).toBeUndefined();
  });
  expect(client.getQueryData(ONBOARDING_KEY)).toEqual(onboardedData);
});

it('should carry the observed failure so the screen can state it', () => {
  const failure = new HttpStatusError(503);
  arrangeQuery({ error: failure, isError: true });

  const { result } = renderBootStatus();

  expect(result.current.failure).toBe(failure);
});

it('should report ready when onboarding has data', () => {
  arrangeQuery({ data: onboardedData });

  const { result } = renderBootStatus();

  expect(result.current.status).toBe('ready');
});

it('should stay ready when a later poll fails after data has loaded', () => {
  arrangeQuery({
    data: onboardedData,
    error: new NetworkError(),
    isError: true
  });

  const { result } = renderBootStatus();

  expect(result.current.status).toBe('ready');
});

it('should report needs-auth when a refetch fails with an AuthError even though cached data exists', () => {
  // Cache-independent re-auth: a 401 on a refetch always routes to the gate,
  // never masked by previously-loaded onboarding data.
  arrangeQuery({
    data: onboardedData,
    error: new AuthError(),
    isError: true
  });

  const { result } = renderBootStatus();

  expect(result.current.status).toBe('needs-auth');
});

it('should keep reporting needs-auth across a render cycle without refetching', () => {
  // Regression guard for the cache-clear-triggers-refetch-loop failure mode:
  // clearing the boot query itself would flip status away from needs-auth and
  // provoke another fetch. Asserting the mocked refetch is never called by the
  // hook itself (only onRetry may call it) proves no such loop exists.
  const refetch = vi.fn();
  arrangeQuery({
    data: onboardedData,
    error: new AuthError(),
    isError: true,
    refetch
  });
  const { result, rerender } = renderBootStatus();
  rerender();
  rerender();

  expect(result.current.status).toBe('needs-auth');
  expect(refetch).not.toHaveBeenCalled();
});

it('should remove cached privileged query data (but not the onboarding query) once gated by an AuthError', async () => {
  const client = makeClient();
  const SEATS_KEY = ['seats'] as const;
  client.setQueryData(SEATS_KEY, { seats: [] });
  client.setQueryData(ONBOARDING_KEY, onboardedData);

  arrangeQuery({
    data: onboardedData,
    error: new AuthError(),
    isError: true
  });

  renderBootStatus(client);

  await waitFor(() => {
    expect(client.getQueryData(SEATS_KEY)).toBeUndefined();
  });
  expect(client.getQueryData(ONBOARDING_KEY)).toEqual(onboardedData);
});

it('should invalidate every query once a successful re-login clears a prior AuthError', () => {
  const client = makeClient();
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');

  arrangeQuery({ error: new AuthError(), isError: true });
  const { rerender } = renderBootStatus(client);

  invalidateSpy.mockClear(); // ignore any calls from the initial gated render

  arrangeQuery({ data: onboardedData, isSuccess: true });
  rerender();

  expect(invalidateSpy).toHaveBeenCalledTimes(1);
  expect(invalidateSpy).toHaveBeenCalledWith();
});

it('should not invalidate queries when the boot status was never gated', () => {
  const client = makeClient();
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');

  arrangeQuery({ data: onboardedData, isSuccess: true });
  const { rerender } = renderBootStatus(client);
  rerender();

  expect(invalidateSpy).not.toHaveBeenCalled();
});

it('should refetch onboarding on retry', () => {
  const refetch = vi.fn();
  arrangeQuery({ error: new NetworkError(), isError: true, refetch });

  const { result } = renderBootStatus();
  result.current.onRetry();

  expect(refetch).toHaveBeenCalled();
});

// Regression: deriving `booting` from the query being in flight unmounted the
// whole tree whenever the onboarding query returned to pending. The components
// this gate guards observe that same query, so unmounting them provoked the next
// fetch and the gate flapped until the browser's connection pool jammed.
it('should stay out of booting once the daemon has answered, even if the query returns to pending', () => {
  arrangeQuery({
    error: new AdminApiError('this Fleet Manager has not been onboarded yet', 'not_onboarded'),
    isError: true
  });

  const { result, rerender } = renderBootStatus();
  expect(result.current.status).toBe('ready');

  arrangeQuery({ isPending: true });
  rerender();

  expect(result.current.status).toBe('ready');
});
