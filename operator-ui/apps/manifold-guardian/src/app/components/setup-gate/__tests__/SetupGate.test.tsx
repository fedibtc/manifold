import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { StrictMode } from 'react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';
import { ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { gateSurface } from '@/shared/surface/gateSurface';
import { SetupGate } from '../SetupGate';

// Rejected with a message the gate must not be reading: the discriminant is the
// whole signal, so a daemon free to reword its sentence cannot close the wizard.
const stubNotOnboarded = () =>
  vi
    .spyOn(adminCallModule, 'adminCall')
    .mockRejectedValue(new AdminApiError('set this host up first', 'not_onboarded'));

const gateTree = (client: QueryClient) => (
  <QueryClientProvider client={client}>
    <MemoryRouter>
      <Routes>
        <Route element={<SetupGate />}>
          <Route index element={<div>dashboard</div>} />
        </Route>
      </Routes>
    </MemoryRouter>
  </QueryClientProvider>
);

const renderGate = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return { client, ...render(gateTree(client)) };
};

const deliverNextAnswer = async (client: QueryClient) => {
  await act(async () => {
    await client.invalidateQueries({ queryKey: ONBOARDING_KEY });
  });
};

afterEach(() => {
  gateSurface.clear('boot');
  gateSurface.clear('setup');
  vi.restoreAllMocks();
});

it('should name the setup wizard, which has no route of its own', async () => {
  stubNotOnboarded();
  renderGate();

  await screen.findByRole('heading', { name: 'Set up Manifold Fedimint Guardian' });

  expect(gateSurface.getSnapshot()).toBe('setup');
});

it('should leave the surface to the pathname once the wizard goes away', async () => {
  stubNotOnboarded();
  const { unmount } = renderGate();

  await screen.findByRole('heading', { name: 'Set up Manifold Fedimint Guardian' });
  unmount();

  expect(gateSurface.getSnapshot()).toBeNull();
});

it('should reopen an approved but unpriced fleet on the terms, not the price', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    stage: 'initial_offer',
    runtime: 'starting'
  });
  renderGate();

  await screen.findByRole('heading', { name: 'Accept the terms of service' });

  expect(screen.queryByRole('heading', { name: 'Set your price' })).toBeNull();
});

it('should stay on the unfinished stage even once the runtime reports ready', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    stage: 'initial_offer',
    runtime: 'ready'
  });
  renderGate();

  await screen.findByRole('heading', { name: 'Accept the terms of service' });

  expect(screen.queryByText('dashboard')).toBeNull();
});

it('should wait out a configured host whose runtime is still starting', async () => {
  const adminCall = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ stage: 'complete', runtime: 'starting' });
  const { client } = renderGate();

  await screen.findByRole('heading', { name: 'Manifold Fedimint Guardian is starting' });

  expect(screen.queryByRole('heading', { name: 'Set up Manifold Fedimint Guardian' })).toBeNull();
  expect(screen.queryByRole('button')).toBeNull();
  expect(screen.queryByText('Start fresh')).toBeNull();
  expect(screen.queryByText('Recover from a phrase')).toBeNull();
  expect(screen.queryByText('dashboard')).toBeNull();
  expect(gateSurface.getSnapshot()).toBeNull();

  adminCall.mockResolvedValue({ stage: 'complete', runtime: 'ready' });
  await deliverNextAnswer(client);

  await screen.findByText('dashboard');

  expect(
    screen.queryByRole('heading', { name: 'Manifold Fedimint Guardian is starting' })
  ).toBeNull();
});

// Finishing the last step stores the offer, and the daemon answers complete
// while the new runtime starts. The wizard can no longer tell which form it was
// on, so the starting screen takes over until the dashboard is ready.
it('should show the starting screen once first-time setup completes', async () => {
  const adminCall = stubNotOnboarded();
  const { client } = renderGate();

  await screen.findByRole('heading', { name: 'Set up Manifold Fedimint Guardian' });

  adminCall.mockResolvedValue({ stage: 'complete', runtime: 'starting' });
  await deliverNextAnswer(client);

  await screen.findByRole('heading', { name: 'Manifold Fedimint Guardian is starting' });
  expect(screen.queryByRole('heading', { name: 'Set up Manifold Fedimint Guardian' })).toBeNull();
  expect(screen.queryByText('dashboard')).toBeNull();

  adminCall.mockResolvedValue({ stage: 'complete', runtime: 'ready' });
  await deliverNextAnswer(client);

  await screen.findByText('dashboard');

  expect(screen.queryByRole('heading', { name: 'Set up Manifold Fedimint Guardian' })).toBeNull();
});

// The regression: a host set up without a badge stores its offer from the
// Monthly data step. The next answer is complete while starting, which no
// longer looks like a badgeless initial_offer, and the wizard used to flash the
// badged "Set your price" form before the dashboard opened.
it('should not flash the price form after a no-badge setup completes', async () => {
  const badgeless = { badgeless_mg: true, nostr: { state: 'checking' } };
  const adminCall = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ ...badgeless, stage: 'initial_offer', runtime: 'starting' });
  const { client } = renderGate();

  fireEvent.click(await screen.findByRole('button', { name: 'Accept and continue' }));
  await screen.findByRole('heading', { name: 'Set your monthly data' });

  adminCall.mockResolvedValue({ ...badgeless, stage: 'complete', runtime: 'starting' });
  await deliverNextAnswer(client);

  await screen.findByRole('heading', { name: 'Manifold Fedimint Guardian is starting' });
  expect(screen.queryByRole('heading', { name: 'Set your price' })).toBeNull();
  expect(screen.queryByRole('heading', { name: 'Set your monthly data' })).toBeNull();
});

it('should keep its surface through the StrictMode double invoke', async () => {
  stubNotOnboarded();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<StrictMode>{gateTree(client)}</StrictMode>);

  await screen.findByRole('heading', { name: 'Set up Manifold Fedimint Guardian' });

  expect(gateSurface.getSnapshot()).toBe('setup');
});
