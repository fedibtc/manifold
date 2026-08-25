import type { GetSetupStateResponse, SetupStatus } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AppShell } from '../AppShell';

vi.mock('@/shared/api/hooks/use-setup-state/useSetupState', () => ({
  SETUP_STATE_KEY: ['setup-state'],
  useSetupState: vi.fn()
}));

import { useSetupState } from '@/shared/api/hooks/use-setup-state/useSetupState';

const NAV_LABELS = ['Overview', 'Funds', 'Advertisement', 'Allocations', 'Settings'];

const mockStatus = (status: SetupStatus) => {
  const data = { status } as GetSetupStateResponse;
  vi.mocked(useSetupState).mockReturnValue({ data } as ReturnType<typeof useSetupState>);
};

const renderShellAt = (path: string) => {
  const queryClient = new QueryClient();
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route element={<AppShell />}>
            <Route index element={<div>overview</div>} />

            <Route path="funds" element={<div>funds</div>} />

            <Route path="settings" element={<div>settings</div>} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
};

describe('AppShell', () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it('should disable every nav row while setup-state is not ready', () => {
    mockStatus('not_configured');
    // AppShell does not decide gating — SetupGate does, above the shell, and
    // normally replaces it outright. This only covers the nav's own
    // disabled-row rendering, for the window where the shell is mounted while
    // setup-state is between answers.
    renderShellAt('/');

    for (const label of NAV_LABELS) {
      const row = screen.getByText(label);
      expect(row.getAttribute('aria-disabled')).toBe('true');
      expect(row.getAttribute('title')).toBe('Complete setup first');
      expect(row.closest('a')).toBeNull();
    }
  });

  it('should enable every nav row when status is ready', () => {
    mockStatus('ready');
    renderShellAt('/');

    for (const label of NAV_LABELS) {
      const row = screen.getByText(label);
      expect(row.getAttribute('aria-disabled')).toBeNull();
      expect(row.closest('a')).not.toBeNull();
    }
  });

  it('should not offer a Setup row — setup is a full screen above the shell', () => {
    mockStatus('not_configured');
    renderShellAt('/');

    expect(screen.queryByText('Setup')).toBeNull();
  });
});
