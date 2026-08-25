import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { SetupDoors } from '../SetupDoors';

const renderDoors = (onNewFleet = vi.fn(), onRestore = vi.fn()) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <SetupDoors onNewFleet={onNewFleet} onRestore={onRestore} />
    </QueryClientProvider>
  );
  return { onNewFleet, onRestore };
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SetupDoors', () => {
  it('should offer both doors', () => {
    renderDoors();

    expect(screen.getByRole('button', { name: 'Start a new fleet' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Recover from a phrase' })).toBeTruthy();
  });

  it('should onboard a new fleet without asking the daemon to tolerate an existing one', async () => {
    const adminCallSpy = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValue({ onboarded: 'new', seats: 0 });
    renderDoors();

    fireEvent.click(screen.getByRole('button', { name: 'Start a new fleet' }));

    await waitFor(() =>
      expect(adminCallSpy).toHaveBeenCalledWith({ OnboardAsNew: { if_needed: false } })
    );
  });

  it('should advance only once the daemon has onboarded the host', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ onboarded: 'new', seats: 0 });
    const { onNewFleet } = renderDoors();

    expect(onNewFleet).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Start a new fleet' }));

    await waitFor(() => expect(onNewFleet).toHaveBeenCalled());
  });

  it('should take the restore door without calling the daemon', () => {
    const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall');
    const { onRestore } = renderDoors();

    fireEvent.click(screen.getByRole('button', { name: 'Recover from a phrase' }));

    expect(onRestore).toHaveBeenCalled();
    expect(adminCallSpy).not.toHaveBeenCalled();
  });

  it('should say recovery is only offered here', () => {
    renderDoors();

    expect(screen.getByText(/Recovery is only offered here/i)).toBeTruthy();
  });
});
