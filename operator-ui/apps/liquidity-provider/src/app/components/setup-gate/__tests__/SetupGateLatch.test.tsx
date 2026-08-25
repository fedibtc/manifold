import type { GetSetupStateResponse, SetupStatus } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AppShell } from '@/app/components/app-shell/AppShell';
import { SetupGate } from '../SetupGate';

vi.mock('@/shared/api/hooks/use-setup-state/useSetupState', () => ({
  SETUP_STATE_KEY: ['setup-state'],
  useSetupState: vi.fn()
}));

// A stub stands in for the real wizard: this suite is about the latch
// transition — when the shell reappears — not the wizard's steps, which
// SetupGate.test.tsx and SetupWizard.test.tsx already cover. The stub exposes
// the one thing that matters here, the onComplete the live screen raises.
vi.mock('@/pages/setup/SetupPage', () => ({
  SetupPage: ({ onComplete }: { onComplete: () => void }) => (
    <button type="button" onClick={onComplete}>
      Leave setup
    </button>
  )
}));

import { useSetupState } from '@/shared/api/hooks/use-setup-state/useSetupState';

const mockStatus = (status: SetupStatus) => {
  const data = { status } as GetSetupStateResponse;
  vi.mocked(useSetupState).mockReturnValue({ data } as ReturnType<typeof useSetupState>);
};

const app = (
  <MemoryRouter initialEntries={['/funds']}>
    <Routes>
      <Route element={<SetupGate />}>
        <Route element={<AppShell />}>
          <Route index element={<div>overview</div>} />

          <Route path="funds" element={<div>funds</div>} />
        </Route>
      </Route>
    </Routes>
  </MemoryRouter>
);

const renderApp = () =>
  render(<QueryClientProvider client={new QueryClient()}>{app}</QueryClientProvider>);

describe('SetupGate latch', () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  // Applying a config flips setup-state to `ready` before the operator has
  // left the setup screen — there is still a "you're live" confirmation to
  // read — so the latch must survive that flip and drop only when the wizard
  // says so.
  it('should keep the full-screen wizard mounted through a ready-status flip, then reveal the shell once the wizard completes', () => {
    mockStatus('not_configured');
    const { rerender } = renderApp();

    expect(screen.queryByRole('navigation', { name: 'Sections' })).toBeNull();
    expect(screen.getByText('Leave setup')).toBeTruthy();

    mockStatus('ready');
    rerender(<QueryClientProvider client={new QueryClient()}>{app}</QueryClientProvider>);

    expect(screen.queryByRole('navigation', { name: 'Sections' })).toBeNull();
    expect(screen.getByText('Leave setup')).toBeTruthy();

    fireEvent.click(screen.getByText('Leave setup'));

    expect(screen.getByRole('navigation', { name: 'Sections' })).toBeTruthy();
  });

  // Setup owns no route, so the location the operator was on is untouched
  // while the wizard covers the screen, and is what they land back on.
  it('should leave the operator on their original route once the gate lifts', () => {
    mockStatus('not_configured');
    const { rerender } = renderApp();

    mockStatus('ready');
    rerender(<QueryClientProvider client={new QueryClient()}>{app}</QueryClientProvider>);
    fireEvent.click(screen.getByText('Leave setup'));

    expect(screen.getByText('funds')).toBeTruthy();
  });

  // Completing the wizard is not a way to dismiss a gate the daemon still
  // wants: an unconfigured status re-latches on the next render.
  it('should re-latch if the wizard completes while setup is still unconfigured', () => {
    mockStatus('not_configured');
    renderApp();

    fireEvent.click(screen.getByText('Leave setup'));

    expect(screen.getByText('Leave setup')).toBeTruthy();
    expect(screen.queryByRole('navigation', { name: 'Sections' })).toBeNull();
  });

  it('should render the shell directly when setup is already ready', () => {
    mockStatus('ready');
    renderApp();

    expect(screen.getByRole('navigation', { name: 'Sections' })).toBeTruthy();
    expect(screen.queryByText('Leave setup')).toBeNull();
  });
});
