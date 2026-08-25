import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { StrictMode } from 'react';
import { createMemoryRouter, MemoryRouter, RouterProvider } from 'react-router-dom';
import { afterEach, expect, it, vi } from 'vitest';
import { useBootStatus } from '@/features/boot/hooks/use-boot-status/useBootStatus';
import { AccessDeniedError, HttpStatusError, NetworkError } from '@/shared/api/errors';
import { gateSurface } from '@/shared/surface/gateSurface';
import { BootGate } from '../BootGate';

vi.mock('@/features/boot/hooks/use-boot-status/useBootStatus');

const useBootStatusMock = vi.mocked(useBootStatus);

const renderWithStatus = (
  status: ReturnType<typeof useBootStatus>['status'],
  failure: unknown = new NetworkError()
) => {
  useBootStatusMock.mockReturnValue({ status, failure, onRetry: vi.fn() });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createMemoryRouter([
    { element: <BootGate />, children: [{ index: true, element: <div>shell</div> }] }
  ]);
  return render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
};

// A plain tree rather than a data router, so a status change can be replayed
// through `rerender` — the gate surface is what these cases assert on, not the
// routed outlet.
const gateTree = (client: QueryClient) => (
  <QueryClientProvider client={client}>
    <MemoryRouter>
      <BootGate />
    </MemoryRouter>
  </QueryClientProvider>
);

const renderGate = (status: ReturnType<typeof useBootStatus>['status']) => {
  useBootStatusMock.mockReturnValue({ status, failure: new NetworkError(), onRetry: vi.fn() });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(gateTree(client));

  const setStatus = (next: ReturnType<typeof useBootStatus>['status']) => {
    useBootStatusMock.mockReturnValue({
      status: next,
      failure: new NetworkError(),
      onRetry: vi.fn()
    });
    view.rerender(gateTree(client));
  };

  return { ...view, setStatus };
};

afterEach(() => {
  gateSurface.clear('boot');
  gateSurface.clear('setup');
});

it('should render the boot loading state', () => {
  renderWithStatus('booting');

  screen.getByText('Loading…');
});

it('should render the sign-in prompt on needs-auth', () => {
  renderWithStatus('needs-auth');

  screen.getByRole('heading', { name: 'Sign in' });
});

it('should render the daemon error screen on daemon-unreachable', () => {
  renderWithStatus('daemon-unreachable');

  screen.getByText("Can't reach the fleet manager");
});

it('should hand the observed failure to the daemon error screen', () => {
  renderWithStatus('daemon-unreachable', new HttpStatusError(500));

  screen.getByText('POST /api/admin · HTTP 500');
});

it('should render the refused screen, not the routed shell, on access-denied', () => {
  renderWithStatus('access-denied', new AccessDeniedError());

  screen.getByText('The fleet manager refused this dashboard');
  screen.getByText('POST /api/admin · HTTP 403');
  expect(screen.queryByText('shell')).toBeNull();
  expect(screen.queryByRole('heading', { name: 'Sign in' })).toBeNull();
});

it('should render the outlet when ready', () => {
  renderWithStatus('ready');

  screen.getByText('shell');
});

it('should name the boot screen, which has no route of its own', () => {
  renderGate('booting');

  expect(gateSurface.getSnapshot()).toBe('boot');
});

it('should name the sign-in prompt, which has no route of its own', () => {
  renderGate('needs-auth');

  expect(gateSurface.getSnapshot()).toBe('auth');
});

it('should name the daemon error screen, which has no route of its own', () => {
  renderGate('daemon-unreachable');

  expect(gateSurface.getSnapshot()).toBe('daemon-error');
});

it('should name the daemon error screen for a refused dashboard too', () => {
  renderGate('access-denied');

  expect(gateSurface.getSnapshot()).toBe('daemon-error');
});

it('should name no surface once the routed tree is on the screen', () => {
  renderGate('ready');

  expect(gateSurface.getSnapshot()).toBeNull();
});

it('should hand the surface to the setup gate when the boot screen finishes', () => {
  gateSurface.set('setup', 'setup');
  const { setStatus } = renderGate('booting');
  expect(gateSurface.getSnapshot()).toBe('boot');

  setStatus('ready');

  expect(gateSurface.getSnapshot()).toBe('setup');
});

it('should hand the surface to the setup gate when the operator signs in', () => {
  gateSurface.set('setup', 'setup');
  const { setStatus } = renderGate('needs-auth');
  expect(gateSurface.getSnapshot()).toBe('auth');

  setStatus('ready');

  expect(gateSurface.getSnapshot()).toBe('setup');
});

it('should keep its surface through the StrictMode double invoke', () => {
  useBootStatusMock.mockReturnValue({ status: 'booting', failure: null, onRetry: vi.fn() });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });

  render(<StrictMode>{gateTree(client)}</StrictMode>);

  expect(gateSurface.getSnapshot()).toBe('boot');
});
