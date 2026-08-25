import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, vi } from 'vitest';
import { useSeatReports } from '@/features/seats/api/hooks/use-seat-reports/useSeatReports';
import * as adminCallModule from '@/shared/api/adminCall';
import { LIST_POLL_MS } from '@/shared/api/pollingIntervals';
import { seatStatusRefetchInterval } from '../seatStatusQuery';
import { useSeatStatus } from '../useSeatStatus';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

// Midpoint jitter, so every interval below is exactly its nominal value.
beforeEach(() => {
  vi.spyOn(Math, 'random').mockReturnValue(0.5);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('should call adminCall with SeatStatus for the given seat id', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ seat_id: 'seat-01' });

  const { result } = renderHook(() => useSeatStatus('seat-01'), { wrapper });

  await waitFor(() => expect(result.current.isSuccess).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith({ SeatStatus: { seat_id: 'seat-01' } });
});

it('should refetch every 5s while the seat is in a non-terminal phase', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    seat_id: 'seat-01',
    report: { state: 'active', health: 'healthy', phase: 'dkg_in_progress' }
  });

  renderHook(() => useSeatStatus('seat-01'), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});

// Regression (W2.3): a running seat used to stop being polled altogether, so the
// health chip on this page froze at whatever the last fetch had said.
it('should keep polling a running seat at the slow health cadence', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    seat_id: 'seat-running',
    report: { state: 'active', health: 'healthy', phase: 'running', invite_code: 'fed11testinvite' }
  });

  renderHook(() => useSeatStatus('seat-running'), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(LIST_POLL_MS);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});

it('should stop polling a decommissioned seat', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    seat_id: 'seat-gone',
    report: { state: 'decommissioned', at_ms: 0 }
  });

  renderHook(() => useSeatStatus('seat-gone'), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(LIST_POLL_MS);
  expect(adminCallSpy).toHaveBeenCalledTimes(1);
});

// The two screens are one seat. If they held two cache entries, or one entry
// under two policies, they could disagree about that seat on the same tab.
it('should share one cache entry and one polling policy with the seats list', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    seat_id: 'seat-shared',
    report: { state: 'active', health: 'healthy', phase: 'running', invite_code: 'fed11testinvite' }
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const bothScreens = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );

  const { result } = renderHook(
    () => {
      useSeatStatus('seat-shared');
      return useSeatReports(['seat-shared']);
    },
    { wrapper: bothScreens }
  );

  await waitFor(() => expect(result.current[0].isSuccess).toBe(true));
  const cached = client.getQueryCache().getAll();
  expect(cached).toHaveLength(1);
  expect(cached[0].observers.map((observer) => observer.options.refetchInterval)).toEqual([
    seatStatusRefetchInterval,
    seatStatusRefetchInterval
  ]);
});
