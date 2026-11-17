import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { RESET_UPTIME_TAPS, UptimeTitle } from '../UptimeTitle';

const renderTitle = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <UptimeTitle />
    </QueryClientProvider>
  );
};

const tap = (times: number) => {
  for (let i = 0; i < times; i++) fireEvent.click(screen.getByRole('button', { name: 'Uptime' }));
};

const dialog = () => screen.queryByRole('dialog', { name: 'Reset uptime?' });

afterEach(() => {
  vi.restoreAllMocks();
});

describe('UptimeTitle', () => {
  it('should open the reset only on the 21st tap', () => {
    renderTitle();

    tap(RESET_UPTIME_TAPS - 1);
    expect(dialog()).toBeNull();

    tap(1);
    expect(dialog()).toBeInTheDocument();
    expect(RESET_UPTIME_TAPS).toBe(21);
    expect(screen.getByText('Only do this if your badge issuer asked you to.')).toBeInTheDocument();
    // Enter on the opened dialog must not reset.
    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus();
  });

  it('should start counting again after a cancel', () => {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall');
    renderTitle();

    tap(RESET_UPTIME_TAPS);
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(dialog()).toBeNull();

    tap(RESET_UPTIME_TAPS - 1);
    expect(dialog()).toBeNull();
    expect(adminCall).not.toHaveBeenCalled();
  });

  it('should reset uptime on confirm and say the checks are running again', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValue({ uptime_reset: 'rechecking' });
    renderTitle();

    tap(RESET_UPTIME_TAPS);
    fireEvent.click(screen.getByRole('button', { name: 'Reset uptime' }));

    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Uptime reset.'));
    expect(adminCall).toHaveBeenCalledWith('ResetUptime');
    expect(dialog()).toBeNull();
  });

  it('should keep the dialog open with the reason when the reset fails', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new Error('database is locked'));
    renderTitle();

    tap(RESET_UPTIME_TAPS);
    fireEvent.click(screen.getByRole('button', { name: 'Reset uptime' }));

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('database is locked'));
    expect(dialog()).toBeInTheDocument();
    expect(screen.queryByRole('status')).toBeNull();
  });
});
