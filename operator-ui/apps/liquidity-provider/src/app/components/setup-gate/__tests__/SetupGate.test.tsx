import type { GetSetupStateResponse, SetupStatus } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AppShell } from '@/app/components/app-shell/AppShell';
import { SetupGate } from '../SetupGate';

vi.mock('@/shared/api/hooks/use-setup-state/useSetupState', () => ({
  SETUP_STATE_KEY: ['setup-state'],
  useSetupState: vi.fn()
}));

import { useSetupState } from '@/shared/api/hooks/use-setup-state/useSetupState';

const mockStatus = (status: SetupStatus) => {
  const data = { status } as GetSetupStateResponse;
  vi.mocked(useSetupState).mockReturnValue({ data } as ReturnType<typeof useSetupState>);
};

const renderAt = (path: string) => {
  const queryClient = new QueryClient();
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route element={<SetupGate />}>
            <Route element={<AppShell />}>
              <Route index element={<div>overview</div>} />

              <Route path="setup" element={<div>setup route (behind the shell)</div>} />

              <Route path="funds" element={<div>funds</div>} />
            </Route>
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
};

describe('SetupGate', () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it('should hide the shell nav while setup is not ready', () => {
    mockStatus('not_configured');
    renderAt('/funds');

    expect(screen.queryByRole('navigation', { name: 'Sections' })).toBeNull();
    expect(screen.getByRole('heading', { name: 'Setup — Network' })).toBeTruthy();
  });

  it('should render the shell nav once setup is ready', () => {
    mockStatus('ready');
    renderAt('/funds');

    expect(screen.getByRole('navigation', { name: 'Sections' })).toBeTruthy();
    expect(screen.getByText('funds')).toBeTruthy();
  });
});
