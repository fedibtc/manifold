import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { afterEach, beforeEach, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';
import { SEAT_FORMATION_POLL_MS } from '@/shared/api/pollingIntervals';
import { SeatDetailPage } from '../SeatDetailPage';

const runningSeat = (seatId: string) => ({
  seat_id: seatId,
  fi_id: 'fi-01',
  plan: { InfiniteBestEffort: { price_msats: 50_000_000 } },
  created_at_ms: 1_753_000_000_000,
  payment_claim: { state: 'success', at_ms: 0 },
  completion_callback: { state: 'not_configured' },
  decommissioned: false,
  report: { state: 'active', health: 'healthy', phase: 'running', invite_code: 'fed1abc' }
});

const renderAt = (seatId: string) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/seats/${seatId}`]}>
        <Routes>
          <Route path="/seats/:seatId" element={<SeatDetailPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  return client;
};

// Midpoint jitter, so the poll below lands on its nominal interval.
beforeEach(() => {
  vi.spyOn(Math, 'random').mockReturnValue(0.5);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('should show a running seat with its invite code and no decommission action', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    seat_id: 'seat-01',
    fi_id: 'fi-01',
    plan: { InfiniteBestEffort: { price_msats: 50_000_000 } },
    created_at_ms: 1_753_000_000_000,
    payment_claim: { state: 'success', at_ms: 0 },
    completion_callback: { state: 'not_configured' },
    decommissioned: false,
    report: { state: 'active', health: 'healthy', phase: 'running', invite_code: 'fed1abc' }
  });
  renderAt('seat-01');

  await waitFor(() => screen.getByText('fed1abc'));
});

it('should show the DKG-in-progress banner and no decommission-blocking control', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    seat_id: 'seat-02',
    fi_id: 'fi-01',
    plan: { InfiniteBestEffort: { price_msats: 50_000_000 } },
    created_at_ms: 1_753_000_000_000,
    payment_claim: { state: 'pending' },
    completion_callback: { state: 'not_configured' },
    decommissioned: false,
    report: { state: 'active', health: 'healthy', phase: 'dkg_in_progress' }
  });
  renderAt('seat-02');

  await waitFor(() => screen.getByText(/no Start\/Restart control here/i));
});

it('should explain an unavailable seat as supervised and recovering, not broken', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    seat_id: 'seat-03',
    fi_id: 'fi-01',
    plan: { InfiniteBestEffort: { price_msats: 50_000_000 } },
    created_at_ms: 1_753_000_000_000,
    payment_claim: { state: 'success', at_ms: 0 },
    completion_callback: { state: 'not_configured' },
    decommissioned: false,
    report: {
      state: 'active',
      health: 'unavailable',
      phase: 'running',
      invite_code: 'fed11testinvite'
    }
  });
  renderAt('seat-03');

  await waitFor(() => screen.getByText(/supervised and currently recovering/i));
});

it('should show an error message for an unknown seat', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AdminApiError('unknown seat'));
  renderAt('missing');

  await waitFor(() => screen.getByText('unknown seat'));
});

// Regression (W3.4): opening this page directly during a blip used to end
// polling for good, leaving an error screen whose only way out was to navigate
// away — while the same seat's row in the list kept retrying under the same key.
it('should keep polling after a failed first read and recover with no reload', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockRejectedValueOnce(new AdminApiError('seat unreadable'))
    .mockResolvedValue(runningSeat('seat-blip'));
  renderAt('seat-blip');

  await vi.waitFor(() => screen.getByText('seat unreadable'));
  await vi.advanceTimersByTimeAsync(SEAT_FORMATION_POLL_MS);

  await vi.waitFor(() => screen.getByText('fed1abc'));
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});

it('should retry the read when the operator asks', async () => {
  vi.spyOn(adminCallModule, 'adminCall')
    .mockRejectedValueOnce(new AdminApiError('seat unreadable'))
    .mockResolvedValue(runningSeat('seat-retry'));
  renderAt('seat-retry');

  await waitFor(() => screen.getByText('seat unreadable'));
  fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

  await waitFor(() => screen.getByText('fed1abc'));
});

it('should claim nothing about the seat before the daemon has answered', () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(() => new Promise(() => {}));
  renderAt('seat-pending');

  screen.getByText('Loading…');
  expect(screen.queryByRole('heading')).toBeNull();
});

// W3.1: the branch this page never had. A failed poll used to replace the whole
// report with an error screen, so an operator watching a seat lost the invite
// code and the health they were reading the moment the daemon blipped.
it('should keep the last-known report under a staleness marker when a poll fails', async () => {
  vi.spyOn(adminCallModule, 'adminCall')
    .mockResolvedValueOnce(runningSeat('seat-stale'))
    .mockRejectedValue(new AdminApiError('seat unreadable'));
  const client = renderAt('seat-stale');

  await waitFor(() => screen.getByText('fed1abc'));
  await act(async () => {
    await client.refetchQueries();
  });

  await screen.findByText('Showing last-known data');
  expect(screen.getByText('fed1abc')).toBeTruthy();
  expect(screen.queryByText('seat unreadable')).toBeNull();
});
