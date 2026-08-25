import type { GetHealthResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { vi } from 'vitest';
import {
  AccessDeniedError,
  AuthError,
  DaemonUnavailableError,
  NetworkError
} from '@/shared/api/errors';
import { useHealth } from '@/shared/api/hooks/use-health/useHealth';
import { useSetupState } from '@/shared/api/hooks/use-setup-state/useSetupState';
import { useBootStatus } from '../useBootStatus';

// Partial mocks: keep the real HEALTH_KEY/SETUP_STATE_KEY exports (the boot
// hook's cache-removal predicate compares against them via hashKey) and only
// replace the query hooks themselves.
vi.mock('@/shared/api/hooks/use-health/useHealth', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/shared/api/hooks/use-health/useHealth')>()),
  useHealth: vi.fn()
}));
vi.mock('@/shared/api/hooks/use-setup-state/useSetupState', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/shared/api/hooks/use-setup-state/useSetupState')>()),
  useSetupState: vi.fn()
}));

const useHealthMock = vi.mocked(useHealth);
const useSetupStateMock = vi.mocked(useSetupState);

const makeClient = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

const wrap = (client: QueryClient) => {
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return Wrapper;
};

interface QueryStates {
  health?: Partial<ReturnType<typeof useHealth>>;
  setup?: Partial<ReturnType<typeof useSetupState>>;
}

const healthDefaults = {
  data: undefined,
  isError: false,
  isPending: false,
  refetch: vi.fn()
};

const setupDefaults = {
  data: undefined,
  error: null,
  isError: false,
  isPending: false,
  refetch: vi.fn()
};

const healthyHealth: GetHealthResponse = {
  overall_status: 'healthy',
  mode: 'normal',
  observed_at: 1721476800,
  components: [{ component: 'daemon', status: 'healthy', detail: null, observed_at: 1721476800 }]
};

const restoreModeHealth: GetHealthResponse = {
  overall_status: 'warning',
  mode: 'restore',
  observed_at: 1721476800,
  components: [
    {
      component: 'daemon',
      status: 'warning',
      detail: null,
      observed_at: 1721476800
    }
  ]
};

const modeHealth = (mode: GetHealthResponse['mode']): GetHealthResponse => ({
  ...restoreModeHealth,
  mode
});

const arrangeQueries = ({ health, setup }: QueryStates) => {
  useHealthMock.mockReturnValue({ ...healthDefaults, ...health } as ReturnType<typeof useHealth>);
  useSetupStateMock.mockReturnValue({
    ...setupDefaults,
    ...setup
  } as ReturnType<typeof useSetupState>);
};

it('should report daemon-unreachable when the health probe errors before first data', () => {
  arrangeQueries({ health: { isError: true } });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('daemon-unreachable');
});

it('should report daemon-unreachable when setup state fails with a NetworkError', () => {
  arrangeQueries({
    health: { data: healthyHealth },
    setup: { error: new NetworkError(), isError: true }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('daemon-unreachable');
});

// The exact shape of a live restore: /health answers 200 and names the mode,
// while every privileged route refuses because there is no runtime behind it.
// The unreachable gate below matches that setup failure, so without a check
// ahead of it the operator was told the daemon was not answering — on the one
// screen they stare at during a recovery, over a line reading
// "GET /health · connection refused".
it('should report reloading rather than unreachable while a live restore swaps', () => {
  arrangeQueries({
    health: { data: modeHealth('reloading') },
    setup: { error: new DaemonUnavailableError(), isError: true }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('reloading');
});

// Same shape, different cause: the Admin API binds while the first generation
// is still building, so every daemon serves this state on the way up.
it('should report no-runtime rather than unreachable while the daemon starts', () => {
  arrangeQueries({
    health: { data: modeHealth('no_runtime') },
    setup: { error: new DaemonUnavailableError(), isError: true }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('no-runtime');
});

// Restore-only boot keeps its standalone console. The full Admin API comes back
// on its own after a live restore, so an operator mid-swap belongs on a waiting
// screen, not in front of archive controls that would refuse them.
it('should keep restore-only mode on the recovery console', () => {
  arrangeQueries({
    health: { data: restoreModeHealth },
    setup: { error: new DaemonUnavailableError(), isError: true }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('restore-mode');
});

// Health polls while the daemon has no runtime; setup-state polls once a
// minute. Without a nudge on the transition, the instant a restore lands the
// gate would drop from the waiting screen to "can't reach the daemon" and stay
// there for up to a minute, because setup still holds the failure it took while
// the runtime was gone.
it('should ask setup-state again the moment the daemon starts serving', () => {
  const refetch = vi.fn();
  arrangeQueries({
    health: { data: modeHealth('reloading') },
    setup: { error: new DaemonUnavailableError(), isError: true, refetch }
  });

  const { result, rerender } = renderHook(() => useBootStatus(), {
    wrapper: wrap(makeClient())
  });
  expect(result.current.status).toBe('reloading');
  refetch.mockClear();

  arrangeQueries({
    health: { data: healthyHealth },
    setup: { error: new DaemonUnavailableError(), isError: true, refetch }
  });
  rerender();

  expect(refetch).toHaveBeenCalledTimes(1);
});

// A daemon that really cannot be reached still reports as such: health has no
// data and errored, so there is no mode to read.
it('should still report unreachable when health itself does not answer', () => {
  arrangeQueries({
    health: { isError: true },
    setup: { error: new DaemonUnavailableError(), isError: true }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('daemon-unreachable');
});

it('should report needs-auth when setup state fails with an AuthError', () => {
  arrangeQueries({
    health: { data: healthyHealth },
    setup: { error: new AuthError(), isError: true }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('needs-auth');
});

it('should report needs-auth when setup state fails with an AuthError even though cached data exists', () => {
  // Cache-independent re-auth: a 401 on a refetch always routes to the gate,
  // never masked by previously-loaded setup data.
  arrangeQueries({
    health: { data: healthyHealth },
    setup: {
      data: { status: 'ready' } as ReturnType<typeof useSetupState>['data'],
      error: new AuthError(),
      isError: true
    }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('needs-auth');
});

it('should report access-denied when setup state fails with an AccessDeniedError', () => {
  arrangeQueries({
    health: { data: healthyHealth },
    setup: { error: new AccessDeniedError(), isError: true }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('access-denied');
});

it('should report access-denied when setup state fails with an AccessDeniedError even though cached data exists', () => {
  arrangeQueries({
    health: { data: healthyHealth },
    setup: {
      data: { status: 'ready' } as ReturnType<typeof useSetupState>['data'],
      error: new AccessDeniedError(),
      isError: true
    }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('access-denied');
});

it('should keep reporting needs-auth across a render cycle without refetching', async () => {
  // Regression guard for the cache-clear-triggers-refetch-loop failure mode:
  // clearing the boot query itself would flip status away from needs-auth and
  // provoke another fetch. Asserting the mocked setup.refetch is never called
  // by the hook itself (only onRetry may call it) proves no such loop exists.
  const refetchSetup = vi.fn();
  arrangeQueries({
    health: { data: healthyHealth },
    setup: {
      data: { status: 'ready' } as ReturnType<typeof useSetupState>['data'],
      error: new AuthError(),
      isError: true,
      refetch: refetchSetup
    }
  });
  const client = makeClient();

  const { result, rerender } = renderHook(() => useBootStatus(), { wrapper: wrap(client) });
  rerender();
  rerender();

  expect(result.current.status).toBe('needs-auth');
  expect(refetchSetup).not.toHaveBeenCalled();
});

it('should remove cached privileged query data (but not the boot queries) once gated by an AuthError', async () => {
  const client = makeClient();
  const FUNDS_KEY = ['funds'] as const;
  client.setQueryData(FUNDS_KEY, { balance_sats: 1 });
  client.setQueryData(['setup-state'], { status: 'ready' });
  client.setQueryData(['health'], healthyHealth);

  arrangeQueries({
    health: { data: healthyHealth },
    setup: {
      data: { status: 'ready' } as ReturnType<typeof useSetupState>['data'],
      error: new AuthError(),
      isError: true
    }
  });

  renderHook(() => useBootStatus(), { wrapper: wrap(client) });

  await waitFor(() => {
    expect(client.getQueryData(FUNDS_KEY)).toBeUndefined();
  });
  expect(client.getQueryData(['setup-state'])).toEqual({ status: 'ready' });
  expect(client.getQueryData(['health'])).toEqual(healthyHealth);
});

it('should invalidate every query once a successful re-login clears a prior AuthError', () => {
  const client = makeClient();
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');

  arrangeQueries({
    health: { data: healthyHealth },
    setup: { error: new AuthError(), isError: true }
  });
  const { rerender } = renderHook(() => useBootStatus(), { wrapper: wrap(client) });

  invalidateSpy.mockClear(); // ignore any calls from the initial gated render

  arrangeQueries({
    health: { data: healthyHealth },
    setup: {
      data: { status: 'ready' } as ReturnType<typeof useSetupState>['data'],
      isSuccess: true
    }
  });
  rerender();

  expect(invalidateSpy).toHaveBeenCalledTimes(1);
  expect(invalidateSpy).toHaveBeenCalledWith();
});

it('should not invalidate queries when the boot status was never gated', () => {
  const client = makeClient();
  const invalidateSpy = vi.spyOn(client, 'invalidateQueries');

  arrangeQueries({
    health: { data: healthyHealth },
    setup: {
      data: { status: 'ready' } as ReturnType<typeof useSetupState>['data'],
      isSuccess: true
    }
  });
  const { rerender } = renderHook(() => useBootStatus(), { wrapper: wrap(client) });
  rerender();

  expect(invalidateSpy).not.toHaveBeenCalled();
});

it('should report booting while the health probe is still pending', () => {
  arrangeQueries({ health: { isPending: true } });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('booting');
});

it('should report booting while setup state is still pending', () => {
  arrangeQueries({
    health: { data: healthyHealth },
    setup: { isPending: true }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('booting');
});

it('should report restore-mode when health reports restore mode', () => {
  arrangeQueries({
    health: { data: restoreModeHealth },
    setup: { data: { status: 'ready' } as ReturnType<typeof useSetupState>['data'] }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('restore-mode');
});

it('should report restore-mode ahead of needs-auth when no token has been entered yet', () => {
  // The real scenario this guards: on first boot into a restore-mode daemon,
  // no token has been entered, so a get_setup_state call fails auth-shaped —
  // but the operator should land on the recovery console, not the normal
  // login prompt, since restore-mode is knowable from the unauthenticated
  // health probe alone, which reports it as a typed mode.
  arrangeQueries({
    health: { data: restoreModeHealth },
    setup: { error: new AuthError(), isError: true }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('restore-mode');
});

it('should report ready when health reports normal mode', () => {
  arrangeQueries({
    health: { data: healthyHealth },
    setup: { data: { status: 'ready' } as ReturnType<typeof useSetupState>['data'] }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('ready');
});

it('should report ready when both queries have data', () => {
  arrangeQueries({
    health: { data: healthyHealth },
    setup: { data: { status: 'ready' } as ReturnType<typeof useSetupState>['data'] }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('ready');
});

it('should stay ready when a later poll fails after setup data has loaded', () => {
  arrangeQueries({
    health: { data: healthyHealth },
    setup: {
      data: { status: 'ready' } as ReturnType<typeof useSetupState>['data'],
      error: new NetworkError(),
      isError: true
    }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });

  expect(result.current.status).toBe('ready');
});

it('should refetch both queries on retry', () => {
  const refetchHealth = vi.fn();
  const refetchSetup = vi.fn();
  arrangeQueries({
    health: { isError: true, refetch: refetchHealth },
    setup: { refetch: refetchSetup }
  });

  const { result } = renderHook(() => useBootStatus(), { wrapper: wrap(makeClient()) });
  result.current.onRetry();

  expect(refetchHealth).toHaveBeenCalled();
  expect(refetchSetup).toHaveBeenCalled();
});
