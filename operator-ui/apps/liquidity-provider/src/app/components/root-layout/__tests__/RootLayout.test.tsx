import type { GetSetupStateResponse, SetupStatus } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { afterEach, vi } from 'vitest';
import { SetupGate } from '@/app/components/setup-gate/SetupGate';
import { RootLayout } from '../RootLayout';

vi.mock('@/shared/api/hooks/use-setup-state/useSetupState', () => ({
  SETUP_STATE_KEY: ['setup-state'],
  useSetupState: vi.fn()
}));

vi.mock('@/pages/setup/SetupPage', () => ({
  SetupPage: () => <div>setup wizard</div>
}));

import { useSetupState } from '@/shared/api/hooks/use-setup-state/useSetupState';

const mockStatus = (status: SetupStatus) => {
  const data = { status } as GetSetupStateResponse;
  vi.mocked(useSetupState).mockReturnValue({ data } as ReturnType<typeof useSetupState>);
};

const renderLayout = () => {
  const client = new QueryClient();
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={['/']}>
        <Routes>
          <Route element={<RootLayout />}>
            <Route index element={<div>root content</div>} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
};

const renderGatedApp = () => {
  const client = new QueryClient();
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={['/']}>
        <Routes>
          <Route element={<RootLayout />}>
            <Route element={<SetupGate />}>
              <Route index element={<div>shell</div>} />
            </Route>
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
};

afterEach(() => {
  vi.clearAllMocks();
});

it('should render the routed content in the outlet', () => {
  renderLayout();

  screen.getByText('root content');
});

// The whole reason the panel mounts here rather than inside AppShell: the
// setup wizard replaces the shell outright, and a developer who latched the
// gate with a scenario switch needs the control that switches it back.
it('should keep the mock controls available while the setup gate covers the screen', async () => {
  mockStatus('not_configured');
  renderGatedApp();

  screen.getByText('setup wizard');
  expect(screen.queryByText('shell')).toBeNull();

  await waitFor(() =>
    expect(screen.queryByRole('button', { name: /mock controls/i })).not.toBeNull()
  );
});
