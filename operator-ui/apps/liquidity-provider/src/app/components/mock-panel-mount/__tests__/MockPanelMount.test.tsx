import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, vi } from 'vitest';

afterEach(() => {
  vi.unstubAllEnvs();
});

// MockPanelMount decides whether mocks are on at *module* scope, not per
// render — that is what lets a production build fold the branch away and
// keep `@/mocks/store` out of the bundle entirely (see the comment in
// MockPanelMount.tsx). Stubbing the env only affects code that reads it
// after the stub, so this test resets the module registry and re-imports
// the component so its module body re-evaluates against the stubbed value.
//
// Both tests below reset modules and re-import `MockPanelMount` (and, for
// the second, `mockStore`) from scratch rather than holding a top-level
// reference: a `vi.resetModules()` call clears the registry for *future*
// dynamic imports without touching bindings a prior test already resolved,
// so a shared top-level `mockStore` import would silently diverge from the
// fresh instance the component's effect resolves via its own dynamic
// `import()` once a reset has happened.
it('should render nothing when mocks are switched off', async () => {
  vi.stubEnv('VITE_MOCKS', 'off');
  vi.resetModules();
  const { MockPanelMount } = await import('../MockPanelMount');
  const queryClient = new QueryClient();

  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>
        <MockPanelMount />
      </MemoryRouter>
    </QueryClientProvider>
  );

  expect(screen.queryByRole('button', { name: /mock controls/i })).toBeNull();
});

it('should offer the mock controls in development', async () => {
  vi.resetModules();
  const { MockPanelMount } = await import('../MockPanelMount');
  const queryClient = new QueryClient();

  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>
        <MockPanelMount />
      </MemoryRouter>
    </QueryClientProvider>
  );

  // The lazy factory resolves six dynamic imports; under a full parallel
  // workspace run their transforms can exceed waitFor's 1s default.
  await waitFor(
    () => expect(screen.queryByRole('button', { name: /mock controls/i })).not.toBeNull(),
    { timeout: 5000 }
  );
});

it('should invalidate cached queries when the scenario changes', async () => {
  vi.resetModules();
  const { MockPanelMount } = await import('../MockPanelMount');
  const { mockStore } = await import('@/mocks/store');
  const queryClient = new QueryClient();
  const invalidate = vi.spyOn(queryClient, 'invalidateQueries');

  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>
        <MockPanelMount />
      </MemoryRouter>
    </QueryClientProvider>
  );

  // The effect subscribes to the store behind a dynamic `import()`, which
  // resolves on a later microtask than this synchronous test body — retry
  // the scenario switch inside `waitFor` so it lands once the subscription
  // is live, instead of racing a single call against an unresolved import.
  await waitFor(() => {
    mockStore.setScenario('all-clear');
    expect(invalidate).toHaveBeenCalled();
  });

  mockStore.reset();
});

// StrictMode (see `@/app/index.tsx`) double-invokes effects — mount, cleanup,
// mount — before any promise has a chance to resolve. If cleanup ran while
// the dynamic `import('@/mocks/store')` was still pending, and the resolution
// callback subscribed unconditionally, the listener would outlive the
// component with nothing left holding its unsubscribe function.
it('should not invalidate queries when unmounted before the store subscription resolves', async () => {
  vi.resetModules();
  const { MockPanelMount } = await import('../MockPanelMount');
  const { mockStore } = await import('@/mocks/store');
  const queryClient = new QueryClient();
  const invalidate = vi.spyOn(queryClient, 'invalidateQueries');

  const { unmount } = render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>
        <MockPanelMount />
      </MemoryRouter>
    </QueryClientProvider>
  );
  unmount();

  // Give the pending `import('@/mocks/store')` a chance to resolve.
  await new Promise((resolve) => setTimeout(resolve, 0));

  mockStore.setScenario('all-clear');

  expect(invalidate).not.toHaveBeenCalled();

  mockStore.reset();
});
