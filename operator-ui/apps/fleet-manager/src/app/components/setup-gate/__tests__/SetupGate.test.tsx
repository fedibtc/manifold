import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { StrictMode } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';
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
      <SetupGate />
    </MemoryRouter>
  </QueryClientProvider>
);

const renderGate = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(gateTree(client));
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

it('should keep its surface through the StrictMode double invoke', async () => {
  stubNotOnboarded();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<StrictMode>{gateTree(client)}</StrictMode>);

  await screen.findByRole('heading', { name: 'Set up Manifold Fedimint Guardian' });

  expect(gateSurface.getSnapshot()).toBe('setup');
});
